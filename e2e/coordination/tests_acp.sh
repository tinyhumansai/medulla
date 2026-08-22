#!/usr/bin/env bash
# ACP transport scenarios for the medulla coordination harness.
#
# `tests.sh` and `tests_multi.sh` drive the CLI transport: the daemon spawns the
# harness's own headless mode and reads its JSONL. This file drives the other
# one. With `MEDULLA_HARNESS_PROTOCOL=acp` the daemon instead spawns an **Agent
# Client Protocol server**, which spawns the harness — a different process, a
# different event stream, and for claude and codex a different *program*
# entirely (`npx @agentclientprotocol/…-acp`, not the CLI binary).
#
# That difference is what makes a separate suite worth having. Everything the
# CLI seam does at spawn time — selecting the preset's model, pointing the run at
# the routed endpoint, carrying the operator's hooks — has to be done again, by
# other means, on this path. Twice now it was not, and both times the run still
# looked healthy: the harness answered from the operator's own account and
# default model while the preset's endpoint sat unused in the environment. A
# suite that only asserted "a reply came back" would have passed through both.
#
# So the stack here boots **two** daemons against one mock LLM — one on each
# transport — and the assertions are comparative:
#
#   mock forwarder ──┬── daemon "acp" (MEDULLA_HARNESS_PROTOCOL=acp) ──┐
#                    └── daemon "cli" (the default)                   ──┴─→ mock LLM
#
#   1. round trip        — a task dispatched over ACP comes back with the marker.
#   2. client identity   — the ACP leg's request reached the mock from a
#                          *different client* than the CLI leg's. This is what
#                          proves the transport actually changed rather than the
#                          environment variable being ignored.
#   3. routed model      — the ACP leg's request names the preset's model, not
#                          whatever the harness runs by default.
#   4. transport parity  — both legs answer the same task the same way.
#
# `$E2E_HARNESS` selects the CLI as everywhere else, so this runs against
# opencode, claude and codex. Exit 0 iff every scenario passes.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

PASSED=()
scenario() { log ""; log "═══ SCENARIO: $1 ═══"; }
ok()       { PASSED+=("$1"); log "  ✓ SCENARIO PASS: $1"; }
skip()     { log "  ⊘ SCENARIO SKIP: $1"; }

SESSION="medulla-e2e-acp-$$"
# Distinct per-leg task text: it is how a request in the shared LLM journal is
# attributed back to the daemon that made it.
ACP_TASK="emit the coordination marker ACPLEG-$$"
CLI_TASK="emit the coordination marker CLILEG-$$"

# ACP dispatch is slower to first token than the CLI seam — it starts a server,
# which starts the harness — so the legs get a longer ceiling than elsewhere.
LEG_TIMEOUT_MS=300000
LEG_WAIT_S=320

main() {
  RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/medulla-e2e-XXXXXX")"
  e2e_init
  boot_forwarder acp cli
  boot_llm ""

  # One daemon per transport, each with its own workspace and enrolled identity,
  # sharing the mock LLM. The shared mock is the point: both legs' requests land
  # in one journal, which is what makes them comparable.
  local acp_ws cli_ws
  acp_ws="$(make_workspace acp)"
  cli_ws="$(make_workspace cli)"
  DAEMON_TRANSPORT=acp boot_daemon_named acp "$acp_ws"
  DAEMON_TRANSPORT=cli boot_daemon_named cli "$cli_ws"

  local acp_worker cli_worker
  acp_worker="$(worker_id acp)"
  cli_worker="$(worker_id cli)"

  # Both legs at once: they are independent, and ACP's slower start would
  # otherwise be paid in series.
  start_owner acp_leg --to "$acp_worker" \
    --task "$ACP_TASK" --task-id "acp-$$" --timeout-ms "$LEG_TIMEOUT_MS"
  start_owner cli_leg --to "$cli_worker" \
    --task "$CLI_TASK" --task-id "cli-$$" --timeout-ms "$LEG_TIMEOUT_MS"
  await_owner acp_leg "$LEG_WAIT_S"
  local acp_rc="$OWNER_RC"
  await_owner cli_leg "$LEG_WAIT_S"
  local cli_rc="$OWNER_RC"

  scenario_round_trip "$acp_rc"
  scenario_client_identity
  scenario_routed_model
  scenario_parity "$cli_rc"

  summarize
}

# ── 1. a task dispatched over ACP comes back with the marker ────────────────
scenario_round_trip() {
  scenario "a task dispatched over the ACP transport completes"
  [ "$1" = "0" ] || fail "ACP owner leg exited $1 (expected 0)"
  "$PYTHON_BIN" - "$RUN_DIR/acp_leg.json" <<'PY' || fail "ACP reply assertion failed"
import json, sys
frame = json.load(open(sys.argv[1]))
assert frame.get("kind") == "Reply", f"kind={frame.get('kind')!r} (expected Reply)"
text = frame.get("text") or ""
assert "COORDINATION_OK" in text, f"ACP reply missing COORDINATION_OK: {text!r}"
print(f"[e2e]   ACP reply: {text[:80]!r}", file=sys.stderr)
PY
  ok "ACP round trip"
}

# ── 2. the ACP leg reached the model through a different client ─────────────
#
# The strongest available proof that the transport really changed. Medulla
# selects ACP with an environment variable; if that variable were ignored — or
# if the ACP path silently fell back to the CLI seam — every other assertion in
# this file would still pass, because the *reply* is identical either way. The
# client that composed the HTTP request is not: the CLI leg's request is made by
# the harness binary, the ACP leg's by the ACP server's own SDK.
#
# Compared rather than matched against a literal, so this survives every version
# bump of either client.
scenario_client_identity() {
  scenario "the ACP leg's request reached the LLM from a different client"
  if ! harness_acp_is_separate_program; then
    skip "$HARNESS serves ACP from its own binary — both transports are the same client"
    return
  fi
  "$PYTHON_BIN" - "$RUN_DIR/llm.jsonl" "$ACP_TASK" "$CLI_TASK" \
    <<'PY' || fail "client identity assertion failed"
import json, sys
path, acp_needle, cli_needle = sys.argv[1], sys.argv[2], sys.argv[3]


def agents_for(needle):
    """Every distinct User-Agent that sent a request carrying `needle`."""
    found = set()
    for line in open(path, encoding="utf-8"):
        line = line.strip()
        if not line:
            continue
        record = json.loads(line)
        payload = record.get("payload") or {}
        if needle in json.dumps(payload.get("messages") or []):
            found.add(payload.get("user_agent") or "")
    return found


acp_agents, cli_agents = agents_for(acp_needle), agents_for(cli_needle)
assert acp_agents, "the ACP leg's task never reached the mock LLM"
assert cli_agents, "the CLI leg's task never reached the mock LLM"
shared = acp_agents & cli_agents
assert not shared, (
    "both transports reached the LLM as the same client "
    f"({shared!r}) — the ACP switch did not take effect"
)
print(f"[e2e]   acp client={sorted(acp_agents)!r}", file=sys.stderr)
print(f"[e2e]   cli client={sorted(cli_agents)!r}", file=sys.stderr)
PY
  ok "ACP client identity"
}

# ── 3. the ACP leg ran the model the preset asked for ───────────────────────
#
# The regression this pins is specific and was live in this repository: the
# routed provider configuration reached Codex's ACP server on the argv, which
# that server parses only for its `login` and `cli` subcommands and ignores
# entirely in server mode. The preset's model was therefore never selected and
# the session opened on Codex's own default, against the operator's own account.
# Nothing failed; the reply came back exactly as it does here.
scenario_routed_model() {
  scenario "the ACP leg runs the model the preset selected"
  "$PYTHON_BIN" - "$RUN_DIR/llm.jsonl" "$ACP_TASK" "$(harness_model)" \
    <<'PY' || fail "routed model assertion failed"
import json, sys
path, needle, expected = sys.argv[1], sys.argv[2], sys.argv[3]
models = set()
for line in open(path, encoding="utf-8"):
    line = line.strip()
    if not line:
        continue
    payload = json.loads(line).get("payload") or {}
    if needle in json.dumps(payload.get("messages") or []):
        models.add(payload.get("model") or "")
assert models, "the ACP leg's task never reached the mock LLM"
wrong = {model for model in models if model != expected}
assert not wrong, (
    f"the ACP leg asked for {sorted(wrong)!r} instead of the configured "
    f"{expected!r} — the routed model was dropped on this transport"
)
print(f"[e2e]   acp model={expected!r}", file=sys.stderr)
PY
  ok "ACP routed model"
}

# ── 4. both transports answer the same task the same way ────────────────────
#
# What must match is the shape the orchestrator sees: a Reply frame carrying the
# marker, from a task it dispatched the same way. The reply *texts* differ by
# design — each leg echoes its own task — so they are not compared.
#
# Token usage is deliberately only required of the CLI leg. ACP's `usage_update`
# carries the context window (`used` of `size`) rather than an input/output
# split, so there is nothing on that transport to fill `usage` with that would
# not be invented. This is a property of the protocol, not a gap in the fold —
# an orchestrator that bills or throttles on reported tokens gets nothing from an
# ACP run and has to account for that.
scenario_parity() {
  scenario "both transports answer the same task the same way"
  [ "$1" = "0" ] || fail "CLI owner leg exited $1 (expected 0)"
  "$PYTHON_BIN" - "$RUN_DIR/acp_leg.json" "$RUN_DIR/cli_leg.json" \
    <<'PY' || fail "transport parity assertion failed"
import json, sys
acp = json.load(open(sys.argv[1]))
cli = json.load(open(sys.argv[2]))
for name, frame in (("acp", acp), ("cli", cli)):
    assert frame.get("kind") == "Reply", f"{name} kind={frame.get('kind')!r}"
    assert "COORDINATION_OK" in (frame.get("text") or ""), f"{name} lost the marker"
    kinds = frame.get("frameKinds") or []
    assert kinds and kinds[0] == "ack" and kinds[-1] == "reply", \
        f"{name} frame sequence is not ack…reply: {kinds!r}"
usage = cli.get("usage") or {}
assert usage.get("inputTokens") is not None, "the CLI leg must report input usage"
assert usage.get("outputTokens") is not None, "the CLI leg must report output usage"
print(f"[e2e]   acp frames={acp.get('frameKinds')!r} usage={acp.get('usage')!r}", file=sys.stderr)
print(f"[e2e]   cli frames={cli.get('frameKinds')!r} usage={cli.get('usage')!r}", file=sys.stderr)
PY
  ok "transport parity"
}

summarize() {
  log ""
  log "═══════════════════════════════════════════════"
  printf '\n[e2e] PASS: all %d ACP scenarios green (%s):\n' "${#PASSED[@]}" "$HARNESS" >&2
  local s
  for s in "${PASSED[@]}"; do printf '[e2e]   ✓ %s\n' "$s" >&2; done
}

main "$@"

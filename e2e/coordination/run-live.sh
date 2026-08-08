#!/usr/bin/env bash
# Live counterpart to the mocked coordination suite: the SAME two-daemon fleet,
# but pointed at real infrastructure instead of the local mocks.
#
#   mocked (tests_multi.sh)             live (this script)
#   ───────────────────────             ──────────────────
#   mock_link_forwarder (loopback)  →   a deployed host-link forwarder
#   mock_llm.py         (loopback)  →   OpenRouter
#
# Everything else — the daemons, opencode, the owner legs, the assertions — is
# identical, which is the point: this is how you find out that a green mocked
# suite still breaks against real infrastructure.
#
# ── STATUS: THE TRANSPORT HALF CANNOT RUN YET ────────────────────────────────
#
# The suite used to reach a hosted relay over HTTP, and enrollment was a matter
# of publishing keys. The host link needs two things this repository cannot
# supply on its own, and `preflight` refuses to start without them rather than
# quietly running against something else:
#
#   1. A deployed forwarder. `MEDULLA_LINK_FORWARDER=<host:port>` must name one
#      that implements docs/host-link-protocol.md §5. None is deployed today.
#   2. Enrolled identities. §7.2's invite/enroll endpoints have no client here —
#      nothing in this repository can obtain a node id, a forwarder key or a
#      forwarder endpoint from a backend. So each end's `node.json` (§7.3) must
#      already exist, and its directory is passed in.
#
# Until both exist this script exits 2 with that explanation. The scenarios below
# are kept because they are what should run the moment it can, and because the
# mocked suite is deliberately their twin — but they have never been run against
# real infrastructure, so treat the first live run as a bring-up, not a
# regression check.
#
# THIS SPENDS REAL MONEY AND TALKS TO REAL SERVERS. It is opt-in on every axis:
#
#   E2E_LIVE=1                     required; the deliberate "yes I mean it" switch
#   OPENROUTER_API_KEY=sk-or-…     required; billed per token
#   MEDULLA_LINK_FORWARDER=h:p     required; the deployed forwarder (see above)
#   MEDULLA_LINK_HOME_<name>       required; a pre-enrolled MEDULLA_HOME per
#                                  daemon (alpha, beta) holding link/node.json
#   MEDULLA_LINK_OWNER_DIR_<name>  required; the matching orchestrator identity
#                                  directory (its own node.json)
#   MEDULLA_STAGING=1              default; set MEDULLA_STAGING=0 to target prod,
#                                  which additionally requires E2E_ALLOW_PROD=1
#
# Optional:
#   LIVE_MODEL=<slug>          OpenRouter model (default: a cheap small model)
#   E2E_KEEP=1                 keep the run dir + tmux session for inspection
#
# Usage:
#   E2E_LIVE=1 OPENROUTER_API_KEY=sk-or-… MEDULLA_LINK_FORWARDER=host:port \
#     MEDULLA_LINK_HOME_alpha=… MEDULLA_LINK_OWNER_DIR_alpha=… \
#     MEDULLA_LINK_HOME_beta=…  MEDULLA_LINK_OWNER_DIR_beta=… \
#     bash e2e/coordination/run-live.sh
#
# Exit 0 iff every live scenario passes; 2 if the live path is not available.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

# Keep the default cheap: this suite is chatty (a capabilities probe plus a task
# per daemon) and nobody wants a surprise bill from an e2e run.
LIVE_MODEL="${LIVE_MODEL:-openai/gpt-4o-mini}"

PASSED=()
scenario() { log ""; log "═══ LIVE SCENARIO: $1 ═══"; }
ok()       { PASSED+=("$1"); log "  ✓ LIVE PASS: $1"; }

# Abort before the stack exists. `fail` dumps diagnostics from RUN_DIR/SESSION,
# which preflight runs too early to have — use this for guardrail failures.
die() { printf '[e2e] FAIL: %s\n' "$*" >&2; exit 1; }

# Refuse, loudly and specifically, when the live transport does not exist.
unavailable() {
  printf '%s\n' \
    "[e2e] FAIL: the live coordination suite cannot run yet." \
    "" \
    "  $1" \
    "" \
    "  The host link replaced the hosted relay this suite used to target." \
    "  A live run now needs BOTH of:" \
    "    * a deployed forwarder implementing docs/host-link-protocol.md §5," \
    "      named by MEDULLA_LINK_FORWARDER=<host:port>. None is deployed." \
    "    * per-endpoint identities from §7.2 enrollment. This repository has no" \
    "      client for the invite/enroll endpoints, so node.json must already" \
    "      exist for each daemon and each orchestrator, and be passed in via" \
    "      MEDULLA_LINK_HOME_<name> / MEDULLA_LINK_OWNER_DIR_<name>." \
    "" \
    "  Until then the mocked fleet (tests_multi.sh) is the coverage that runs." >&2
  exit 2
}

# ── guardrails ──────────────────────────────────────────────────────────────
# Every check here fails closed. A live suite that runs by accident — in CI, in
# a loop, against prod — is worse than one that is annoying to start.
preflight() {
  # This suite writes its own opencode provider block against a live OpenRouter
  # key, so it is opencode-only. The other harnesses reach a live endpoint
  # through a Medulla preset instead, which is a different arrangement than the
  # one written below — running it here would silently test neither.
  [ "$HARNESS" = "opencode" ] || {
    printf 'refusing to run: run-live.sh is opencode-only (E2E_HARNESS=%s)\n' "$HARNESS" >&2
    exit 2
  }
  [ "${E2E_LIVE:-0}" = "1" ] || {
    printf '%s\n' \
      "refusing to run: this suite bills a real OpenRouter key and talks to real" \
      "infrastructure. Re-run with E2E_LIVE=1 if that is what you want." >&2
    exit 2
  }
  [ -n "${OPENROUTER_API_KEY:-}" ] \
    || die "OPENROUTER_API_KEY is unset — the live suite has no model to call"

  # Default to staging. Prod needs a second, separate opt-in.
  : "${MEDULLA_STAGING:=1}"
  export MEDULLA_STAGING
  if [ "$MEDULLA_STAGING" != "1" ] && [ "${E2E_ALLOW_PROD:-0}" != "1" ]; then
    die "MEDULLA_STAGING=$MEDULLA_STAGING targets production; set E2E_ALLOW_PROD=1 to confirm"
  fi

  [ -n "${MEDULLA_LINK_FORWARDER:-}" ] \
    || unavailable "MEDULLA_LINK_FORWARDER is unset."
  local name home_var owner_var
  for name in alpha beta; do
    home_var="MEDULLA_LINK_HOME_$name"
    owner_var="MEDULLA_LINK_OWNER_DIR_$name"
    [ -n "${!home_var:-}" ] && [ -d "${!home_var:-}/link" ] \
      || unavailable "$home_var does not name an enrolled MEDULLA_HOME."
    [ -n "${!owner_var:-}" ] && [ -f "${!owner_var:-}/node.json" ] \
      || unavailable "$owner_var does not name an enrolled orchestrator identity."
  done
  FORWARDER_ADDR="$MEDULLA_LINK_FORWARDER"

  log "forwarder: $FORWARDER_ADDR"
  log "model:  $LIVE_MODEL (OpenRouter)"
  log "key:    ${OPENROUTER_API_KEY:0:8}…${OPENROUTER_API_KEY: -4}"
}

# Adopt a pre-enrolled pair instead of minting one, setting the same globals
# `enroll_pair` sets so every lib.sh helper works unchanged.
#
# The identity directories are *symlinked*, never copied. `node.json` holds the
# sequence reservation of §3.1, and two processes advancing two copies of one
# counter under one pair key reuses AEAD nonces — a confidentiality break, not a
# tidiness problem.
adopt_pair() {
  local name="$1"
  local home_var="MEDULLA_LINK_HOME_$name" owner_var="MEDULLA_LINK_OWNER_DIR_$name"
  mkdir -p "$RUN_DIR/mhome-$name/e2e" "$RUN_DIR/owner-$name" "$RUN_DIR/queue-$name"
  # `MEDULLA_HOME` is the install root and the daemon runs as account `e2e`
  # (see lib.sh's boot_daemon_named), so the host identity is adopted there.
  ln -sfn "${!home_var}/link" "$RUN_DIR/mhome-$name/e2e/link"
  ln -sfn "${!owner_var}" "$RUN_DIR/owner-$name/link"

  local host_id owner_id
  host_id="$(node_id_of "$RUN_DIR/mhome-$name/e2e/link/node.json")"
  owner_id="$(node_id_of "$RUN_DIR/owner-$name/link/node.json")"
  [ -n "$host_id" ] && [ -n "$owner_id" ] || die "could not read node ids for '$name'"
  printf -v "HOST_NODE_ID_$name" '%s' "$host_id"
  printf -v "OWNER_NODE_ID_$name" '%s' "$owner_id"
  printf -v "OWNER_FOR_$host_id" '%s' "$name"
  printf -v "HOST_FOR_$owner_id" '%s' "$host_id"
  log "adopted '$name': host=$host_id owner=$owner_id"
}

# Echo the `node_id` field of a node.json.
node_id_of() {
  "$PYTHON_BIN" -c 'import json,sys; print(json.load(open(sys.argv[1]))["node_id"])' "$1"
}

# Render the live opencode config: real OpenRouter provider, real key, real model.
# Mirrors lib.sh's boot_llm, which writes the mock config — the daemons cannot
# tell the difference, which is exactly what makes this comparison meaningful.
write_live_opencode_config() {
  OC_CONFIG="$RUN_DIR/opencode.json"
  "$PYTHON_BIN" - "$SCRIPT_DIR/opencode.live.json" "$OC_CONFIG" \
    "$LIVE_MODEL" "$OPENROUTER_API_KEY" <<'PY'
import json, sys
src, dst, model, key = sys.argv[1:5]
cfg = json.load(open(src))
cfg["model"] = f"openrouter/{model}"
prov = cfg["provider"]["openrouter"]
prov["options"]["apiKey"] = key
prov["models"] = {model: {"name": model}}
json.dump(cfg, open(dst, "w"), indent=2)
PY
  chmod 600 "$OC_CONFIG"  # it holds a live API key
  log "wrote live opencode config (mode 600)"
}

# Boot the live fleet. No mock forwarder, no boot_llm — those are the mocks we
# are deliberately replacing; the owner drivers and daemons are the same ones the
# mocked suite runs.
boot_live_fleet() {
  SESSION="medulla-live-$$"
  RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/medulla-live-XXXXXX")"
  e2e_init
  write_live_opencode_config

  local name
  for name in alpha beta; do
    adopt_pair "$name"
    boot_owner "$name"
  done

  WORK_ALPHA="$(make_workspace alpha "")"
  WORK_BETA="$(make_workspace beta "")"
  boot_daemon_named alpha "$WORK_ALPHA"
  boot_daemon_named beta  "$WORK_BETA"
}

main() {
  preflight
  boot_live_fleet

  local alpha beta
  alpha="$(worker_id alpha)"
  beta="$(worker_id beta)"

  # ── 1. two daemons serve on the real forwarder ────────────────────────────
  scenario "two daemons serve over the live forwarder as distinct hosts"
  [ -n "$alpha" ] && [ -n "$beta" ] || fail "a daemon failed to open its link"
  [ "$alpha" != "$beta" ] || fail "both daemons hold the SAME node id ($alpha)"
  log "  alpha=$alpha  beta=$beta"
  ok "live fleet boot"

  # ── 2. capabilities against a real model ──────────────────────────────────
  # A real model actually answers the probe, so unlike the mock run we can assert
  # the model populated `tools` — proof the probe round-tripped through OpenRouter
  # rather than falling back to the deterministic local digest.
  scenario "capabilities probe round-trips through OpenRouter"
  run_owner caps_alpha --to "$alpha" \
    --kind capabilities --task "report" --task-id "live-caps-$$" --timeout-ms 180000
  [ "$OWNER_RC" = "0" ] || fail "live capabilities owner exited $OWNER_RC"
  "$PYTHON_BIN" - "$RUN_DIR/caps_alpha.json" "$WORK_ALPHA" <<'PY' \
    || fail "live capabilities assertion failed"
import json, sys
frame = json.load(open(sys.argv[1]))
mine = sys.argv[2]
assert frame.get("kind") == "CapabilitiesResult", f"kind={frame.get('kind')!r}"
caps = json.loads(frame.get("text") or "{}")
assert "opencode" in (caps.get("providers") or []), f"providers={caps.get('providers')!r}"
cwd = caps.get("cwd") or ""
assert cwd.rstrip("/").endswith(mine.rstrip("/").rsplit("/", 1)[-1]), \
    f"cwd {cwd!r} is not the assigned workspace {mine!r}"
tools = caps.get("tools") or []
assert tools, "a real model reported no tools — the probe likely fell back to local facts"
print(f"[e2e]   live caps: cwd={cwd!r} tools={len(tools)}", file=sys.stderr)
PY
  ok "live capabilities probe"

  # ── 3. concurrent routing against real infrastructure ─────────────────────
  # A real model will not echo a marker verbatim the way the mock does, so the
  # routing assertion moves to the transport layer: each reply must come back
  # correlated to its own taskId. That is the property we actually care about.
  scenario "concurrent live tasks route back to the right task"
  start_owner live_alpha --to "$alpha" \
    --task "Reply with exactly the word ALPHAOK and nothing else." \
    --task-id "live-a-$$" --timeout-ms 300000
  start_owner live_beta --to "$beta" \
    --task "Reply with exactly the word BETAOK and nothing else." \
    --task-id "live-b-$$" --timeout-ms 300000
  await_owner live_alpha 320
  [ "$OWNER_RC" = "0" ] || fail "live alpha task exited $OWNER_RC"
  await_owner live_beta 320
  [ "$OWNER_RC" = "0" ] || fail "live beta task exited $OWNER_RC"
  "$PYTHON_BIN" - "$RUN_DIR/live_alpha.json" "live-a-$$" "$RUN_DIR/live_beta.json" "live-b-$$" \
    <<'PY' || fail "live routing assertion failed"
import json, sys
a, a_id, b, b_id = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
for path, want in ((a, a_id), (b, b_id)):
    frame = json.load(open(path))
    assert frame.get("kind") == "Reply", f"{want}: kind={frame.get('kind')!r}"
    got = frame.get("taskId")
    assert got == want, f"reply correlated to the WRONG task: got {got!r}, want {want!r}"
    assert (frame.get("text") or "").strip(), f"{want}: empty reply from a live model"
print("[e2e]   both live replies correlated to their own taskId", file=sys.stderr)
PY
  ok "live concurrent routing"

  log ""
  log "═══════════════════════════════════════════════"
  printf '\n[e2e] LIVE PASS: all %d scenarios green:\n' "${#PASSED[@]}" >&2
  local s
  for s in "${PASSED[@]}"; do printf '[e2e]   ✓ %s\n' "$s" >&2; done
  log ""
  log "note: live runs move real traffic through $FORWARDER_ADDR"
}

main "$@"

#!/usr/bin/env bash
# End-to-end "live harness" for the medulla coordination round trip, driven by
# tmux with real processes:
#
#   owner driver (a real medulla-link endpoint)
#     → mock link forwarder (blind UDP; docs/host-link-protocol.md §5)
#       → `medulla daemon` (real binary, the host end of the same link)
#         → the REAL coding CLI (spawned by the daemon as its provider)
#           → mock LLM → `COORDINATION_OK <echo>`
#     ← the reply frame flows back over the link, printed as JSON, exit 0.
#
# The forwarder is the only mocked transport piece, and it is blind by
# construction: it authenticates the 58-byte cleartext header with each node's
# forwarder key and copies the ChaCha20-Poly1305 payload verbatim. Every byte of
# payload encryption is the real `medulla-link` crate on both ends.
#
# Which CLI that is comes from `$E2E_HARNESS` — `opencode` (the default),
# `claude` or `codex`. The claude and codex legs reach the mock through a Medulla
# *custom harness preset*, which is the same routing path a real OpenRouter
# preset takes, so the only fake in the chain is the model behind the endpoint.
# See `harness.sh`.
#
# tmux is a hard requirement: both `medulla daemon` and the coding CLI run under
# tmux control. Every process gets its own tmux window; a second "smoke" window
# drives the CLI's interactive TUI directly against the same mock LLM so tmux
# proves it controls the harness as well as medulla.
#
# All traffic is loopback. No real provider keys. Deterministic.
#
# This is the HAPPY-PATH entrypoint: `bash run.sh` (no args) boots the stack,
# runs the smoke leg + a single task round trip, asserts, and exits 0 on PASS.
# The Docker image wraps exactly this. Additional functional scenarios live in
# `tests.sh`, which shares the boot/teardown helpers in `lib.sh`.
#
# Shared-boot helpers + all env knobs are documented in lib.sh; overrides:
#   MEDULLA_BIN / FORWARDER_BIN / OWNER_BIN            prebuilt binaries
#   OPENCODE_BIN / CLAUDE_BIN / CODEX_BIN              prebuilt coding CLIs
#   E2E_HARNESS=opencode|claude|codex  which CLI to drive (default: opencode)
#   E2E_KEEP=1     keep the run dir + tmux session on exit (debugging)
#   E2E_SMOKE=0    skip the interactive TUI smoke leg
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

SESSION="medulla-e2e-$$"
OWNER_TASK="emit the coordination marker E2E-$$-$RANDOM"

main() {
  e2e_init

  boot_forwarder daemon
  boot_llm ""
  boot_daemon

  # Interactive opencode TUI smoke leg (tmux drives opencode directly).
  if [ "${E2E_SMOKE:-1}" != "0" ]; then
    smoke_leg
  else
    log "smoke leg skipped (E2E_SMOKE=0)"
  fi

  # Owner driver: send the task frame over the link, wait for the reply.
  run_owner owner --to "$WORKER_ID" \
    --task "$OWNER_TASK" --task-id "coord-$$" --timeout-ms 180000

  assert_all "$OWNER_RC"
}

assert_all() {
  local owner_rc="$1"
  log "asserting results…"

  # (a) owner exited 0, kind == Reply, text contains COORDINATION_OK.
  [ "$owner_rc" = "0" ] || fail "owner exited $owner_rc (expected 0)"
  [ -s "$RUN_DIR/owner.json" ] || fail "owner produced no terminal frame JSON"
  "$PYTHON_BIN" - "$RUN_DIR/owner.json" <<'PY' || fail "owner reply assertion failed"
import json, sys
frame = json.load(open(sys.argv[1]))
assert frame.get("kind") == "Reply", f"kind={frame.get('kind')!r} (expected Reply)"
text = frame.get("text") or ""
assert "COORDINATION_OK" in text, f"reply missing COORDINATION_OK: {text!r}"
print(f"[e2e]   (a) reply OK: {text[:80]!r}", file=sys.stderr)
PY

  # (b) mock LLM log has >=1 completion request, on the wire dialect this
  # harness is supposed to speak, whose messages include the task text.
  [ -f "$RUN_DIR/llm.jsonl" ] || fail "mock LLM wrote no request log"
  "$PYTHON_BIN" - "$RUN_DIR/llm.jsonl" "$OWNER_TASK" "$(harness_llm_kind)" \
    <<'PY' || fail "mock LLM assertion failed"
import json, sys
path, needle, kind = sys.argv[1], sys.argv[2], sys.argv[3]
chats = hit = 0
for line in open(path):
    line = line.strip()
    if not line:
        continue
    rec = json.loads(line)
    if rec.get("kind") != kind:
        continue
    chats += 1
    blob = json.dumps(rec.get("payload", {}).get("messages") or [])
    if needle in blob:
        hit += 1
assert chats >= 1, f"no {kind} requests reached the mock LLM"
assert hit >= 1, f"task text never appeared in an LLM {kind} request ({chats} seen)"
print(f"[e2e]   (b) LLM saw the task in {hit}/{chats} {kind} request(s)", file=sys.stderr)
PY

  # (c) the forwarder moved state-carrying datagrams in BOTH directions.
  assert_bidirectional_delivery "$RUN_DIR/owner.json"
  log "  (c) bidirectional delivery across the forwarder confirmed"

  printf '\n[e2e] PASS: coordination round trip green — owner=Reply(COORDINATION_OK) via the real %s CLI, LLM saw the task, bidirectional link delivery confirmed.\n' "$HARNESS" >&2
}

main "$@"

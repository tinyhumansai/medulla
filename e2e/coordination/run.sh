#!/usr/bin/env bash
# End-to-end "live harness" for the medulla coordination round trip, driven by
# tmux with real processes:
#
#   owner driver
#     → mock tiny.place Signal server (real X3DH/double-ratchet in the SDK)
#       → `medulla daemon` (real binary)
#         → the REAL `opencode` CLI (spawned by the daemon as its provider)
#           → mock OpenAI-compatible LLM → `COORDINATION_OK <echo>`
#     ← encrypted reply frame flows back, printed as JSON, exit 0.
#
# tmux is a hard requirement: both `medulla daemon` and `opencode` run under tmux
# control. Every process gets its own tmux window; a second "smoke" window drives
# an interactive `opencode` TUI directly against the same mock LLM so tmux proves
# it controls opencode as well as medulla.
#
# All traffic is loopback. No real provider keys. Deterministic.
#
# This is the HAPPY-PATH entrypoint: `bash run.sh` (no args) boots the stack,
# runs the smoke leg + a single task round trip, asserts, and exits 0 on PASS.
# The Docker image wraps exactly this. Additional functional scenarios live in
# `tests.sh`, which shares the boot/teardown helpers in `lib.sh`.
#
# Shared-boot helpers + all env knobs are documented in lib.sh; overrides:
#   MEDULLA_BIN / MOCK_SIGNAL_BIN / OWNER_BIN / OPENCODE_BIN  prebuilt binaries
#   E2E_KEEP=1     keep the run dir + tmux session on exit (debugging)
#   E2E_SMOKE=0    skip the interactive opencode TUI smoke leg
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

SESSION="medulla-e2e-$$"
OWNER_TASK="emit the coordination marker E2E-$$-$RANDOM"

main() {
  e2e_init

  boot_signal
  boot_llm ""
  boot_daemon

  # Interactive opencode TUI smoke leg (tmux drives opencode directly).
  if [ "${E2E_SMOKE:-1}" != "0" ]; then
    smoke_leg
  else
    log "smoke leg skipped (E2E_SMOKE=0)"
  fi

  # Owner driver: send task, drain for the encrypted reply.
  run_owner owner --endpoint "$SIGNAL_URL" --to "$WORKER_ID" \
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

  # (b) mock LLM log has >=1 chat request whose messages include the task text.
  [ -f "$RUN_DIR/llm.jsonl" ] || fail "mock LLM wrote no request log"
  "$PYTHON_BIN" - "$RUN_DIR/llm.jsonl" "$OWNER_TASK" <<'PY' || fail "mock LLM assertion failed"
import json, sys
path, needle = sys.argv[1], sys.argv[2]
chats = hit = 0
for line in open(path):
    line = line.strip()
    if not line:
        continue
    rec = json.loads(line)
    if rec.get("kind") != "chat":
        continue
    chats += 1
    blob = json.dumps(rec.get("payload", {}).get("messages") or [])
    if needle in blob:
        hit += 1
assert chats >= 1, "no chat requests reached the mock LLM"
assert hit >= 1, f"task text never appeared in an LLM chat request ({chats} chats)"
print(f"[e2e]   (b) LLM saw the task in {hit}/{chats} chat request(s)", file=sys.stderr)
PY

  # (c) /debug/stored shows envelopes delivered in BOTH directions.
  assert_bidirectional_delivery "$RUN_DIR/owner.json"
  log "  (c) bidirectional encrypted delivery confirmed"

  printf '\n[e2e] PASS: coordination round trip green — owner=Reply(COORDINATION_OK) via real opencode, LLM saw the task, bidirectional encrypted delivery confirmed.\n' >&2
}

main "$@"

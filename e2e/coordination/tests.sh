#!/usr/bin/env bash
# Functional scenario suite for the medulla coordination harness.
#
# Reuses the real-process stack helpers in `lib.sh` (mock Signal server →
# `medulla daemon` → real `opencode` → mock LLM). The happy-path round trip lives
# in `run.sh`; this file adds the functional edges around it:
#
#   1. capabilities  — a `capabilities` probe returns a capabilities_result frame
#                      advertising the opencode provider (a distinct round trip,
#                      different frame kind, on the running daemon).
#   2. token-usage   — a task reply carries propagated child token usage
#                      (regression guard for the opencode `tokens:{input,output}`
#                      shape the daemon must surface to the orchestrator).
#   3. second-round  — a second delegation from a fresh owner identity succeeds on
#                      the same running daemon (daemon is not single-shot).
#   4. bad-provider  — a task requesting an unavailable provider comes back as an
#                      Error frame, not a hang or a wrong-provider run.
#   5. custom-marker — a bespoke MOCK_LLM_MARKER flows the whole chain end to end
#                      (fresh stack; proves the marker is not hard-coded anywhere).
#
# Scenarios 1–4 share one booted stack (cheap, isolated assertions); scenario 5
# boots a fresh stack because the marker is baked into the LLM at launch.
#
# The interactive TUI smoke leg is exercised by run.sh, so it is skipped here to
# keep the suite fast. Same env overrides as run.sh (see lib.sh). Exit 0 iff every
# scenario passes.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

PASSED=()
scenario() { log ""; log "═══ SCENARIO: $1 ═══"; }
ok()       { PASSED+=("$1"); log "  ✓ SCENARIO PASS: $1"; }

# Teardown the current stack (respecting E2E_KEEP) so a fresh one can boot.
teardown_stack() {
  if [ "${E2E_KEEP:-0}" = "1" ]; then
    log "E2E_KEEP=1 — leaving session $SESSION and run dir $RUN_DIR"
    return
  fi
  tmux kill-session -t "$SESSION" 2>/dev/null || true
  rm -rf "$RUN_DIR" 2>/dev/null || true
}

# ── group A: four scenarios against one shared stack ────────────────────────
group_shared_stack() {
  SESSION="medulla-e2e-$$-a"
  RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/medulla-e2e-XXXXXX")"
  e2e_init
  boot_signal
  boot_llm ""
  boot_daemon

  # ── 1. capabilities probe ────────────────────────────────────────────────
  scenario "capabilities probe returns opencode in a capabilities_result"
  run_owner caps --endpoint "$SIGNAL_URL" --to "$WORKER_ID" \
    --kind capabilities --task "report" --task-id "caps-$$" --timeout-ms 120000
  [ "$OWNER_RC" = "0" ] || fail "capabilities owner exited $OWNER_RC (expected 0)"
  "$PYTHON_BIN" - "$RUN_DIR/caps.json" <<'PY' || fail "capabilities assertion failed"
import json, sys
frame = json.load(open(sys.argv[1]))
assert frame.get("kind") == "CapabilitiesResult", f"kind={frame.get('kind')!r}"
caps = json.loads(frame.get("text") or "{}")
providers = caps.get("providers") or []
assert "opencode" in providers, f"providers missing opencode: {providers!r}"
assert caps.get("cwd"), f"capabilities missing cwd: {caps!r}"
print(f"[e2e]   caps: providers={providers} cwd={caps.get('cwd')!r}", file=sys.stderr)
PY
  ok "capabilities probe"

  # ── 2. token-usage propagation ───────────────────────────────────────────
  scenario "task reply carries propagated child token usage"
  run_owner usage --endpoint "$SIGNAL_URL" --to "$WORKER_ID" \
    --task "emit the marker for USAGE-$$" --task-id "usage-$$" --timeout-ms 180000
  [ "$OWNER_RC" = "0" ] || fail "usage owner exited $OWNER_RC (expected 0)"
  "$PYTHON_BIN" - "$RUN_DIR/usage.json" <<'PY' || fail "token-usage assertion failed"
import json, sys
frame = json.load(open(sys.argv[1]))
assert frame.get("kind") == "Reply", f"kind={frame.get('kind')!r}"
assert "COORDINATION_OK" in (frame.get("text") or ""), "reply missing marker"
# The daemon acks before replying.
assert "ack" in (frame.get("frameKinds") or []), f"no ack frame: {frame.get('frameKinds')!r}"
usage = frame.get("usage")
assert usage is not None, "reply carried no token usage (usage propagation broken)"
for k in ("inputTokens", "outputTokens"):
    assert isinstance(usage.get(k), int), f"usage.{k} not an int: {usage!r}"
    assert usage[k] >= 0, f"usage.{k} negative: {usage!r}"
print(f"[e2e]   usage: {usage}  frameKinds={frame.get('frameKinds')}", file=sys.stderr)
PY
  ok "token-usage propagation"

  # ── 3. second delegation round trip on the same daemon ───────────────────
  scenario "a fresh owner gets a second reply from the same running daemon"
  run_owner second --endpoint "$SIGNAL_URL" --to "$WORKER_ID" \
    --task "emit the marker for SECOND-$$" --task-id "second-$$" --timeout-ms 180000
  [ "$OWNER_RC" = "0" ] || fail "second owner exited $OWNER_RC (expected 0)"
  "$PYTHON_BIN" - "$RUN_DIR/second.json" <<'PY' || fail "second round-trip assertion failed"
import json, sys
frame = json.load(open(sys.argv[1]))
assert frame.get("kind") == "Reply", f"kind={frame.get('kind')!r}"
assert "COORDINATION_OK" in (frame.get("text") or ""), "second reply missing marker"
print(f"[e2e]   second reply ownerId={frame.get('ownerId')!r}", file=sys.stderr)
PY
  assert_bidirectional_delivery "$RUN_DIR/second.json"
  ok "second delegation round trip"

  # ── 4. unavailable-provider error path ───────────────────────────────────
  scenario "a task requesting an unavailable provider returns an Error frame"
  run_owner badprov --endpoint "$SIGNAL_URL" --to "$WORKER_ID" \
    --provider claude --task "run this" --task-id "badprov-$$" --timeout-ms 120000
  # The owner exits non-zero on an Error terminal frame — expected here.
  [ "$OWNER_RC" != "0" ] || fail "bad-provider owner exited 0 (expected non-zero on Error)"
  "$PYTHON_BIN" - "$RUN_DIR/badprov.json" <<'PY' || fail "bad-provider assertion failed"
import json, sys
frame = json.load(open(sys.argv[1]))
assert frame.get("kind") == "Error", f"kind={frame.get('kind')!r} (expected Error)"
text = frame.get("text") or ""
assert "no available provider" in text, f"unexpected error text: {text!r}"
print(f"[e2e]   error: {text!r}", file=sys.stderr)
PY
  ok "unavailable-provider error path"

  teardown_stack
}

# ── group B: custom marker on a fresh stack ─────────────────────────────────
group_custom_marker() {
  local marker="CUSTOM_MARKER_$$_$RANDOM"
  SESSION="medulla-e2e-$$-b"
  RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/medulla-e2e-XXXXXX")"
  e2e_init
  boot_signal
  boot_llm "export MOCK_LLM_MARKER=$(printf %q "$marker")"
  boot_daemon

  scenario "a custom MOCK_LLM_MARKER flows the full chain end to end"
  run_owner marker --endpoint "$SIGNAL_URL" --to "$WORKER_ID" \
    --task "emit the marker for MARKER-$$" --task-id "marker-$$" --timeout-ms 180000
  [ "$OWNER_RC" = "0" ] || fail "custom-marker owner exited $OWNER_RC (expected 0)"
  "$PYTHON_BIN" - "$RUN_DIR/marker.json" "$marker" <<'PY' || fail "custom-marker assertion failed"
import json, sys
frame = json.load(open(sys.argv[1]))
marker = sys.argv[2]
assert frame.get("kind") == "Reply", f"kind={frame.get('kind')!r}"
text = frame.get("text") or ""
assert marker in text, f"reply missing custom marker {marker!r}: {text!r}"
assert "COORDINATION_OK" not in text, f"default marker leaked through: {text!r}"
print(f"[e2e]   custom marker present: {text[:80]!r}", file=sys.stderr)
PY
  ok "custom marker end to end"

  teardown_stack
}

main() {
  local started; started=$(date +%s)
  group_shared_stack
  group_custom_marker
  local elapsed=$(( $(date +%s) - started ))

  log ""
  log "═══════════════════════════════════════════════"
  printf '\n[e2e] PASS: all %d scenarios green (%ds):\n' "${#PASSED[@]}" "$elapsed" >&2
  local s
  for s in "${PASSED[@]}"; do printf '[e2e]   ✓ %s\n' "$s" >&2; done
}

main "$@"

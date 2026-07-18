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
# Prebuilt-binary overrides (used by the Docker path so the image bakes them in):
#   MEDULLA_BIN         path to the `medulla` binary           (default: cargo build)
#   MOCK_SIGNAL_BIN     path to the mock_signal_server example (default: cargo build)
#   OWNER_BIN           path to the coordination_owner example (default: cargo build)
#   OPENCODE_BIN        path to the `opencode` CLI             (default: PATH / ~/.opencode)
#   E2E_KEEP=1          keep the run dir + tmux session on exit (debugging)
#   E2E_SMOKE=0         skip the interactive opencode TUI smoke leg
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
SESSION="medulla-e2e-$$"
RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/medulla-e2e-XXXXXX")"
OWNER_TASK="emit the coordination marker E2E-$$-$RANDOM"

log()  { printf '[e2e] %s\n' "$*" >&2; }
fail() { printf '[e2e] FAIL: %s\n' "$*" >&2; dump_diagnostics; exit 1; }

cleanup() {
  local rc=$?
  if [ "${E2E_KEEP:-0}" = "1" ]; then
    log "E2E_KEEP=1 — leaving session $SESSION and run dir $RUN_DIR"
  else
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    rm -rf "$RUN_DIR" 2>/dev/null || true
  fi
  return $rc
}
trap cleanup EXIT

dump_diagnostics() {
  printf '\n[e2e] ===== DIAGNOSTICS =====\n' >&2
  for name in signal llm daemon owner smoke; do
    local f="$RUN_DIR/$name.log"
    if [ -f "$f" ]; then
      printf '\n----- %s.log -----\n' "$name" >&2
      tail -n 60 "$f" >&2 || true
    fi
  done
  [ -f "$RUN_DIR/owner.json" ]  && { printf '\n----- owner.json -----\n' >&2; cat "$RUN_DIR/owner.json" >&2 || true; }
  [ -f "$RUN_DIR/llm.jsonl" ]   && { printf '\n----- llm.jsonl (last 5) -----\n' >&2; tail -n 5 "$RUN_DIR/llm.jsonl" >&2 || true; }
  if tmux has-session -t "$SESSION" 2>/dev/null; then
    printf '\n----- tmux panes -----\n' >&2
    for w in signal llm daemon smoke owner; do
      if tmux list-windows -t "$SESSION" -F '#{window_name}' 2>/dev/null | grep -qx "$w"; then
        printf '\n### pane %s ###\n' "$w" >&2
        tmux capture-pane -p -t "$SESSION:$w" 2>/dev/null | grep -v '^[[:space:]]*$' | tail -n 40 >&2 || true
      fi
    done
  fi
  printf '\n[e2e] =======================\n' >&2
}

# Wait until FILE contains a line matching REGEX (extended), or TIMEOUT seconds.
wait_for_regex() {
  local file="$1" regex="$2" timeout="${3:-30}"
  local deadline=$(( $(date +%s) + timeout ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if [ -f "$file" ] && grep -Eq "$regex" "$file" 2>/dev/null; then return 0; fi
    sleep 0.3
  done
  return 1
}

resolve_binaries() {
  OPENCODE_BIN="${OPENCODE_BIN:-$(command -v opencode || true)}"
  if [ -z "$OPENCODE_BIN" ] && [ -x "$HOME/.opencode/bin/opencode" ]; then
    OPENCODE_BIN="$HOME/.opencode/bin/opencode"
  fi
  [ -n "$OPENCODE_BIN" ] && [ -x "$OPENCODE_BIN" ] || fail "opencode CLI not found (set OPENCODE_BIN)"
  OPENCODE_DIR="$(cd "$(dirname "$OPENCODE_BIN")" && pwd)"
  log "opencode: $OPENCODE_BIN ($("$OPENCODE_BIN" --version 2>/dev/null | head -1))"

  if [ -z "${MEDULLA_BIN:-}" ] || [ -z "${MOCK_SIGNAL_BIN:-}" ] || [ -z "${OWNER_BIN:-}" ]; then
    log "building medulla + examples (release)…"
    ( cd "$SDK_DIR" && cargo build --release --bin medulla \
        --example mock_signal_server --example coordination_owner >&2 )
    MEDULLA_BIN="${MEDULLA_BIN:-$SDK_DIR/target/release/medulla}"
    MOCK_SIGNAL_BIN="${MOCK_SIGNAL_BIN:-$SDK_DIR/target/release/examples/mock_signal_server}"
    OWNER_BIN="${OWNER_BIN:-$SDK_DIR/target/release/examples/coordination_owner}"
  fi
  for b in "$MEDULLA_BIN" "$MOCK_SIGNAL_BIN" "$OWNER_BIN"; do
    [ -x "$b" ] || fail "missing binary: $b"
  done
  PYTHON_BIN="${PYTHON_BIN:-$(command -v python3)}"
  [ -n "$PYTHON_BIN" ] || fail "python3 not found"
}

# Launch a service in its own tmux window from a launcher script file. The BODY
# (passed on stdin) is the command(s); env exports are written above it. Output
# goes to RUN_DIR/NAME.log; the exit status lands in RUN_DIR/NAME.rc.
launch() {
  local name="$1"
  local script="$RUN_DIR/$name.cmd"
  {
    printf '#!/usr/bin/env bash\nset -uo pipefail\ncd %q\n' "$RUN_DIR"
    cat
  } > "$script"
  chmod +x "$script"
  tmux new-window -t "$SESSION" -n "$name" -c "$RUN_DIR"
  tmux send-keys -t "$SESSION:$name" \
    "bash $(printf %q "$script") > $(printf %q "$RUN_DIR/$name.log") 2>&1; echo \$? > $(printf %q "$RUN_DIR/$name.rc")" C-m
}

main() {
  command -v tmux >/dev/null || fail "tmux is required but not installed"
  resolve_binaries

  mkdir -p "$RUN_DIR"/{ochome,mhome,work,tp}
  log "run dir: $RUN_DIR"

  tmux new-session -d -s "$SESSION" -x 220 -y 50 -c "$RUN_DIR"
  tmux set-option -t "$SESSION" -g history-limit 20000 >/dev/null 2>&1 || true

  # ── 1. mock tiny.place Signal server (ephemeral loopback port) ────────────
  launch signal <<EOF
export MOCK_SIGNAL_PORT=0
exec $(printf %q "$MOCK_SIGNAL_BIN")
EOF
  wait_for_regex "$RUN_DIR/signal.log" 'listening on http://127\.0\.0\.1:[0-9]+' 30 \
    || fail "mock signal server did not start"
  SIGNAL_URL="$(grep -Eo 'http://127\.0\.0\.1:[0-9]+' "$RUN_DIR/signal.log" | head -1)"
  log "signal server: $SIGNAL_URL"

  # ── 2. mock OpenAI-compatible LLM (ephemeral loopback port) ───────────────
  launch llm <<EOF
export MOCK_LLM_PORT=0
export MOCK_LLM_LOG=$(printf %q "$RUN_DIR/llm.jsonl")
exec $(printf %q "$PYTHON_BIN") $(printf %q "$SCRIPT_DIR/mock_llm.py")
EOF
  wait_for_regex "$RUN_DIR/llm.log" 'listening on http://127\.0\.0\.1:[0-9]+' 30 \
    || fail "mock LLM did not start"
  LLM_PORT="$(grep -Eo '127\.0\.0\.1:[0-9]+' "$RUN_DIR/llm.log" | head -1 | cut -d: -f2)"
  log "mock LLM: 127.0.0.1:$LLM_PORT"

  # opencode config with the real mock-LLM port substituted in.
  OC_CONFIG="$RUN_DIR/opencode.json"
  sed "s/MOCK_LLM_PORT/$LLM_PORT/" "$SCRIPT_DIR/opencode.json" > "$OC_CONFIG"

  # ── 3. medulla daemon (spawns opencode as its provider) ───────────────────
  launch daemon <<EOF
export HOME=$(printf %q "$RUN_DIR/ochome")
export OPENCODE_CONFIG=$(printf %q "$OC_CONFIG")
export OPENCODE_DISABLE_AUTOUPDATE=1
export PATH=$(printf %q "$OPENCODE_DIR"):\$PATH
export TINYPLACE_ENDPOINT=$(printf %q "$SIGNAL_URL")
export TINYPLACE_CONFIG=$(printf %q "$RUN_DIR/tp/config.json")
export MEDULLA_HOME=$(printf %q "$RUN_DIR/mhome")
exec $(printf %q "$MEDULLA_BIN") daemon --providers opencode \
  --workspace $(printf %q "$RUN_DIR/work") --poll-ms 500
EOF
  wait_for_regex "$RUN_DIR/daemon.log" 'serving providers .* as .* on ' 90 \
    || fail "medulla daemon did not reach the serving state"
  WORKER_ID="$(grep -Eo 'as [^ ]+ on ' "$RUN_DIR/daemon.log" | head -1 | awk '{print $2}')"
  [ -n "$WORKER_ID" ] || fail "could not scrape worker agent id from daemon.log"
  log "daemon worker id: $WORKER_ID"

  # ── 4. interactive opencode TUI smoke leg (tmux drives opencode directly) ──
  if [ "${E2E_SMOKE:-1}" != "0" ]; then
    smoke_leg
  else
    log "smoke leg skipped (E2E_SMOKE=0)"
  fi

  # ── 5. owner driver: send task, drain for the encrypted reply ─────────────
  launch owner <<EOF
exec $(printf %q "$OWNER_BIN") --endpoint $(printf %q "$SIGNAL_URL") \
  --to $(printf %q "$WORKER_ID") --task $(printf %q "$OWNER_TASK") \
  --task-id coord-$$ --timeout-ms 180000
EOF
  wait_for_regex "$RUN_DIR/owner.rc" '.' 200 || fail "owner driver did not finish in time"
  local owner_rc; owner_rc="$(cat "$RUN_DIR/owner.rc")"
  # The owner prints its terminal frame JSON to stdout (captured in owner.log).
  grep -E '^\{.*"kind"' "$RUN_DIR/owner.log" | tail -1 > "$RUN_DIR/owner.json" || true

  assert_all "$owner_rc"
}

# Drive a real interactive opencode TUI in its own tmux pane against the mock LLM.
smoke_leg() {
  log "smoke leg: driving interactive opencode TUI…"
  # A launcher so the TUI inherits the mock-LLM env; it paints to the pane (a real
  # PTY), which we scrape with capture-pane.
  cat > "$RUN_DIR/smoke.cmd" <<EOF
#!/usr/bin/env bash
export HOME=$(printf %q "$RUN_DIR/ochome")
export OPENCODE_CONFIG=$(printf %q "$OC_CONFIG")
export OPENCODE_DISABLE_AUTOUPDATE=1
export PATH=$(printf %q "$OPENCODE_DIR"):\$PATH
cd $(printf %q "$RUN_DIR/work")
exec $(printf %q "$OPENCODE_BIN")
EOF
  chmod +x "$RUN_DIR/smoke.cmd"
  tmux new-window -t "$SESSION" -n smoke -c "$RUN_DIR/work"
  tmux send-keys -t "$SESSION:smoke" "bash $(printf %q "$RUN_DIR/smoke.cmd")" C-m

  # Wait for the editor to actually paint its prompt ("Ask anything...") rather
  # than a fixed sleep; dismiss any blocking dialog (e.g. an auto-update
  # "Please restart" prompt) that steals focus before the editor is ready.
  local ready=0 ready_deadline=$(( $(date +%s) + 60 )) pane
  while [ "$(date +%s)" -lt "$ready_deadline" ]; do
    pane="$(tmux capture-pane -p -t "$SESSION:smoke" 2>/dev/null || true)"
    if printf '%s' "$pane" | grep -Eqi 'update complete|please restart|restart the application'; then
      log "smoke leg: dismissing unexpected dialog"
      tmux send-keys -t "$SESSION:smoke" Enter 2>/dev/null || true
      sleep 1
      continue
    fi
    if printf '%s' "$pane" | grep -q 'Ask anything'; then ready=1; break; fi
    sleep 1
  done
  [ "$ready" = "1" ] || fail "smoke leg: opencode editor never became ready"

  tmux send-keys -t "$SESSION:smoke" "reply with the marker for SMOKE-$$"
  sleep 1
  tmux send-keys -t "$SESSION:smoke" Enter
  local deadline=$(( $(date +%s) + 120 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if tmux capture-pane -p -t "$SESSION:smoke" 2>/dev/null | grep -q 'COORDINATION_OK'; then
      log "smoke leg: opencode TUI rendered COORDINATION_OK"
      tmux capture-pane -p -t "$SESSION:smoke" 2>/dev/null > "$RUN_DIR/smoke.log" || true
      tmux send-keys -t "$SESSION:smoke" C-c 2>/dev/null || true
      return 0
    fi
    sleep 2
  done
  tmux capture-pane -p -t "$SESSION:smoke" 2>/dev/null > "$RUN_DIR/smoke.log" || true
  fail "smoke leg: opencode TUI never rendered COORDINATION_OK"
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
  local to_worker to_owner owner_id
  owner_id="$(grep -Eo '"ownerId"[[:space:]]*:[[:space:]]*"[^"]+"' "$RUN_DIR/owner.json" \
    | sed -E 's/.*"ownerId"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
  [ -n "$owner_id" ] || fail "could not read ownerId from owner.json"
  to_worker="$(curl -s "$SIGNAL_URL/debug/stored?to=$WORKER_ID" | "$PYTHON_BIN" -c 'import sys,json;print(json.load(sys.stdin)["count"])')"
  to_owner="$(curl -s "$SIGNAL_URL/debug/stored?to=$owner_id"  | "$PYTHON_BIN" -c 'import sys,json;print(json.load(sys.stdin)["count"])')"
  [ "${to_worker:-0}" -ge 1 ] || fail "no envelopes stored for the worker (owner→worker leg)"
  [ "${to_owner:-0}" -ge 1 ]  || fail "no envelopes stored for the owner (worker→owner leg)"
  log "  (c) envelopes: owner→worker=$to_worker  worker→owner=$to_owner"

  printf '\n[e2e] PASS: coordination round trip green — owner=Reply(COORDINATION_OK) via real opencode, LLM saw the task, bidirectional encrypted delivery confirmed.\n' >&2
}

main "$@"

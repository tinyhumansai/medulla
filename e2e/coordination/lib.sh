#!/usr/bin/env bash
# Shared boot/teardown/assertion helpers for the medulla coordination e2e harness.
#
# Both the happy-path driver (`run.sh`) and the multi-scenario suite (`tests.sh`)
# source this file. It owns the real-process stack under tmux:
#
#   mock tiny.place Signal server → `medulla daemon --providers opencode`
#     → real `opencode` CLI → mock OpenAI-compatible LLM
#
# Callers set SCRIPT_DIR + SDK_DIR (this file lives next to run.sh/tests.sh),
# then call `e2e_init`, `boot_signal`, `boot_llm`, `boot_daemon`, run owner legs
# via `run_owner`, and rely on the EXIT trap installed here for cleanup.
#
# All state lands in the shared globals RUN_DIR / SESSION / SIGNAL_URL /
# LLM_PORT / OC_CONFIG / WORKER_ID. Loopback only; deterministic; no real keys.

# ── logging + diagnostics ───────────────────────────────────────────────────
log()  { printf '[e2e] %s\n' "$*" >&2; }
fail() { printf '[e2e] FAIL: %s\n' "$*" >&2; dump_diagnostics; exit 1; }

dump_diagnostics() {
  printf '\n[e2e] ===== DIAGNOSTICS =====\n' >&2
  for f in "$RUN_DIR"/*.log; do
    [ -f "$f" ] || continue
    printf '\n----- %s -----\n' "$(basename "$f")" >&2
    tail -n 60 "$f" >&2 || true
  done
  for j in "$RUN_DIR"/*.json; do
    [ -f "$j" ] || continue
    printf '\n----- %s -----\n' "$(basename "$j")" >&2
    cat "$j" >&2 || true
  done
  [ -f "$RUN_DIR/llm.jsonl" ] && { printf '\n----- llm.jsonl (last 5) -----\n' >&2; tail -n 5 "$RUN_DIR/llm.jsonl" >&2 || true; }
  if tmux has-session -t "$SESSION" 2>/dev/null; then
    printf '\n----- tmux panes -----\n' >&2
    for w in $(tmux list-windows -t "$SESSION" -F '#{window_name}' 2>/dev/null); do
      printf '\n### pane %s ###\n' "$w" >&2
      tmux capture-pane -p -t "$SESSION:$w" 2>/dev/null | grep -v '^[[:space:]]*$' | tail -n 40 >&2 || true
    done
  fi
  printf '\n[e2e] =======================\n' >&2
}

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

# ── binary resolution ───────────────────────────────────────────────────────
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

# ── stack lifecycle ─────────────────────────────────────────────────────────
# Create the run dir + tmux session and install the cleanup trap. Callers must
# have set SESSION and RUN_DIR before sourcing/using; e2e_init derives sane
# defaults when unset.
e2e_init() {
  command -v tmux >/dev/null || fail "tmux is required but not installed"
  : "${SESSION:=medulla-e2e-$$}"
  : "${RUN_DIR:=$(mktemp -d "${TMPDIR:-/tmp}/medulla-e2e-XXXXXX")}"
  trap cleanup EXIT
  resolve_binaries
  mkdir -p "$RUN_DIR"/{ochome,mhome,work,tp}
  # opencode's snapshot feature misbehaves in a non-git cwd (upstream #31382):
  # `opencode run` can produce no output at all. The config sets snapshot:false;
  # a git repo in the workdir is belt-and-braces for when git is present.
  if command -v git >/dev/null 2>&1; then
    git -C "$RUN_DIR/work" init -q 2>/dev/null || true
  fi
  log "run dir: $RUN_DIR"
  tmux new-session -d -s "$SESSION" -x 220 -y 50 -c "$RUN_DIR"
  tmux set-option -t "$SESSION" -g history-limit 20000 >/dev/null 2>&1 || true
}

# Launch a service in its own tmux window from a launcher script file. The BODY
# (passed on stdin) is the command(s). Output → RUN_DIR/NAME.log; exit status →
# RUN_DIR/NAME.rc.
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

# Boot the mock tiny.place Signal server; sets SIGNAL_URL.
boot_signal() {
  launch signal <<EOF
export MOCK_SIGNAL_PORT=0
exec $(printf %q "$MOCK_SIGNAL_BIN")
EOF
  wait_for_regex "$RUN_DIR/signal.log" 'listening on http://127\.0\.0\.1:[0-9]+' 30 \
    || fail "mock signal server did not start"
  SIGNAL_URL="$(grep -Eo 'http://127\.0\.0\.1:[0-9]+' "$RUN_DIR/signal.log" | head -1)"
  log "signal server: $SIGNAL_URL"
}

# Boot the mock LLM; sets LLM_PORT and writes OC_CONFIG. Any extra args are
# emitted as `export` lines into the launcher (e.g. MOCK_LLM_MARKER=...).
boot_llm() {
  local extra_env="$1"
  launch llm <<EOF
export MOCK_LLM_PORT=0
export MOCK_LLM_LOG=$(printf %q "$RUN_DIR/llm.jsonl")
$extra_env
exec $(printf %q "$PYTHON_BIN") $(printf %q "$SCRIPT_DIR/mock_llm.py")
EOF
  wait_for_regex "$RUN_DIR/llm.log" 'listening on http://127\.0\.0\.1:[0-9]+' 30 \
    || fail "mock LLM did not start"
  LLM_PORT="$(grep -Eo '127\.0\.0\.1:[0-9]+' "$RUN_DIR/llm.log" | head -1 | cut -d: -f2)"
  log "mock LLM: 127.0.0.1:$LLM_PORT"
  OC_CONFIG="$RUN_DIR/opencode.json"
  sed "s/MOCK_LLM_PORT/$LLM_PORT/" "$SCRIPT_DIR/opencode.json" > "$OC_CONFIG"
}

# Boot the medulla daemon; sets WORKER_ID. Extra daemon flags are appended.
boot_daemon() {
  local extra_flags="${1:-}"
  launch daemon <<EOF
export HOME=$(printf %q "$RUN_DIR/ochome")
export OPENCODE_CONFIG=$(printf %q "$OC_CONFIG")
export OPENCODE_DISABLE_AUTOUPDATE=1
export PATH=$(printf %q "$OPENCODE_DIR"):\$PATH
export TINYPLACE_ENDPOINT=$(printf %q "$SIGNAL_URL")
export TINYPLACE_CONFIG=$(printf %q "$RUN_DIR/tp/config.json")
export MEDULLA_HOME=$(printf %q "$RUN_DIR/mhome")
exec $(printf %q "$MEDULLA_BIN") daemon --providers opencode \
  --workspace $(printf %q "$RUN_DIR/work") --poll-ms 500 $extra_flags
EOF
  wait_for_regex "$RUN_DIR/daemon.log" 'serving providers .* as .* on ' 90 \
    || fail "medulla daemon did not reach the serving state"
  WORKER_ID="$(grep -Eo 'as [^ ]+ on ' "$RUN_DIR/daemon.log" | head -1 | awk '{print $2}')"
  [ -n "$WORKER_ID" ] || fail "could not scrape worker agent id from daemon.log"
  log "daemon worker id: $WORKER_ID"
}

# Run an owner leg in its own tmux window. Usage:
#   run_owner <label> <owner-arg>...
# Waits for completion, writes the terminal frame JSON to RUN_DIR/<label>.json,
# and sets OWNER_RC to the owner exit code. Never fails the suite itself (the
# caller asserts on the JSON / rc), so error-path scenarios can inspect it.
run_owner() {
  local label="$1"; shift
  local args=""
  local a
  for a in "$@"; do args+=" $(printf %q "$a")"; done
  launch "$label" <<EOF
exec $(printf %q "$OWNER_BIN")$args
EOF
  wait_for_regex "$RUN_DIR/$label.rc" '.' 220 || fail "owner leg '$label' did not finish in time"
  OWNER_RC="$(cat "$RUN_DIR/$label.rc")"
  grep -E '^\{.*"kind"' "$RUN_DIR/$label.log" | tail -1 > "$RUN_DIR/$label.json" || true
}

# Confirm bidirectional encrypted delivery via the Signal /debug/stored surface.
assert_bidirectional_delivery() {
  local json="$1"
  local owner_id to_worker to_owner
  owner_id="$(grep -Eo '"ownerId"[[:space:]]*:[[:space:]]*"[^"]+"' "$json" \
    | sed -E 's/.*"ownerId"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
  [ -n "$owner_id" ] || fail "could not read ownerId from $json"
  to_worker="$(curl -s "$SIGNAL_URL/debug/stored?to=$WORKER_ID" | "$PYTHON_BIN" -c 'import sys,json;print(json.load(sys.stdin)["count"])')"
  to_owner="$(curl -s "$SIGNAL_URL/debug/stored?to=$owner_id"  | "$PYTHON_BIN" -c 'import sys,json;print(json.load(sys.stdin)["count"])')"
  [ "${to_worker:-0}" -ge 1 ] || fail "no envelopes stored for the worker (owner→worker leg)"
  [ "${to_owner:-0}" -ge 1 ]  || fail "no envelopes stored for the owner (worker→owner leg)"
  log "  envelopes: owner→worker=$to_worker  worker→owner=$to_owner"
}

# Drive a real interactive opencode TUI in its own tmux pane against the mock
# LLM, proving tmux controls opencode as well as medulla.
smoke_leg() {
  log "smoke leg: driving interactive opencode TUI…"
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

#!/usr/bin/env bash
# Shared boot/teardown/assertion helpers for the medulla coordination e2e harness.
#
# Both the happy-path driver (`run.sh`) and the scenario suites (`tests.sh`,
# `tests_multi.sh`) source this file. It owns the real-process stack under tmux:
#
#   mock link forwarder (blind UDP, docs/host-link-protocol.md §5)
#     → owner driver (long-lived, one per enrolled pair)
#     → `medulla daemon --providers opencode`
#       → real `opencode` CLI → mock OpenAI-compatible LLM
#
# Callers set SCRIPT_DIR + SDK_DIR (this file lives next to run.sh/tests.sh),
# then call `e2e_init`, `boot_forwarder <name>…`, `boot_llm`, `boot_daemon`, run
# owner legs via `run_owner`, and rely on the EXIT trap installed here.
#
# Enrollment (protocol §7) happens up front, before anything boots: for each
# name, `coordination_owner --enroll` mints one orchestrator/host pair — a pair
# key, two node ids and two forwarder keys — and writes a `node.json` for each
# end. The forwarder is then seeded with the two *forwarder* keys, which is all a
# blind forwarder is ever given; it never sees a pair key and cannot read a
# payload.
#
# All state lands in the shared globals RUN_DIR / SESSION / FORWARDER_ADDR /
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
    # opencode.json carries a provider API key — a real one under run-live.sh.
    # Diagnostics land in CI logs and pasted bug reports, so redact any value
    # that looks like a credential rather than trusting the file not to hold one.
    sed -E 's/("(apiKey|api_key|token|secret)"[[:space:]]*:[[:space:]]*")[^"]*"/\1<redacted>"/g' \
      "$j" >&2 || true
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

  if [ -z "${MEDULLA_BIN:-}" ] || [ -z "${FORWARDER_BIN:-}" ] || [ -z "${OWNER_BIN:-}" ]; then
    log "building medulla + examples (release)…"
    ( cd "$SDK_DIR" && cargo build --release --bin medulla \
        --example mock_link_forwarder --example coordination_owner >&2 )
    MEDULLA_BIN="${MEDULLA_BIN:-$SDK_DIR/target/release/medulla}"
    FORWARDER_BIN="${FORWARDER_BIN:-$SDK_DIR/target/release/examples/mock_link_forwarder}"
    OWNER_BIN="${OWNER_BIN:-$SDK_DIR/target/release/examples/coordination_owner}"
  fi
  for b in "$MEDULLA_BIN" "$FORWARDER_BIN" "$OWNER_BIN"; do
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
  mkdir -p "$RUN_DIR"/{ochome,work}
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

# Create an additional isolated workspace for a second (third, …) daemon.
# Usage: make_workspace <name> [sentinel]
#
# Each daemon owns a private workspace directory, so multi-daemon scenarios can
# assert that a daemon only ever reports and reads its own directory. When a
# SENTINEL is given it is written into the workspace's AGENTS.md — the daemon's
# `dir_context` reader picks that file up to ground its capabilities probe, so
# the sentinel shows up in that daemon's LLM requests and nowhere else. That is
# what makes "this agent is bound to this dir" an assertion rather than a hope.
#
# Echoes the workspace path.
make_workspace() {
  local name="$1" sentinel="${2:-}"
  local dir="$RUN_DIR/work-$name"
  mkdir -p "$dir"
  if command -v git >/dev/null 2>&1; then
    git -C "$dir" init -q 2>/dev/null || true
  fi
  if [ -n "$sentinel" ]; then
    cat > "$dir/AGENTS.md" <<EOF
# Workspace $name

This workspace is identified by the sentinel token $sentinel.
When asked to describe this directory, always mention $sentinel.
EOF
  fi
  printf '%s' "$dir"
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

# Echo a free UDP port on loopback.
#
# The forwarder's address has to be known *before* it starts, because it is
# written into every node.json at enrollment (protocol §7.3) and the daemon has
# no flag to override it. So the port is picked here, released, and rebound by
# the forwarder a moment later; if something takes it in between, the forwarder
# fails to bind and the run stops loudly rather than pointing endpoints at
# nothing.
free_port() {
  "$PYTHON_BIN" - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

# Enroll one orchestrator/host pair for NAME (protocol §7).
#
# Writes `$RUN_DIR/mhome-<name>/e2e/link/node.json` (the host, which the daemon
# opens) and `$RUN_DIR/owner-<name>/link/node.json` (its orchestrator), and sets:
#
# `MEDULLA_HOME` names the install *root*, and a medulla home is
# `<root>/<account>` — so the host identity goes under the account the daemon is
# launched with (`MEDULLA_USER=e2e`), not directly under the root.
#
#   HOST_NODE_ID_<name>   the worker's wire id — what an owner leg's --to takes
#   OWNER_NODE_ID_<name>  the orchestrator's wire id
#   OWNER_FOR_<host id>   reverse lookup used by start_owner to pick a queue
#   HOST_FOR_<owner id>   reverse lookup used by the delivery assertion
#   FORWARDER_NODES       accumulated --node specs for the forwarder's table
enroll_pair() {
  local name="$1" out
  mkdir -p "$RUN_DIR/mhome-$name/e2e" "$RUN_DIR/owner-$name" "$RUN_DIR/queue-$name"
  out="$("$OWNER_BIN" --enroll \
    --state-dir "$RUN_DIR/owner-$name/link" \
    --host-state-dir "$RUN_DIR/mhome-$name/e2e/link" \
    --forwarder "$FORWARDER_ADDR" 2>"$RUN_DIR/enroll-$name.log")" \
    || fail "enrollment for '$name' failed (see enroll-$name.log)"

  local owner_id owner_key host_id host_key
  owner_id="$(printf '%s\n' "$out" | sed -n 's/^OWNER_NODE_ID=//p')"
  owner_key="$(printf '%s\n' "$out" | sed -n 's/^OWNER_FORWARDER_KEY=//p')"
  host_id="$(printf '%s\n' "$out" | sed -n 's/^HOST_NODE_ID=//p')"
  host_key="$(printf '%s\n' "$out" | sed -n 's/^HOST_FORWARDER_KEY=//p')"
  [ -n "$owner_id" ] && [ -n "$host_id" ] || fail "enrollment for '$name' printed no node ids"

  printf -v "HOST_NODE_ID_$name" '%s' "$host_id"
  printf -v "OWNER_NODE_ID_$name" '%s' "$owner_id"
  printf -v "OWNER_FOR_$host_id" '%s' "$name"
  printf -v "HOST_FOR_$owner_id" '%s' "$host_id"
  # One team for the whole harness: §5 rule 6's cross-team drop is a forwarder
  # unit test, not something a green fleet run should be exercising.
  FORWARDER_NODES+=("--node" "$owner_id:$owner_key:e2e" "--node" "$host_id:$host_key:e2e")
  log "enrolled '$name': host=$host_id owner=$owner_id"
}

# Boot the mock link forwarder and one owner driver per enrolled pair.
#   boot_forwarder <name>…
#
# Sets FORWARDER_ADDR. Every name given is enrolled first (the forwarder's node
# table is fixed at boot), then the forwarder starts, then one long-lived
# `coordination_owner --serve` per name.
boot_forwarder() {
  local port name
  port="$(free_port)"
  FORWARDER_ADDR="127.0.0.1:$port"
  FORWARDER_NODES=()
  for name in "$@"; do enroll_pair "$name"; done

  local args="" a
  for a in "${FORWARDER_NODES[@]}"; do args+=" $(printf %q "$a")"; done
  launch forwarder <<EOF
exec $(printf %q "$FORWARDER_BIN") --bind $(printf %q "$FORWARDER_ADDR")$args
EOF
  wait_for_regex "$RUN_DIR/forwarder.log" 'listening on 127\.0\.0\.1:[0-9]+' 30 \
    || fail "mock link forwarder did not start on $FORWARDER_ADDR"
  log "forwarder: $FORWARDER_ADDR"

  for name in "$@"; do boot_owner "$name"; done
}

# Boot the long-lived owner driver for NAME.
#
# One process per enrolled pair, kept up for the whole suite. That is a
# requirement, not a shortcut: SSP state lives in memory, so an owner that exited
# between legs would come back at state 0 while the daemon still held state n,
# and neither side's diffs would apply again (see the example's module docs).
boot_owner() {
  local name="$1"
  launch "owner-$name" <<EOF
exec $(printf %q "$OWNER_BIN") \
  --state-dir $(printf %q "$RUN_DIR/owner-$name/link") \
  --forwarder $(printf %q "$FORWARDER_ADDR") \
  --serve $(printf %q "$RUN_DIR/queue-$name") \
  --results $(printf %q "$RUN_DIR")
EOF
  wait_for_regex "$RUN_DIR/owner-$name.log" 'coordination_owner serving ' 30 \
    || fail "owner driver for '$name' did not start"
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

# Boot one medulla daemon under a caller-chosen NAME.
#   boot_daemon_named <name> <workspace-dir> [extra-daemon-flags]
#
# NAME must be a bash-identifier-safe token and must already be enrolled (see
# `boot_forwarder`); it names the tmux window, the log file, and the
# `WORKER_NAME_<NAME>` global holding the label the daemon advertises. Each
# daemon gets its own MEDULLA_HOME — which is where its link identity lives, so
# each is a *distinct enrolled host* — plus its own opencode HOME.
#
# The mock LLM and the forwarder are shared: those are the fixtures under test.
boot_daemon_named() {
  local name="$1" workspace="$2" extra_flags="${3:-}"
  local host_var="HOST_NODE_ID_$name"
  [ -n "${!host_var:-}" ] || fail "daemon '$name' was never enrolled — call boot_forwarder $name"
  mkdir -p "$RUN_DIR/ochome-$name"
  launch "$name" <<EOF
export HOME=$(printf %q "$RUN_DIR/ochome-$name")
export OPENCODE_CONFIG=$(printf %q "$OC_CONFIG")
export OPENCODE_DISABLE_AUTOUPDATE=1
export PATH=$(printf %q "$OPENCODE_DIR"):\$PATH
export MEDULLA_HOME=$(printf %q "$RUN_DIR/mhome-$name")
export MEDULLA_USER=e2e
export MEDULLA_LINK_OWNER=$(printf %q "orchestrator-$name")
exec $(printf %q "$MEDULLA_BIN") daemon --providers opencode --no-pair \
  --name $(printf %q "worker-$name") \
  --workspace $(printf %q "$workspace") --poll-ms 500 $extra_flags
EOF
  wait_for_regex "$RUN_DIR/$name.log" 'serving providers .* as .* for ' 90 \
    || fail "medulla daemon '$name' did not reach the serving state"
  local label
  label="$(grep -Eo 'as [^ ]+ for ' "$RUN_DIR/$name.log" | head -1 | awk '{print $2}')"
  [ -n "$label" ] || fail "could not scrape the advertised worker name from $name.log"
  printf -v "WORKER_NAME_$name" '%s' "$label"
  log "daemon '$name' node=${!host_var} advertises '$label'  workspace: $workspace"
}

# Echo the worker *node id* of the pair enrolled for NAME — what an owner leg
# addresses with --to (protocol §2: only the id ever travels on the wire).
worker_id() {
  local var="HOST_NODE_ID_$1"
  printf '%s' "${!var:-}"
}

# Echo the name the daemon advertises for NAME (scraped from its serving line).
worker_name() {
  local var="WORKER_NAME_$1"
  printf '%s' "${!var:-}"
}

# Boot the single default daemon in $RUN_DIR/work; sets WORKER_ID.
# Back-compat wrapper over boot_daemon_named for the single-daemon callers
# (run.sh, tests.sh) that predate multi-daemon support.
boot_daemon() {
  local extra_flags="${1:-}"
  boot_daemon_named daemon "$RUN_DIR/work" "$extra_flags"
  WORKER_ID="$(worker_id daemon)"
}

# Echo the request-queue directory of the owner enrolled with a leg's `--to`.
queue_for_leg() {
  local to="" a prev=""
  for a in "$@"; do
    if [ "$prev" = "--to" ]; then to="$a"; break; fi
    prev="$a"
  done
  [ -n "$to" ] || fail "owner leg has no --to <worker node id>"
  local var="OWNER_FOR_$to"
  local name="${!var:-}"
  [ -n "$name" ] || fail "no enrolled owner for worker node $to"
  printf '%s' "$RUN_DIR/queue-$name"
}

# Start an owner leg and return immediately. Usage:
#   start_owner <label> <owner-arg>...
#
# The leg is a request file dropped into the queue of the owner driver enrolled
# with the `--to` worker: one argument per line, written under a temporary name
# and renamed into place so the driver never reads a half-written request. Pair
# with `await_owner <label>`. Splitting start from await is what lets a scenario
# run legs against several workers concurrently, or interfere with the stack
# (e.g. kill a daemon) while a leg is still in flight.
start_owner() {
  local label="$1"; shift
  local queue; queue="$(queue_for_leg "$@")"
  rm -f "$RUN_DIR/$label.rc" "$RUN_DIR/$label.json"
  printf '%s\n' "$@" > "$queue/$label.pending"
  mv "$queue/$label.pending" "$queue/$label.req"
}

# Ask the owner driver enrolled for NAME to rebuild its link, dispatching
# nothing.
#
# Call this between killing a host and starting its replacement. The restarted
# host comes back with empty SSP state, and a frame the old session was still
# retransmitting would land on it and leave the two ends numbering from different
# origins — so the orchestrator's side is torn down while nothing is listening.
reset_owner_link() {
  local name="$1"
  local label="reset-$name-$RANDOM"
  rm -f "$RUN_DIR/$label.rc"
  printf '%s\n' --reset-only > "$RUN_DIR/queue-$name/$label.pending"
  mv "$RUN_DIR/queue-$name/$label.pending" "$RUN_DIR/queue-$name/$label.req"
  wait_for_regex "$RUN_DIR/$label.rc" '.' 60 || fail "owner '$name' did not rebuild its link"
  log "owner '$name' rebuilt its link"
}

# Wait for a started owner leg to finish. The driver writes RUN_DIR/<label>.json
# (the terminal frame) before RUN_DIR/<label>.rc (the exit code), so an rc that
# exists means both are complete. Sets OWNER_RC. Never fails the suite itself
# (the caller asserts on the JSON / rc), so error-path scenarios can inspect the
# outcome. Pass a TIMEOUT (seconds, default 220) to bound the wait.
await_owner() {
  local label="$1" timeout="${2:-220}"
  wait_for_regex "$RUN_DIR/$label.rc" '.' "$timeout" \
    || fail "owner leg '$label' did not finish within ${timeout}s"
  OWNER_RC="$(tr -d '[:space:]' < "$RUN_DIR/$label.rc")"
}

# Wait for an owner leg, tolerating non-completion. Sets OWNER_RC to the exit
# code, or the empty string if the leg was still running at the deadline. Used by
# the failure scenarios, where "never finished" is itself a valid outcome.
await_owner_maybe() {
  local label="$1" timeout="${2:-60}"
  if wait_for_regex "$RUN_DIR/$label.rc" '.' "$timeout"; then
    OWNER_RC="$(tr -d '[:space:]' < "$RUN_DIR/$label.rc")"
  else
    # shellcheck disable=SC2034  # read by the sourcing scenario scripts
    OWNER_RC=""
  fi
}

# Run an owner leg to completion. Usage:
#   run_owner <label> <owner-arg>...
run_owner() {
  local label="$1"; shift
  start_owner "$label" "$@"
  await_owner "$label"
}

# Hard-kill a named daemon's tmux window, simulating an agent crash. The daemon
# holds no listening socket of its own, so killing the window is a faithful
# stand-in for the process dying: its side of the link stops answering and the
# forwarder's binding for it goes stale.
kill_daemon() {
  local name="$1"
  tmux kill-window -t "$SESSION:$name" 2>/dev/null || true
  log "killed daemon '$name'"
}

# Confirm delivery in BOTH directions across the forwarder, for the pair that
# produced a terminal-frame JSON.
#
# The forwarder logs one line per datagram it moved, carrying only what a blind
# forwarder is entitled to see: source, destination, sequence, size, and whether
# the datagram was a heartbeat. That is enough to prove traffic crossed in each
# direction and no more — it cannot tell a task frame from an acknowledgement,
# which is the property the whole design rests on.
assert_bidirectional_delivery() {
  local json="$1"
  local owner_id host_id to_worker to_owner
  owner_id="$(grep -Eo '"ownerId"[[:space:]]*:[[:space:]]*"[^"]+"' "$json" \
    | sed -E 's/.*"ownerId"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
  [ -n "$owner_id" ] || fail "could not read ownerId from $json"
  local var="HOST_FOR_$owner_id"
  host_id="${!var:-}"
  [ -n "$host_id" ] || fail "no enrolled host for owner node $owner_id"
  to_worker="$(grep -c "forward src=$owner_id dst=$host_id .*kind=state" "$RUN_DIR/forwarder.log" || true)"
  to_owner="$(grep -c "forward src=$host_id dst=$owner_id .*kind=state" "$RUN_DIR/forwarder.log" || true)"
  [ "${to_worker:-0}" -ge 1 ] || fail "no state datagrams forwarded owner→worker"
  [ "${to_owner:-0}" -ge 1 ]  || fail "no state datagrams forwarded worker→owner"
  log "  datagrams: owner→worker=$to_worker  worker→owner=$to_owner"
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

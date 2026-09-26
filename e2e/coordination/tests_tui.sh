#!/usr/bin/env bash
# Terminal-facing scenarios: how Medulla and a real coding CLI share a terminal.
#
# Every other suite drives Medulla headlessly — a task frame goes in, a reply
# frame comes out, and no terminal is involved. This one covers the surfaces an
# operator actually looks at, both of which are full-screen TUIs on a real
# pseudo-terminal:
#
#   `medulla <harness>`      the wrapper. It launches the real CLI in the
#                            operator's terminal exactly as if they had run it
#                            themselves, while tailing the harness's own
#                            transcript underneath and forwarding it to the
#                            orchestrator over the host link.
#   `medulla daemon --tui`   the operator screen: sessions, log, requests.
#
# These break in ways headless tests cannot see. A harness TUI refuses to start
# when its stdin is a pipe rather than a terminal (Codex says so outright:
# "stdin is not a terminal"), so the wrapper has to allocate a PTY, and whether
# it did is only observable by looking at what the terminal painted. Everything
# here therefore runs under tmux and asserts on captured panes.
#
# Scenarios:
#
#   1. wrapper transparency — `medulla <harness> --no-bridge` paints the real
#                             CLI's TUI, and a prompt typed into it is answered.
#                             Medulla is in the middle of that terminal and must
#                             be invisible in it.
#   2. wrapper bridging     — the same wrapper, this time enrolled on the link,
#                             forwards the session to its orchestrator: the blind
#                             forwarder shows datagrams leaving the wrapper's own
#                             node. (Claude and Codex only — the wrapper does not
#                             tail OpenCode's transcript; see wrapper/mod.rs.)
#   3. operator screen      — `medulla daemon --tui` paints its screen and keeps
#                             serving: a task dispatched while it is up is
#                             answered, so the screen is not a mode that stops
#                             the daemon doing its job.
#
# `$E2E_HARNESS` selects the CLI as everywhere else. Exit 0 iff every scenario
# passes.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

PASSED=()
scenario() { log ""; log "═══ SCENARIO: $1 ═══"; }
ok()       { PASSED+=("$1"); log "  ✓ SCENARIO PASS: $1"; }
skip()     { log "  ⊘ SCENARIO SKIP: $1"; }

SESSION="medulla-e2e-tui-$$"

# How long a full-screen TUI gets to paint its composer, and a turn to answer.
READY_TIMEOUT_S=90
ANSWER_TIMEOUT_S=120

main() {
  RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/medulla-e2e-XXXXXX")"
  e2e_init
  # Four enrolled pairs: the headless daemon the other suites use, the wrapper
  # session (a distinct link endpoint of its own), and two operator screens.
  # Screens cannot share MEDULLA_HOME with each other or the headless daemon.
  boot_forwarder daemon wrapper screen state
  boot_llm "export MOCK_LLM_TOOL_DELAY_MS=5000"
  boot_daemon

  scenario_wrapper_transparency
  scenario_wrapper_bridging
  scenario_operator_screen
  scenario_claude_session_states

  summarize
}

# Launch a wrapper session in its own tmux window and wait for the CLI's
# composer to appear.
#   launch_wrapper <window> <medulla-home> <extra-wrapper-flags>
#
# No pipe on the command: a pipeline would put something other than a terminal on
# the child's stdin, which is the very condition the wrapper's PTY exists to
# avoid — and Claude Code silently falls back to `--print` mode when it sees one,
# which would make this suite assert nothing at all.
launch_wrapper() {
  local window="$1" home="$2" extra="${3:-}"
  local work="$RUN_DIR/work-$window"
  mkdir -p "$work"
  harness_seed_home "$RUN_DIR/wraphome-$window" "$work"
  cat > "$RUN_DIR/$window.cmd" <<EOF
#!/usr/bin/env bash
$(harness_env "$RUN_DIR/wraphome-$window")
export MEDULLA_HOME=$(printf %q "$home")
export MEDULLA_USER=e2e
export MEDULLA_LINK_OWNER=$(printf %q "orchestrator-wrapper")
cd $(printf %q "$work")
$(harness_wrapper_launch "$extra")
EOF
  chmod +x "$RUN_DIR/$window.cmd"
  tmux new-window -t "$SESSION" -n "$window" -c "$work"
  tmux send-keys -t "$SESSION:$window" "bash $(printf %q "$RUN_DIR/$window.cmd")" C-m

  # An enrolled wrapper's first run opens Medulla's own worker-setup wizard —
  # name, owner, confirm — before it hands the terminal to the harness. That is a
  # second full-screen TUI in the same pane, and it is Medulla's, so answering it
  # here is part of what this suite covers rather than a nuisance to route
  # around. Every step's default is the one this stack configured, so Enter is
  # the whole interaction. WIZARD_STEPS records how many were answered.
  WIZARD_STEPS=0
  HOOK_PROMPTS=0
  local deadline=$(( $(date +%s) + READY_TIMEOUT_S )) pane
  while [ "$(date +%s)" -lt "$deadline" ]; do
    pane="$(tmux capture-pane -p -t "$SESSION:$window" 2>/dev/null || true)"
    if printf '%s' "$pane" | grep -Eq "$(harness_tui_ready_regex)"; then
      return 0
    fi
    if printf '%s' "$pane" | grep -Eq 'Step [0-9]+/[0-9]+|worker setup'; then
      tmux send-keys -t "$SESSION:$window" Enter
      WIZARD_STEPS=$(( WIZARD_STEPS + 1 ))
      sleep 2
      continue
    fi
    # Medulla injects its hooks into the harness, and Codex will not run hooks it
    # has not been told to trust — it asks, once, on the first run that sees new
    # ones. Answering "trust all" is what a host running Medulla's own hooks does;
    # declining would leave the session running with the hooks silently disabled.
    if printf '%s' "$pane" | grep -Eq 'Hooks need review|hooks are new or changed'; then
      HOOK_PROMPTS=$(( ${HOOK_PROMPTS:-0} + 1 ))
      select_menu "$window" 'Trust all'
      continue
    fi
    sleep 1
  done
  tmux capture-pane -p -t "$SESSION:$window" > "$RUN_DIR/$window.pane" 2>/dev/null || true
  fail "wrapper '$window': the $HARNESS composer never appeared"
}

# Type TEXT into a wrapper window and wait for the marker to be painted.
prompt_wrapper() {
  local window="$1" text="$2"
  tmux send-keys -t "$SESSION:$window" "$text"
  sleep 1
  tmux send-keys -t "$SESSION:$window" Enter
  local deadline=$(( $(date +%s) + ANSWER_TIMEOUT_S ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if tmux capture-pane -p -t "$SESSION:$window" 2>/dev/null | grep -q 'COORDINATION_OK'; then
      tmux capture-pane -p -t "$SESSION:$window" > "$RUN_DIR/$window.pane" 2>/dev/null || true
      return 0
    fi
    sleep 2
  done
  tmux capture-pane -p -t "$SESSION:$window" > "$RUN_DIR/$window.pane" 2>/dev/null || true
  return 1
}

# Boot the daemon that paints the operator screen.
#
# Deliberately not `boot_daemon_named`: that helper redirects the daemon's
# stdout into a log file, and a screen with a file on its stdout is not a screen.
# This one keeps the tmux pane as the daemon's terminal, which is the whole point
# of the scenario, and reads its readiness from what the pane painted rather than
# from a log line it will never write.
boot_screen_daemon() {
  local name="${1:-screen}" permission_mode="${2:-bypass}" daemon_flags
  local work="$RUN_DIR/work-$name"
  if [ "$permission_mode" = prompt ]; then
    # Peer-task daemons bypass permissions by default; this explicit negative
    # flag is what makes the watched state probe stop on Claude's real dialog.
    daemon_flags="$(harness_daemon_routing_flags) --no-skip-permissions"
  else
    daemon_flags="$(harness_daemon_flags)"
  fi
  mkdir -p "$work"
  harness_seed_home "$RUN_DIR/ochome-$name" "$work"
  cat > "$RUN_DIR/$name.cmd" <<EOF
#!/usr/bin/env bash
$(harness_env "$RUN_DIR/ochome-$name")
export MEDULLA_HOME=$(printf %q "$RUN_DIR/mhome-$name")
export MEDULLA_USER=e2e
export MEDULLA_LINK_OWNER=$(printf %q "orchestrator-$name")
exec $(printf %q "$MEDULLA_BIN") daemon --tui --providers $(printf %q "$HARNESS") --no-pair \\
  --name $(printf %q "worker-$name") --workspace $(printf %q "$work") --poll-ms 500 $daemon_flags
EOF
  chmod +x "$RUN_DIR/$name.cmd"
  tmux new-window -t "$SESSION" -n "$name" -c "$work"
  tmux send-keys -t "$SESSION:$name" "bash $(printf %q "$RUN_DIR/$name.cmd")" C-m

  # A first-run operator screen asks two questions before it becomes a screen:
  # how the worker should run tasks, and which coding agent powers it. Both are
  # menus, and answering them is part of what this scenario covers.
  #
  # Interactive mode is chosen where the harness supports it, because that is the
  # mode where a dispatched task becomes a live session in a pane — the thing
  # worth asserting about a screen. OpenCode falls back to headless.
  if harness_screen_interactive; then
    select_menu "$name" 'Interactive'
  else
    select_menu "$name" 'Headless'
  fi
  select_menu "$name" "$(harness_screen_label)"

  # The screen proper announces the worker it is running as and which agent
  # powers it. That line is also the confirmation that the menus above were
  # answered the way this scenario intended.
  local deadline=$(( $(date +%s) + READY_TIMEOUT_S )) pane
  while [ "$(date +%s)" -lt "$deadline" ]; do
    pane="$(tmux capture-pane -p -t "$SESSION:$name" 2>/dev/null || true)"
    # The embedded session opens a harness of its own, so the same hook-trust
    # question can appear here as in a wrapper pane.
    if printf '%s' "$pane" | grep -Eq 'Hooks need review|hooks are new or changed'; then
      select_menu "$name" 'Trust all'
      continue
    fi
    if printf '%s' "$pane" | grep -Eq "WORKER .*(interactive|headless) on $HARNESS"; then
      log "  operator screen is up: $(printf '%s' "$pane" | grep -Eo "WORKER.*on $HARNESS" | head -1 || true)"
      return 0
    fi
    sleep 1
  done
  tmux capture-pane -p -t "$SESSION:$name" > "$RUN_DIR/$name.pane" 2>/dev/null || true
  fail "\`medulla daemon --tui\` never reached its worker screen"
}

# Answer a live session's first-run consent prompts while an owner leg is in
# flight.
#   answer_session_prompts <window> <leg-label>
#
# Returns as soon as the leg has finished (its `.rc` exists) or the wait runs
# out; the caller still awaits the leg and asserts on it. This only clears
# prompts — it never fails, because "no prompt appeared" is the ordinary case.
answer_session_prompts() {
  local window="$1" label="$2"
  local deadline=$(( $(date +%s) + 180 )) pane
  while [ "$(date +%s)" -lt "$deadline" ]; do
    [ -f "$RUN_DIR/$label.rc" ] && return 0
    pane="$(tmux capture-pane -p -t "$SESSION:$window" 2>/dev/null || true)"
    if printf '%s' "$pane" | grep -Eq 'Hooks need review|hooks are new or changed'; then
      log "  answering the session's hook-trust prompt"
      select_menu "$window" 'Trust all'
      continue
    fi
    if printf '%s' "$pane" | grep -Eq 'Do you trust the contents|trust this folder|Yes, I trust'; then
      log "  answering the session's folder-trust prompt"
      tmux send-keys -t "$SESSION:$window" Enter
      sleep 2
      continue
    fi
    sleep 2
  done
  return 0
}

# Answer one of the operator screen's numbered setup menus, if it is asked.
#   select_menu <window> <label-regex>
#
# The option is chosen by its own number rather than by arrow keys, and the
# number is read off the pane rather than hardcoded: which agents a menu offers
# depends on what is installed on the box, so a fixed digit would select a
# different harness on a machine with a different set of CLIs.
#
# A menu Medulla does not ask is not an error. The agent question is skipped when
# only one agent is available, so on a single-CLI box the screen goes straight
# from the run-mode question to the worker header. Reaching the header therefore
# ends the wait successfully — what matters is that setup completed, not that
# every question was posed.
select_menu() {
  local window="$1" label="$2"
  local deadline=$(( $(date +%s) + 60 )) pane line digit
  while [ "$(date +%s)" -lt "$deadline" ]; do
    pane="$(tmux capture-pane -p -t "$SESSION:$window" 2>/dev/null || true)"
    # `|| true` because the suite runs under `pipefail`: a grep that has not
    # found the option yet is the normal case on every pass but the last, and
    # without this it ends the run instead of looping.
    line="$(printf '%s' "$pane" | grep -E "[0-9][.)]? +$label" | head -1 || true)"
    if [ -n "$line" ]; then
      digit="$(printf '%s' "$line" | grep -Eo '[0-9]' | head -1 || true)"
      [ -n "$digit" ] || fail "menu option '$label' has no number: $line"
      tmux send-keys -t "$SESSION:$window" "$digit"
      sleep 1
      tmux send-keys -t "$SESSION:$window" Enter
      sleep 3
      return 0
    fi
    if printf '%s' "$pane" | grep -Eq 'WORKER .*(interactive|headless) on '; then
      log "  '$label' was not asked — setup is already past it"
      return 0
    fi
    sleep 1
  done
  tmux capture-pane -p -t "$SESSION:$window" > "$RUN_DIR/$window.pane" 2>/dev/null || true
  fail "the operator screen never offered a '$label' option"
}

# Wait on the daemon screen's narrow agent rail, excluding the terminal pane so
# Claude's own spinner or prompt cannot accidentally satisfy a Medulla-state
# assertion.
wait_for_rail() {
  local window="$1" pattern="$2" timeout="${3:-$READY_TIMEOUT_S}"
  local deadline=$(( $(date +%s) + timeout ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if tmux capture-pane -p -t "$SESSION:$window" 2>/dev/null \
      | awk '{ print substr($0, 1, 34) }' | grep -Eq "$pattern"; then
      return 0
    fi
    sleep 1
  done
  tmux capture-pane -p -t "$SESSION:$window" > "$RUN_DIR/$window.pane" 2>/dev/null || true
  return 1
}

# ── 1. the wrapper is invisible in the terminal it wraps ────────────────────
scenario_wrapper_transparency() {
  scenario "\`medulla $HARNESS --no-bridge\` paints the real CLI and answers a prompt"
  launch_wrapper plain "$RUN_DIR/mhome-plain" --no-bridge
  # That the pane showed the harness's own chrome rather than a Medulla screen —
  # the whole claim of this mode — is already established: `launch_wrapper` waits
  # for that CLI's own composer placeholder and fails if it never appears. It is
  # not re-checked here because a composer hint is not required to survive the
  # turn; opencode replaces its placeholder with the answer.
  prompt_wrapper plain "reply with the marker for PLAIN-$$" \
    || fail "the wrapped $HARNESS TUI never painted COORDINATION_OK"
  log "  the wrapped $HARNESS TUI answered in its own chrome"
  ok "wrapper transparency"
}

# ── 2. the wrapper forwards the session to its orchestrator ─────────────────
#
# Asserted at the forwarder, not inside Medulla: the forwarder logs one line per
# datagram it moved, carrying only source, destination, sequence and size. Seeing
# state-carrying datagrams leave the wrapper's *own* node id is proof the session
# was bridged, and it is proof of exactly that and nothing more — a blind
# forwarder cannot tell a session event from an acknowledgement, which is the
# property the transport design rests on.
scenario_wrapper_bridging() {
  scenario "an enrolled wrapper session reaches its orchestrator over the link"
  if ! harness_wrapper_bridges; then
    skip "the wrapper does not tail $HARNESS transcripts (passthrough only)"
    return
  fi
  local node before after
  node="$(worker_id wrapper)"
  [ -n "$node" ] || fail "the wrapper pair was never enrolled"
  before="$(grep -c "forward src=$node .*kind=state" "$RUN_DIR/forwarder.log" || true)"

  launch_wrapper bridged "$RUN_DIR/mhome-wrapper"
  # The enrolled path goes through Medulla's own first-run wizard before the
  # harness gets the terminal. Asserting it happened is what keeps this scenario
  # honest: a wrapper that skipped registration would reach the harness faster
  # and forward nothing, which is the failure the datagram count below catches
  # only if we know registration was actually attempted.
  [ "${WIZARD_STEPS:-0}" -ge 1 ] \
    || fail "the enrolled wrapper never opened Medulla's worker-setup wizard"
  log "  answered $WIZARD_STEPS worker-setup step(s) before the harness took the terminal"
  prompt_wrapper bridged "reply with the marker for BRIDGED-$$" \
    || fail "the bridged $HARNESS TUI never painted COORDINATION_OK"

  # The transcript is tailed on a poll, so the forward trails the paint.
  local deadline=$(( $(date +%s) + 60 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    after="$(grep -c "forward src=$node .*kind=state" "$RUN_DIR/forwarder.log" || true)"
    [ "${after:-0}" -gt "${before:-0}" ] && break
    sleep 2
  done
  [ "${after:-0}" -gt "${before:-0}" ] \
    || fail "no state datagrams left the wrapper's node $node — the session was not bridged"
  log "  wrapper node $node forwarded $(( after - before )) state datagram(s)"
  ok "wrapper session bridging"
}

# ── 3. the operator screen paints, and the daemon keeps serving ─────────────
#
# `--tui` is a screen over a running daemon, not a different daemon. The failure
# worth catching is the screen taking over the process: a daemon that paints
# beautifully and stops answering its inbox is worse than one with no screen at
# all. So the assertion is both halves — the screen is up *and* a task
# dispatched while it is up comes back.
scenario_operator_screen() {
  scenario "\`medulla daemon --tui\` paints its screen and keeps serving tasks"
  boot_screen_daemon
  local pane
  pane="$(tmux capture-pane -p -t "$SESSION:screen" 2>/dev/null || true)"
  printf '%s' "$pane" > "$RUN_DIR/screen.pane"

  local task="emit the coordination marker TUI-$$"
  start_owner tui_task --to "$(worker_id screen)" \
    --task "$task" --task-id "tui-$$" --timeout-ms 180000
  # The embedded session's harness is spawned when the task arrives, not when
  # the screen came up, so its first-run consent prompts appear *now* — and a
  # harness parked on one never takes the prompt the worker is trying to type
  # into it ("the harness never took the prompt"). Answering while the leg is in
  # flight is what an operator watching the pane would do.
  answer_session_prompts screen tui_task
  await_owner tui_task
  [ "$OWNER_RC" = "0" ] || fail "task dispatched to the --tui daemon exited $OWNER_RC"
  "$PYTHON_BIN" - "$RUN_DIR/tui_task.json" <<'PY' || fail "operator-screen task assertion failed"
import json, sys
frame = json.load(open(sys.argv[1]))
assert frame.get("kind") == "Reply", f"kind={frame.get('kind')!r} (expected Reply)"
assert "COORDINATION_OK" in (frame.get("text") or ""), "the --tui daemon lost the marker"
print(f"[e2e]   reply while the screen is up: {(frame.get('text') or '')[:60]!r}", file=sys.stderr)
PY

  # In interactive mode the task is not merely served, it is *shown*: the screen
  # opens the harness in a pane and the operator watches the turn happen. That
  # is the claim the mode exists to make, and a reply frame alone would not
  # substantiate it — a headless daemon produces the same frame and paints
  # nothing.
  if harness_screen_interactive; then
    pane="$(tmux capture-pane -p -t "$SESSION:screen" 2>/dev/null || true)"
    printf '%s' "$pane" > "$RUN_DIR/screen.pane"
    printf '%s' "$pane" | grep -qF "$task" \
      || fail "the screen never showed the task it was serving"
    printf '%s' "$pane" | grep -q 'COORDINATION_OK' \
      || fail "the screen never showed the harness's answer"
    log "  the screen showed the task and its answer in a live session pane"
  fi
  ok "operator screen serves"
}

# ── 4. Claude lifecycle reports drive working and attention state ──────────
#
# The mock returns a real Anthropic `tool_use` block for the probe prompt. That
# makes the actual Claude CLI submit a dispatched turn and wait for Bash
# permission. Capturing the worker screen at both edges proves Medulla follows
# the harness lifecycle rather than a hand-authored terminal fixture.
scenario_claude_session_states() {
  scenario "Claude session moves from working to attention"
  if [ "$HARNESS" != claude ]; then
    skip "Claude lifecycle state capture (Claude-only)"
    return 0
  fi

  boot_screen_daemon state prompt
  local task='MEDULLA_STATE_PROBE: request the Bash tool now'
  start_owner state_task --to "$(worker_id state)" \
    --task "$task" --task-id "state-$$" --timeout-ms 180000

  wait_for_rail state '[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]' "$ANSWER_TIMEOUT_S" \
    || fail "Claude's dispatched turn never painted Medulla's working spinner"
  tmux capture-pane -p -t "$SESSION:state" > "$RUN_DIR/state-working.pane"

  wait_for_rail state '⚠' "$ANSWER_TIMEOUT_S" \
    || fail "Claude's Bash request never painted Medulla's attention glyph"
  tmux capture-pane -p -t "$SESSION:state" > "$RUN_DIR/state-attention.pane"

  # The worker screen is intentionally watch-only; cleanup terminates the
  # deliberately blocked fixture immediately after the suite reports success.
  log "  captured state-working.pane and state-attention.pane"
  ok "Claude lifecycle state capture"
}

summarize() {
  log ""
  log "═══════════════════════════════════════════════"
  printf '\n[e2e] PASS: all %d terminal scenarios green (%s):\n' "${#PASSED[@]}" "$HARNESS" >&2
  local s
  for s in "${PASSED[@]}"; do printf '[e2e]   ✓ %s\n' "$s" >&2; done
}

main "$@"

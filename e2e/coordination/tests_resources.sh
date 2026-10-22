#!/usr/bin/env bash
# Real-process proof of the host Resources tab under tmux. The app and all
# harnesses run in a network-isolated container; assertions use pane text from
# the running Medulla UI, not rendered fixtures.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

SESSION="medulla-e2e-resources-$$"
READY_TIMEOUT_S=90

capture_app() {
  tmux capture-pane -p -t "$SESSION:resources" 2>/dev/null || true
}

wait_app_regex() {
  local regex="$1" timeout="${2:-$READY_TIMEOUT_S}" deadline
  deadline=$(( $(date +%s) + timeout ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if capture_app | grep -Eq "$regex"; then return 0; fi
    sleep 0.5
  done
  capture_app > "$RUN_DIR/resources.pane" || true
  return 1
}

assert_panel_width() {
  local pane_file="$1" label="$2" min_width="$3"
  "$PYTHON_BIN" - "$pane_file" "$label" "$min_width" <<'PY'
import sys
path, label, minimum = sys.argv[1], sys.argv[2], int(sys.argv[3])
lines = open(path, encoding="utf-8").read().splitlines()
matches = [line for line in lines if label in line]
assert matches, f"no captured pane line contains {label!r}"
line = matches[0]
label_at = line.index(label)
left_candidates = [i for i in (line.find("┌"), line.find("├"), line.find("│")) if 0 <= i < label_at]
right_candidates = [i for i in (line.find("┐", label_at), line.find("┤", label_at), line.find("│", label_at)) if i >= 0]
assert left_candidates and right_candidates, f"cannot find panel borders around {label!r}: {line!r}"
width = min(right_candidates) - max(left_candidates) + 1
assert width >= minimum, f"{label} panel width {width} < {minimum}: {line!r}"
print(f"[e2e] {label} panel width: {width} columns")
PY
}

main() {
  RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/medulla-resources-e2e-XXXXXX")"
  EXTERNAL_DIR="${TMPDIR:-/tmp}/coordination-external-$$"
  e2e_init
  boot_llm "export MOCK_LLM_RESPONSE_DELAY_MS=15000"

  # Run a minimal real workflow in the client TUI. Its local workflow host
  # dispatches the real harness from the same Medulla process as the sampler,
  # so the resource row can be joined to its registered session origin.
  local app_config="$RUN_DIR/app.json" app_home="$RUN_DIR/app-home" workspace="$RUN_DIR/work"
  local app_transport_env=""
  if [ "$HARNESS" = codex ]; then
    app_transport_env="$(DAEMON_TRANSPORT=acp harness_transport_env)"
  fi
  mkdir -p "$app_home" "$workspace/.medulla/workflows"
  harness_seed_home "$app_home/harness-home" "$workspace"
  "$PYTHON_BIN" - "$HARNESS" "${MEDULLA_CONFIG:-}" "$app_config" "$workspace/.medulla/workflows/resources-proof.json" <<'PY'
import json, sys
harness, source, target, workflow_path = sys.argv[1:]
try:
    config = json.load(open(source)) if source else {}
except FileNotFoundError:
    config = {}
config["resources"] = {"updateMs": 2000}
config["workflows"] = {"defaultProvider": harness, "evolve": {"autoOnFailure": False}}
json.dump(config, open(target, "w"), indent=2)
workflow = {
    "id": "resources-proof", "name": "Resources proof",
    "description": "Keep a real harness request visible for Resources attribution.",
    "nodes": [
        {"id": "trigger", "kind": "trigger", "name": "start", "config": {"trigger_kind": "manual"}},
        {"id": "agent", "kind": "agent", "name": "Keep harness live", "config": {"prompt": "reply with the coordination marker RESOURCES_PROOF"}},
    ],
    "edges": [{"from_node": "trigger", "to_node": "agent"}],
}
json.dump(workflow, open(workflow_path, "w"), indent=2)
PY

  cat > "$RUN_DIR/resources.cmd" <<EOF
#!/usr/bin/env bash
set -euo pipefail
$(harness_env "$app_home/harness-home")
$app_transport_env
export HOME=$(printf %q "$app_home")
export MEDULLA_HOME=$(printf %q "$app_home")
export MEDULLA_USER=e2e
export MEDULLA_ANALYTICS_DISABLED=1
echo \$\$ > $(printf %q "$RUN_DIR/app.pid")
cd $(printf %q "$workspace")
exec $(printf %q "$MEDULLA_BIN") --mock --config $(printf %q "$app_config")
EOF
  chmod +x "$RUN_DIR/resources.cmd"
  tmux new-window -t "$SESSION" -n resources -c "$workspace"
  tmux send-keys -t "$SESSION:resources" "bash $(printf %q "$RUN_DIR/resources.cmd")" C-m
  wait_app_regex 'Sessions|Welcome|Start a new session' 60 \
    || fail "Medulla app did not paint its initial screen"
  tmux send-keys -t "$SESSION:resources" Tab
  wait_app_regex 'Resources proof' || fail "the workflow fixture was not loaded"
  tmux send-keys -t "$SESSION:resources" x
  sleep 1
  tmux send-keys -t "$SESSION:resources" Tab Tab
  wait_app_regex 'CPU.*2000ms' \
    || fail "Resources did not open at its 2000ms default interval"

  local pane provider_label
  case "$HARNESS" in
    opencode) provider_label='OpenCode' ;;
    claude) provider_label='Claude' ;;
    codex) provider_label='Codex' ;;
  esac
  pane="$(capture_app)"
  printf '%s\n' "$pane" > "$RUN_DIR/resources-initial.pane"
  for title in 'CPU [1]' 'Mem [2]' 'Net [3]' 'Agents [4]'; do
    printf '%s\n' "$pane" | grep -Fq "┌ $title" \
      || fail "Resources screen is missing the $title box title"
  done

  # The workflow run is the process registry's task origin until the provider
  # reports a native session id. Wait for the launched run's provider row.
  local task_id="resources-proof" deadline=$(( $(date +%s) + READY_TIMEOUT_S ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    pane="$(capture_app)"
    if printf '%s\n' "$pane" | grep -Eq "│[0-9]+[[:space:]]+$provider_label[[:space:]]+"; then break; fi
    sleep 1
  done
  printf '%s\n' "$pane" > "$RUN_DIR/resources-session.pane"
  "$PYTHON_BIN" - "$RUN_DIR/resources-session.pane" "$provider_label" "$task_id" "$RUN_DIR/session.pid" <<'PY' \
    || fail "Resources did not attribute a running $HARNESS process to its session"
import sys
text = open(sys.argv[1], encoding="utf-8").read()
rows = []
seen = []
for line in text.splitlines():
    fields = line.replace("│", " ").split()
    if any(token.lower() == sys.argv[2].lower() for token in fields):
        seen.append(fields)
    for index, token in enumerate(fields):
        if token.lower() == sys.argv[2].lower() and index >= 1 and index + 1 < len(fields) and fields[index - 1].isdigit():
            origin = fields[index + 1]
            if origin.lower() == "task" and index + 2 < len(fields):
                origin = f"task {fields[index + 2]}"
            rows.append((line, fields[index - 1], origin))
assert rows, f"no {sys.argv[2]} process row in Resources pane; provider lines={seen!r}"
line, pid, origin = rows[0]
open(sys.argv[4], "w", encoding="utf-8").write(pid)
assert origin.lower() not in {"external", "host", "—"}, \
    f"{sys.argv[2]} row is not attributed to its harness session: {line!r}"
if origin.lower().startswith("task "):
    assert origin.startswith("task wf:run-"), \
        f"expected this workflow's run origin, got {origin!r}"
    assert "Running Resources proof" in text, \
        "the proof workflow is not visibly running beside its attributed task row"
    print(f"[e2e] workflow run origin {origin} for PID {pid}: {line.strip()}")
else:
    assert sys.argv[3] not in origin, f"unexpected workflow task origin {origin!r}"
    print(f"[e2e] provider session id {origin} for PID {pid}: {line.strip()}")
PY
  log "  dispatched $HARNESS process is visible with its session or exact workflow-run origin"

  # A second process has the harness executable basename but was started by
  # tmux, outside Medulla. It has no registry entry and must render as external.
  mkdir -p "$EXTERNAL_DIR"
  local external_name="$HARNESS"
  cp "$(command -v python3)" "$EXTERNAL_DIR/$external_name"
  cat > "$RUN_DIR/external.cmd" <<EOF
#!/usr/bin/env bash
"$EXTERNAL_DIR/$external_name" -c 'import time; time.sleep(180)' >/dev/null 2>&1 &
echo \$! > $(printf %q "$RUN_DIR/external.pid")
wait
EOF
  chmod +x "$RUN_DIR/external.cmd"
  tmux new-window -t "$SESSION" -n external -c "$workspace"
  tmux send-keys -t "$SESSION:external" "bash $(printf %q "$RUN_DIR/external.cmd")" C-m
  wait_for_regex "$RUN_DIR/external.pid" '.' 10 || fail "external process fixture did not start"
  local external_pid="$(cat "$RUN_DIR/external.pid")"
  [ "$(basename "$(readlink "/proc/$external_pid/exe")")" = "$external_name" ] \
    || fail "external fixture executable is not named like a harness binary"
  local external_started_at="$(date +%s)"
  deadline=$(( external_started_at + 5 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    kill -0 "$external_pid" 2>/dev/null \
      || fail "external fixture exited before spanning two sampler intervals"
    sleep 1
  done
  [ "$(( $(date +%s) - external_started_at ))" -ge 4 ] \
    || fail "external fixture did not span at least two 2000ms sampler intervals"
  deadline=$(( $(date +%s) + 30 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    kill -0 "$external_pid" 2>/dev/null \
      || fail "external fixture exited before the Resources sampler could refresh it"
    pane="$(capture_app)"
    if printf '%s\n' "$pane" | "$PYTHON_BIN" -c 'import sys; pid=sys.argv[1]; sys.exit(0 if any(pid in line.replace("│", " ").split() for line in sys.stdin) else 1)' "$external_pid"; then
      break
    fi
    sleep 1
  done
  kill -0 "$external_pid" 2>/dev/null \
    || fail "external fixture exited before its captured Resources row was checked"
  printf '%s\n' "$pane" > "$RUN_DIR/resources-external.pane"
  "$PYTHON_BIN" - "$RUN_DIR/resources-external.pane" "$external_pid" "$provider_label" <<'PY' \
    || fail "the unregistered $HARNESS process was not labeled external"
import sys
text = open(sys.argv[1], encoding="utf-8").read().splitlines()
rows = [line for line in text if sys.argv[2] in line.replace("│", " ").split()]
assert rows, f"external PID {sys.argv[2]} has no Agents row"
fields = rows[0].replace("│", " ").split()
pid_index = fields.index(sys.argv[2])
assert pid_index + 2 < len(fields), f"external PID {sys.argv[2]} has an incomplete Agents row: {rows!r}"
kind, origin = fields[pid_index + 1 : pid_index + 3]
assert origin.lower() == "external", \
    f"external PID {sys.argv[2]} has origin {origin!r}, expected 'external': {rows!r}"
assert kind.lower() in {"external", sys.argv[3].lower()}, \
    f"external PID {sys.argv[2]} has unexpected kind {kind!r}: {rows!r}"
print(f"[e2e] external process row: {rows[0].strip()}")
PY
  kill "$external_pid" 2>/dev/null || true
  log "  unregistered $HARNESS-like process is visible with origin external"

  # Toggle Agents off and prove the production-rendered Mem/Net borders widen.
  assert_panel_width "$RUN_DIR/resources-external.pane" 'Mem [2]' 40
  assert_panel_width "$RUN_DIR/resources-external.pane" 'Net [3]' 40
  tmux send-keys -t "$SESSION:resources" 4
  sleep 2
  pane="$(capture_app)"
  printf '%s\n' "$pane" > "$RUN_DIR/resources-agents-hidden.pane"
  printf '%s\n' "$pane" | grep -Fq '┌ Agents [4]' \
    && fail "Agents box title remained after pressing 4"
  assert_panel_width "$RUN_DIR/resources-agents-hidden.pane" 'Mem [2]' 120
  assert_panel_width "$RUN_DIR/resources-agents-hidden.pane" 'Net [3]' 120
  tmux send-keys -t "$SESSION:resources" 4
  sleep 1
  tmux send-keys -t "$SESSION:resources" +
  wait_app_regex 'CPU.*2100ms' \
    || fail "pressing + did not advance interval label from 2000ms to 2100ms"
  tmux send-keys -t "$SESSION:resources" -
  wait_app_regex 'CPU.*2000ms' \
    || fail "pressing - did not restore the 2000ms sampler interval"
  log "  Agents visibility, Mem/Net width, and interval controls passed"

  # Keep the production sampler running in the same Medulla process, but hide
  # the charts and table after their behavior has been asserted. Their full
  # width terminal repaints would otherwise be counted against the sampler's
  # process-level CPU measurement below.
  tmux send-keys -t "$SESSION:resources" 1 2 3 4
  sleep 1
  pane="$(capture_app)"
  for title in 'CPU [1]' 'Mem [2]' 'Net [3]' 'Agents [4]'; do
    printf '%s\n' "$pane" | grep -Fq "┌ $title" \
      && fail "$title remained visible before sampler CPU measurement"
  done
  # Keep the Resources view alive while minimizing unrelated terminal repaint
  # work; every panel's production behavior has already been asserted above.
  tmux resize-window -t "$SESSION:resources" -x 80 -y 24

  # Stop the proof harness once its real session row has been asserted. The
  # workflow fixture exists to keep a provider session observable; its outcome
  # is outside this proof. Keep automatic failure review disabled in app.json,
  # then wait for the run to settle before measuring Medulla's process CPU.
  local session_pid="$(cat "$RUN_DIR/session.pid")"
  kill "$session_pid" 2>/dev/null || true
  deadline=$(( $(date +%s) + 30 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if ! kill -0 "$session_pid" 2>/dev/null; then break; fi
    state="$(awk '{print $3}' "/proc/$session_pid/stat" 2>/dev/null || true)"
    [ "$state" = Z ] && break
    sleep 1
  done
  wait_app_regex 'resources-proof: (failed|succeeded)' 30 \
    || fail "the Resources proof workflow did not settle after ending its harness session"
  pane="$(capture_app)"
  printf '%s\n' "$pane" | grep -Fq 'Reviewing why resources-proof failed' \
    && fail "failure review remained active before the performance sample"
  log "  proof harness process $session_pid ended and the workflow settled before sampler CPU measurement"

  # §7 of resources-tab.md specifies 1000 processes at a 2000ms cadence, with
  # sampler CPU p95 <=40ms per tick and <=2% of one core averaged over one
  # minute. Keep 1000 unclassified sleepers alive so they exercise the real
  # process walk without filling the Agents table. Process-level /proc stats
  # conservatively include the rest of Medulla as well as the sampler.
  local perf_seconds="${E2E_RESOURCE_PERF_SECONDS:-60}"
  local app_pid="$(cat "$RUN_DIR/app.pid")"
  "$PYTHON_BIN" - "$app_pid" "$perf_seconds" <<'PY' || fail "Resources process exceeded the sampler CPU budget"
import os, subprocess, sys, time
pid, duration = sys.argv[1], float(sys.argv[2])
fixtures = []
try:
    for _ in range(1000):
        fixtures.append(subprocess.Popen(["sleep", "120"], stdin=subprocess.DEVNULL,
                                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
    assert len(fixtures) == 1000, f"only started {len(fixtures)} process fixtures"
    ticks = os.sysconf("SC_CLK_TCK")
    def cpu_ticks():
        fields = open(f"/proc/{pid}/stat").read().split()
        return int(fields[13]) + int(fields[14])
    if int(duration) < 2:
        raise ValueError("perf sampling must span at least one sampler tick")
    interval = 2.0
    count = max(1, int(duration // interval))
    time.sleep(interval * 2)
    points = [cpu_ticks()]
    start = time.monotonic()
    for _ in range(count):
        time.sleep(interval)
        points.append(cpu_ticks())
    elapsed = time.monotonic() - start
    tick_ms = [(points[i + 1] - points[i]) / ticks * 1000 for i in range(count)]
    average_pct = sum(tick_ms) / (elapsed * 10)
    ordered = sorted(tick_ms)
    p95 = ordered[min(len(ordered) - 1, int(0.95 * len(ordered)))]
    print(f"[e2e] process fixture: {len(fixtures)} unclassified sleepers; {len(tick_ms)} Medulla process CPU samples at {interval:.0f}s cadence; p95={p95:.1f}ms/tick, max={max(tick_ms):.1f}ms/tick, average={average_pct:.3f}% of one core over {elapsed:.1f}s (limits: <=40ms/tick, <=2.000%)")
    print(f"[e2e] sorted per-tick process CPU samples (ms): {ordered}")
    assert p95 <= 40.0, f"Medulla CPU p95 {p95:.1f}ms per 2s tick exceeds 40ms"
    assert average_pct <= 2.0, f"Medulla CPU average {average_pct:.3f}% exceeds 2% of one core"
finally:
    for proc in fixtures:
        if proc.poll() is None:
            proc.terminate()
    for proc in fixtures:
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
PY

  log "PASS: Resources live process scenarios ($HARNESS)"
}

main "$@"

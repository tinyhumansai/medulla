#!/usr/bin/env bash
# Multi-agent scenario suite for the medulla coordination harness.
#
# `tests.sh` exercises one daemon; this file exercises a *fleet*. It boots two
# medulla daemons — each with its own workspace, MEDULLA_HOME and tiny.place
# identity — against one shared mock Signal server and one shared mock LLM:
#
#   mock Signal server ──┬── daemon "alpha" (workspace work-alpha) ── opencode ──┐
#                        └── daemon "beta"  (workspace work-beta)  ── opencode ──┴─→ mock LLM
#
# Scenarios:
#
#   1. fleet-registration — both daemons reach the serving state and register as
#                           two *distinct* worker identities on one relay.
#   2. workspace-binding  — each daemon reports its own workspace as `cwd`, and
#                           reads only its own workspace's AGENTS.md. Proven via
#                           per-workspace sentinels: alpha's sentinel appears in
#                           the LLM request log, beta's never appears in a request
#                           alpha caused, and vice versa.
#   3. concurrent-routing — two owner legs dispatched in parallel, one per worker,
#                           both return their own task's marker with no cross-talk.
#   4. fresh-review       — a review task is routed to a different worker and
#                           returns the required structured verdict note.
#   5. crash-containment  — killing beta mid-task does not disturb alpha: alpha
#                           still answers a fresh task while beta is down.
#   6. crash-recovery     — restarting beta re-onboards the *same* identity and it
#                           serves again, proving a crash is not terminal.
#
# Scenarios 1–6 share one booted stack. Same env overrides as run.sh (see lib.sh).
# Exit 0 iff every scenario passes.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

PASSED=()
scenario() { log ""; log "═══ SCENARIO: $1 ═══"; }
ok()       { PASSED+=("$1"); log "  ✓ SCENARIO PASS: $1"; }

# Sentinels planted in each workspace's AGENTS.md. Unique per run so a stale
# container or leaked log can never make an assertion pass by accident.
ALPHA_SENTINEL="ALPHAMARK-$$-$RANDOM"
BETA_SENTINEL="BETAMARK-$$-$RANDOM"

# ── assertion helpers ───────────────────────────────────────────────────────

# Assert that a terminal frame JSON is a successful Reply carrying MARKER.
assert_reply_contains() {
  local json="$1" marker="$2" label="$3"
  "$PYTHON_BIN" - "$json" "$marker" "$label" <<'PY' || fail "reply assertion failed for $3"
import json, sys
path, marker, label = sys.argv[1], sys.argv[2], sys.argv[3]
frame = json.load(open(path))
assert frame.get("kind") == "Reply", f"{label}: kind={frame.get('kind')!r} (expected Reply)"
text = frame.get("text") or ""
assert marker in text, f"{label}: reply missing {marker!r}: {text[:200]!r}"
print(f"[e2e]   {label}: reply carries {marker}", file=sys.stderr)
PY
}

# Assert a terminal frame JSON does NOT mention MARKER (cross-talk guard).
assert_reply_excludes() {
  local json="$1" marker="$2" label="$3"
  "$PYTHON_BIN" - "$json" "$marker" "$label" <<'PY' || fail "cross-talk assertion failed for $3"
import json, sys
path, marker, label = sys.argv[1], sys.argv[2], sys.argv[3]
frame = json.load(open(path))
text = frame.get("text") or ""
assert marker not in text, f"{label}: CROSS-TALK — reply leaked {marker!r}: {text[:200]!r}"
print(f"[e2e]   {label}: no leak of {marker}", file=sys.stderr)
PY
}

# ── stack ───────────────────────────────────────────────────────────────────
boot_fleet() {
  SESSION="medulla-e2e-multi-$$"
  RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/medulla-e2e-XXXXXX")"
  e2e_init
  boot_signal
  boot_llm ""

  WORK_ALPHA="$(make_workspace alpha "$ALPHA_SENTINEL")"
  WORK_BETA="$(make_workspace beta "$BETA_SENTINEL")"
  boot_daemon_named alpha "$WORK_ALPHA"
  boot_daemon_named beta  "$WORK_BETA"
}

main() {
  local started; started=$(date +%s)
  boot_fleet

  local alpha beta
  alpha="$(worker_id alpha)"
  beta="$(worker_id beta)"

  # ── 1. two distinct workers on one relay ──────────────────────────────────
  scenario "two daemons register as distinct workers on one relay"
  [ -n "$alpha" ] || fail "alpha registered no worker id"
  [ -n "$beta" ]  || fail "beta registered no worker id"
  [ "$alpha" != "$beta" ] \
    || fail "both daemons registered the SAME worker id ($alpha) — identities collided"
  log "  alpha=$alpha  beta=$beta"
  ok "fleet registration"

  # ── 2. each daemon is bound to its own workspace ──────────────────────────
  # A capabilities probe makes each daemon read its workspace and self-describe.
  scenario "each daemon reports and reads only its own workspace"
  start_owner caps_alpha --endpoint "$SIGNAL_URL" --to "$alpha" \
    --kind capabilities --task "report" --task-id "caps-alpha-$$" --timeout-ms 120000
  start_owner caps_beta --endpoint "$SIGNAL_URL" --to "$beta" \
    --kind capabilities --task "report" --task-id "caps-beta-$$" --timeout-ms 120000
  await_owner caps_alpha
  [ "$OWNER_RC" = "0" ] || fail "alpha capabilities owner exited $OWNER_RC (expected 0)"
  await_owner caps_beta
  [ "$OWNER_RC" = "0" ] || fail "beta capabilities owner exited $OWNER_RC (expected 0)"

  "$PYTHON_BIN" - "$RUN_DIR/caps_alpha.json" "$WORK_ALPHA" "$WORK_BETA" <<'PY' \
    || fail "alpha workspace-binding assertion failed"
import json, sys
frame = json.load(open(sys.argv[1]))
mine, theirs = sys.argv[2], sys.argv[3]
assert frame.get("kind") == "CapabilitiesResult", f"kind={frame.get('kind')!r}"
caps = json.loads(frame.get("text") or "{}")
cwd = caps.get("cwd") or ""
assert cwd.rstrip("/").endswith(mine.rstrip("/").rsplit("/", 1)[-1]), \
    f"alpha cwd {cwd!r} is not its own workspace {mine!r}"
dirs = caps.get("accessible_dirs") or caps.get("accessibleDirs") or []
assert not any(theirs in str(d) for d in dirs), \
    f"alpha advertises beta's workspace in accessible_dirs: {dirs!r}"
print(f"[e2e]   alpha cwd={cwd!r}", file=sys.stderr)
PY

  "$PYTHON_BIN" - "$RUN_DIR/caps_beta.json" "$WORK_BETA" "$WORK_ALPHA" <<'PY' \
    || fail "beta workspace-binding assertion failed"
import json, sys
frame = json.load(open(sys.argv[1]))
mine, theirs = sys.argv[2], sys.argv[3]
assert frame.get("kind") == "CapabilitiesResult", f"kind={frame.get('kind')!r}"
caps = json.loads(frame.get("text") or "{}")
cwd = caps.get("cwd") or ""
assert cwd.rstrip("/").endswith(mine.rstrip("/").rsplit("/", 1)[-1]), \
    f"beta cwd {cwd!r} is not its own workspace {mine!r}"
dirs = caps.get("accessible_dirs") or caps.get("accessibleDirs") or []
assert not any(theirs in str(d) for d in dirs), \
    f"beta advertises alpha's workspace in accessible_dirs: {dirs!r}"
print(f"[e2e]   beta cwd={cwd!r}", file=sys.stderr)
PY

  # The sentinels prove the *directory read*, not just the reported path: each
  # daemon grounded its probe in its own AGENTS.md, so both sentinels reached the
  # LLM, and neither daemon could have read the other's file.
  "$PYTHON_BIN" - "$RUN_DIR/llm.jsonl" "$ALPHA_SENTINEL" "$BETA_SENTINEL" <<'PY' \
    || fail "workspace sentinel assertion failed"
import json, sys
log_path, alpha_s, beta_s = sys.argv[1], sys.argv[2], sys.argv[3]
alpha_hits = beta_hits = 0
both_in_one = 0
for line in open(log_path, encoding="utf-8"):
    line = line.strip()
    if not line:
        continue
    blob = json.dumps(json.loads(line))
    a, b = alpha_s in blob, beta_s in blob
    alpha_hits += a
    beta_hits += b
    both_in_one += a and b
assert alpha_hits, f"alpha sentinel {alpha_s} never reached the LLM — workspace not read"
assert beta_hits, f"beta sentinel {beta_s} never reached the LLM — workspace not read"
assert not both_in_one, (
    "a single LLM request carried BOTH workspace sentinels — a daemon read "
    "outside its own workspace"
)
print(f"[e2e]   sentinels: alpha in {alpha_hits} req, beta in {beta_hits} req, "
      f"never co-occurring", file=sys.stderr)
PY
  ok "per-daemon workspace binding"

  # ── 3. concurrent routing, no cross-talk ──────────────────────────────────
  scenario "concurrent tasks route to the right worker with no cross-talk"
  local ta="TASKALPHA-$$" tb="TASKBETA-$$"
  start_owner task_alpha --endpoint "$SIGNAL_URL" --to "$alpha" \
    --task "emit the marker for $ta" --task-id "ta-$$" --timeout-ms 180000
  start_owner task_beta --endpoint "$SIGNAL_URL" --to "$beta" \
    --task "emit the marker for $tb" --task-id "tb-$$" --timeout-ms 180000
  await_owner task_alpha
  [ "$OWNER_RC" = "0" ] || fail "alpha task owner exited $OWNER_RC (expected 0)"
  await_owner task_beta
  [ "$OWNER_RC" = "0" ] || fail "beta task owner exited $OWNER_RC (expected 0)"
  # The mock LLM echoes the prompt, so each reply must carry its OWN task marker
  # and never the sibling's — that is the routing assertion.
  assert_reply_contains "$RUN_DIR/task_alpha.json" "$ta" "alpha"
  assert_reply_excludes "$RUN_DIR/task_alpha.json" "$tb" "alpha"
  assert_reply_contains "$RUN_DIR/task_beta.json" "$tb" "beta"
  assert_reply_excludes "$RUN_DIR/task_beta.json" "$ta" "beta"
  ok "concurrent routing without cross-talk"

  # ── 4. fresh-context review returns a structured verdict note ─────────────
  scenario "review task runs on a different worker and returns APPROVE"
  local review_task="MEDULLA_AUTOREVIEW target=ta-$$
Delegate this review to beta. The implementer alpha MUST NOT perform this review.

## Contract
Outcome: emit the marker for $ta
Non-goals:
- do not change beta
Verify:
- inspect the supplied evidence

## Exact diff
\`\`\`diff
-before
+after
\`\`\`

## Required verdict
APPROVE | FINDINGS:"
  run_owner review --endpoint "$SIGNAL_URL" --to "$beta" \
    --task "$review_task" --task-id "review-$$" --timeout-ms 180000
  [ "$OWNER_RC" = "0" ] || fail "review owner exited $OWNER_RC (expected 0)"
  "$PYTHON_BIN" - "$RUN_DIR/review.json" <<'PY' || fail "fresh-review assertion failed"
import json, sys
frame = json.load(open(sys.argv[1]))
assert frame.get("kind") == "Reply", f"kind={frame.get('kind')!r}"
text = (frame.get("text") or "").strip()
assert "APPROVE" in text, f"review did not return required verdict: {text!r}"
assert "FINDINGS:" not in text, f"review returned both verdict shapes: {text!r}"
print(f"[e2e]   review verdict note: {text!r}", file=sys.stderr)
PY
  ok "fresh-context review verdict round trip"

  # ── 5. a crashed daemon does not take the fleet down ──────────────────────
  scenario "killing beta leaves alpha serving"
  kill_daemon beta
  # Beta is gone: nothing drains its inbox, so this leg must NOT come back.
  start_owner orphan --endpoint "$SIGNAL_URL" --to "$beta" \
    --task "emit the marker for ORPHAN-$$" --task-id "orphan-$$" --timeout-ms 20000
  # Meanwhile alpha must still work.
  local survive="SURVIVE-$$"
  run_owner task_survive --endpoint "$SIGNAL_URL" --to "$alpha" \
    --task "emit the marker for $survive" --task-id "surv-$$" --timeout-ms 180000
  [ "$OWNER_RC" = "0" ] || fail "alpha stopped serving after beta was killed (rc=$OWNER_RC)"
  assert_reply_contains "$RUN_DIR/task_survive.json" "$survive" "alpha-after-beta-crash"

  await_owner_maybe orphan 60
  if [ "$OWNER_RC" = "0" ]; then
    fail "task to the killed daemon 'beta' succeeded — it was still being served"
  fi
  log "  orphaned leg to dead beta did not succeed (rc=${OWNER_RC:-<still running>})"
  ok "crash containment"

  # ── 6. the crashed daemon recovers ────────────────────────────────────────
  scenario "restarting beta restores service under the same identity"
  boot_daemon_named beta "$WORK_BETA"
  local rebooted; rebooted="$(worker_id beta)"
  [ "$rebooted" = "$beta" ] \
    || fail "beta re-onboarded as a DIFFERENT identity ($rebooted != $beta) — config not reused"
  local again="RECOVER-$$"
  run_owner task_recover --endpoint "$SIGNAL_URL" --to "$beta" \
    --task "emit the marker for $again" --task-id "rec-$$" --timeout-ms 180000
  [ "$OWNER_RC" = "0" ] || fail "restarted beta did not serve (rc=$OWNER_RC)"
  assert_reply_contains "$RUN_DIR/task_recover.json" "$again" "beta-after-restart"
  ok "crash recovery"

  local elapsed=$(( $(date +%s) - started ))
  log ""
  log "═══════════════════════════════════════════════"
  printf '\n[e2e] PASS: all %d multi-agent scenarios green (%ds):\n' \
    "${#PASSED[@]}" "$elapsed" >&2
  local s
  for s in "${PASSED[@]}"; do printf '[e2e]   ✓ %s\n' "$s" >&2; done
}

main "$@"

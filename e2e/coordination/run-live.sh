#!/usr/bin/env bash
# Live counterpart to the mocked coordination suite: the SAME two-daemon fleet,
# but pointed at real infrastructure instead of the local mocks.
#
#   mocked (tests_multi.sh)          live (this script)
#   ───────────────────────          ──────────────────
#   mock_signal_server (loopback) →  staging tiny.place relay
#   mock_llm.py        (loopback) →  OpenRouter
#
# Everything else — the daemons, opencode, the owner legs, the assertions — is
# identical, which is the point: this is how you find out that a green mocked
# suite still breaks against real staging.
#
# THIS SPENDS REAL MONEY AND TALKS TO REAL SERVERS. It is opt-in on every axis
# and will refuse to run unless you have said so explicitly:
#
#   E2E_LIVE=1                 required; the deliberate "yes I mean it" switch
#   OPENROUTER_API_KEY=sk-or-… required; billed per token
#   MEDULLA_STAGING=1          default; set MEDULLA_STAGING=0 to target prod,
#                              which additionally requires E2E_ALLOW_PROD=1
#
# Optional:
#   LIVE_MODEL=<slug>          OpenRouter model (default: a cheap small model)
#   TINYPLACE_ENDPOINT=<url>   override the relay (default: staging)
#   E2E_KEEP=1                 keep the run dir + tmux session for inspection
#
# Usage:
#   E2E_LIVE=1 OPENROUTER_API_KEY=sk-or-… bash e2e/coordination/run-live.sh
#
# Exit 0 iff every live scenario passes.
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

# ── guardrails ──────────────────────────────────────────────────────────────
# Every check here fails closed. A live suite that runs by accident — in CI, in
# a loop, against prod — is worse than one that is annoying to start.
preflight() {
  [ "${E2E_LIVE:-0}" = "1" ] || {
    printf '%s\n' \
      "refusing to run: this suite bills a real OpenRouter key and talks to real" \
      "tiny.place infrastructure. Re-run with E2E_LIVE=1 if that is what you want." >&2
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

  if [ "$MEDULLA_STAGING" = "1" ]; then
    LIVE_ENDPOINT="${TINYPLACE_ENDPOINT:-https://staging-api.tiny.place}"
    log "target: STAGING ($LIVE_ENDPOINT)"
  else
    LIVE_ENDPOINT="${TINYPLACE_ENDPOINT:-https://api.tiny.place}"
    log "target: PRODUCTION ($LIVE_ENDPOINT)  [E2E_ALLOW_PROD=1]"
  fi

  command -v curl >/dev/null || die "curl is required"
  # Fail fast on an unreachable relay rather than 90s into a daemon boot timeout.
  curl -fsS --max-time 15 -o /dev/null "$LIVE_ENDPOINT" 2>/dev/null \
    || log "  warning: GET $LIVE_ENDPOINT was not OK — continuing (the relay may not serve /)"

  log "model:  $LIVE_MODEL (OpenRouter)"
  log "key:    ${OPENROUTER_API_KEY:0:8}…${OPENROUTER_API_KEY: -4}"
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

# Boot the live fleet. No boot_signal, no boot_llm — those are the mocks we are
# deliberately replacing. SIGNAL_URL is what lib.sh feeds to TINYPLACE_ENDPOINT.
boot_live_fleet() {
  SESSION="medulla-live-$$"
  RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/medulla-live-XXXXXX")"
  e2e_init
  SIGNAL_URL="$LIVE_ENDPOINT"
  write_live_opencode_config

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

  # ── 1. two daemons onboard against the real relay ─────────────────────────
  scenario "two daemons onboard to the live relay as distinct identities"
  [ -n "$alpha" ] && [ -n "$beta" ] || fail "a daemon failed to onboard against $LIVE_ENDPOINT"
  [ "$alpha" != "$beta" ] || fail "both daemons onboarded as the SAME identity ($alpha)"
  log "  alpha=$alpha  beta=$beta"
  ok "live fleet onboarding"

  # ── 2. capabilities against a real model ──────────────────────────────────
  # A real model actually answers the probe, so unlike the mock run we can assert
  # the model populated `tools` — proof the probe round-tripped through OpenRouter
  # rather than falling back to the deterministic local digest.
  scenario "capabilities probe round-trips through OpenRouter"
  run_owner caps_alpha --endpoint "$SIGNAL_URL" --to "$alpha" \
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
  start_owner live_alpha --endpoint "$SIGNAL_URL" --to "$alpha" \
    --task "Reply with exactly the word ALPHAOK and nothing else." \
    --task-id "live-a-$$" --timeout-ms 300000
  start_owner live_beta --endpoint "$SIGNAL_URL" --to "$beta" \
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
  log "note: live runs leave real identities on $LIVE_ENDPOINT"
}

main "$@"

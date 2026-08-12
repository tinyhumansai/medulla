#!/usr/bin/env bash
# Everything the coordination e2e harness knows about a specific coding CLI.
#
# The stack in `lib.sh` is harness-agnostic: it enrolls a link pair, boots a
# forwarder, a mock LLM and a `medulla daemon`, and asserts on the reply that
# comes back. *Which* CLI the daemon spawns underneath, and how that CLI is
# pointed at the mock LLM, lives here — one function per question, switching on
# `$E2E_HARNESS`.
#
#   opencode  reached through its own provider block (`opencode.json`), the
#             arrangement that predates custom harness presets.
#   claude    reached through a Medulla *custom harness preset*: the daemon is
#             given a config naming a preset whose `baseUrl` is the mock LLM, and
#             Medulla injects `ANTHROPIC_BASE_URL`/`ANTHROPIC_AUTH_TOKEN` at the
#             spawn seam exactly as it does for a real OpenRouter preset.
#   codex     the same, plus `codexOverrides` — Codex ignores a bare
#             `OPENAI_BASE_URL` (it prefers a signed-in account and refuses an
#             uncatalogued model), so the preset's `-c` provider block and derived
#             model catalog are what actually get the mock answering.
#
# That makes the claude/codex legs a test of Medulla's real routing path rather
# than of a bespoke test-only arrangement: the only fake in the chain is the
# model behind the endpoint.
#
# Set by `resolve_harness`/`write_harness_config`, read by `lib.sh`:
#   HARNESS            the selected CLI (opencode|claude|codex)
#   HARNESS_BIN        its absolute path
#   HARNESS_DIR        the directory holding it, prepended to a spawn's PATH
#   OC_CONFIG          opencode's provider config (opencode leg only)
#   MEDULLA_CONFIG     the daemon config carrying the preset (claude/codex legs)

# The CLI under test. Every scenario honours it, so one suite covers all three.
HARNESS="${E2E_HARNESS:-opencode}"

# How the daemon talks to that CLI.
#
#   cli  spawn the harness's own headless mode and read its JSONL (the default).
#   acp  spawn an Agent Client Protocol server, which spawns the harness. A
#        different transport, a different event stream, and — for claude and
#        codex — a different *process*: `npx @agentclientprotocol/…-acp`, not the
#        CLI binary. `tests_acp.sh` is the suite that exercises it.
TRANSPORT="${E2E_TRANSPORT:-cli}"

# Where the ACP servers' npm packages are cached. The Docker image primes this
# at build time and then runs offline out of it; on a developer's box it is
# unset and npx resolves normally.
ACP_NPM_CACHE="${ACP_NPM_CACHE:-}"

# The preset id and the daemon env var holding its (fake) key. The daemon only
# selects a default preset whose key is present, so the variable has to be
# exported into the daemon's environment as well as named in the config.
HARNESS_PRESET_ID="mock-harness"
HARNESS_KEY_ENV="MOCK_LLM_API_KEY"
HARNESS_KEY_VALUE="mock-key"

# Reject an unknown harness up front rather than at the first spawn: a typo would
# otherwise boot the whole stack and fail as "the daemon offers no provider",
# which reads like a detection bug.
case "$HARNESS" in
  opencode | claude | codex) ;;
  *) printf '[e2e] FAIL: unknown E2E_HARNESS=%s (want opencode|claude|codex)\n' "$HARNESS" >&2; exit 1 ;;
esac

case "$TRANSPORT" in
  cli | acp) ;;
  *) printf '[e2e] FAIL: unknown E2E_TRANSPORT=%s (want cli|acp)\n' "$TRANSPORT" >&2; exit 1 ;;
esac

# The env var naming a prebuilt binary for the selected harness.
harness_bin_var() {
  case "$HARNESS" in
    opencode) printf 'OPENCODE_BIN' ;;
    claude) printf 'CLAUDE_BIN' ;;
    codex) printf 'CODEX_BIN' ;;
  esac
}

# Resolve HARNESS_BIN / HARNESS_DIR, or fail naming the override to set.
#
# Also keeps OPENCODE_BIN populated for the opencode leg, because `run-live.sh`
# and the Docker image both speak in terms of that variable.
resolve_harness() {
  local var bin
  var="$(harness_bin_var)"
  bin="${!var:-}"
  [ -n "$bin" ] || bin="$(command -v "$HARNESS" || true)"
  # opencode's installer drops the binary outside PATH for non-login shells.
  if [ -z "$bin" ] && [ "$HARNESS" = "opencode" ] && [ -x "$HOME/.opencode/bin/opencode" ]; then
    bin="$HOME/.opencode/bin/opencode"
  fi
  [ -n "$bin" ] && [ -x "$bin" ] || fail "$HARNESS CLI not found (set $var)"
  HARNESS_BIN="$bin"
  HARNESS_DIR="$(cd "$(dirname "$HARNESS_BIN")" && pwd)"
  [ "$HARNESS" = "opencode" ] && OPENCODE_BIN="$HARNESS_BIN"
  log "harness: $HARNESS → $HARNESS_BIN ($("$HARNESS_BIN" --version 2>/dev/null | head -1))"
}

# Write the configuration that points the harness at the mock LLM on LLM_PORT.
#
# opencode gets its own provider block; claude and codex get a Medulla config
# whose single custom harness preset is marked default, which is what makes an
# untargeted task frame run on it.
write_harness_config() {
  case "$HARNESS" in
    opencode)
      OC_CONFIG="$RUN_DIR/opencode.json"
      sed "s/MOCK_LLM_PORT/$LLM_PORT/" "$SCRIPT_DIR/opencode.json" > "$OC_CONFIG"
      ;;
    claude | codex)
      MEDULLA_CONFIG="$RUN_DIR/medulla.json"
      sed "s/MOCK_LLM_PORT/$LLM_PORT/" "$SCRIPT_DIR/medulla.$HARNESS.json" > "$MEDULLA_CONFIG"
      ;;
  esac
}

# Prepare a private harness HOME under DIR, seeding whatever the CLI expects to
# find there. An optional WORKSPACE is the directory the CLI will be opened in.
#
# Codex is seeded because the stack has no network: `models_cache.json` is
# normally fetched from Codex's own API on first run, and a `codexOverrides`
# preset derives its model catalog from that file (see
# `src/sdk/src/codex_overrides/catalog.rs`). Without it the spawn fails before it
# reaches the mock LLM.
#
# Claude Code is seeded because its *interactive* first run is a wizard — theme
# picker, security notice, folder-trust prompt — and a fresh HOME means a fresh
# first run every time. Headless `-p` runs skip all of it, so this only matters
# to the smoke leg; seeding the answers is what lets that leg assert on the
# editor instead of on an onboarding screen.
harness_seed_home() {
  local home="$1" workspace="${2:-}"
  mkdir -p "$home"
  case "$HARNESS" in
    codex)
      mkdir -p "$home/.codex"
      cp "$SCRIPT_DIR/codex_models_cache.json" "$home/.codex/models_cache.json"
      # Codex's interactive first run in an unknown directory asks whether the
      # contents are trusted. Same story as Claude Code's wizard below: headless
      # runs never see it, the smoke leg would stall on it.
      if [ -n "$workspace" ]; then
        printf '[projects."%s"]\ntrust_level = "trusted"\n' "$workspace" \
          > "$home/.codex/config.toml"
      fi
      # What `codex login --api-key` writes. The ACP server refuses to start a
      # session without it ("Authentication required") even when the routed key
      # is in the environment and the provider block names it — its startup auth
      # check is separate from the provider's credential. A real host that ran
      # `codex login` has this file, so seeding it is the host state, not a
      # workaround for the mock.
      printf '{"auth_mode":"apikey","OPENAI_API_KEY":"%s","tokens":null,"last_refresh":null}\n' \
        "$HARNESS_KEY_VALUE" > "$home/.codex/auth.json"
      chmod 600 "$home/.codex/auth.json"
      ;;
    claude)
      "$PYTHON_BIN" - "$home/.claude.json" "$workspace" \
        "$("$HARNESS_BIN" --version 2>/dev/null | awk '{print $1}')" <<'PY'
import json, sys
path, workspace, version = sys.argv[1], sys.argv[2], sys.argv[3]
state = {
    "hasCompletedOnboarding": True,
    "lastOnboardingVersion": version,
    "theme": "dark",
    # Accepted up front so the leg never has to answer the bypass warning.
    "bypassPermissionsModeAccepted": True,
}
if workspace:
    state["projects"] = {workspace: {"hasTrustDialogAccepted": True}}
json.dump(state, open(path, "w"), indent=2)
PY
      ;;
  esac
}

# Emit the `export` lines a launcher needs to run the harness with HOME.
#
# Only non-secret, non-routing variables: the endpoint and the credential reach
# claude/codex through Medulla's own preset injection, which is the path under
# test and must not be short-circuited here. The fake key is exported because the
# daemon resolves `apiKeyEnv` by name out of its own environment.
harness_env() {
  local home="$1"
  printf 'export HOME=%q\n' "$home"
  printf 'export PATH=%q:$PATH\n' "$HARNESS_DIR"
  harness_transport_env
  case "$HARNESS" in
    opencode)
      printf 'export OPENCODE_CONFIG=%q\n' "$OC_CONFIG"
      printf 'export OPENCODE_DISABLE_AUTOUPDATE=1\n'
      ;;
    claude)
      printf 'export %s=%q\n' "$HARNESS_KEY_ENV" "$HARNESS_KEY_VALUE"
      # Nothing here may reach api.anthropic.com, and an inherited operator
      # credential would silently make a "routed" run a real one.
      printf 'unset ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL\n'
      ;;
    codex)
      printf 'export %s=%q\n' "$HARNESS_KEY_ENV" "$HARNESS_KEY_VALUE"
      printf 'export CODEX_HOME=%q\n' "$home/.codex"
      # The same knobs the preset publishes, but in the daemon's own environment
      # so they also reach the spawns a preset does not cover — the capability
      # probe most of all, which routes through the daemon's `[router]` and would
      # otherwise ask Codex for an uncatalogued model on its own account.
      printf 'export MEDULLA_CODEX_OVERRIDES=1\n'
      printf 'export MEDULLA_CODEX_DISPLAY_NAME=%q\n' "Mock Codex"
      printf 'export MEDULLA_CODEX_CONTEXT_WINDOW=300000\n'
      printf 'export MEDULLA_CODEX_REASONING_EFFORT=low\n'
      printf 'unset OPENAI_API_KEY OPENAI_BASE_URL\n'
      ;;
  esac
}

# The `export` lines that select the transport, if it is not the default.
#
# ACP is chosen by an environment switch (`MEDULLA_HARNESS_PROTOCOL=acp`), which
# is the product's own mechanism rather than a test hook — so a leg that sets it
# runs the same dispatch a fleet host does.
#
# Claude and Codex reach ACP through `npx …-acp@latest`, so a container with no
# network needs npm pointed at a primed cache and told not to look past it.
# npm's cache holds the packument as well as the tarball, so even the `@latest`
# tag resolves out of it. Unset on a developer's box: there, npx resolves the way
# it always does.
harness_transport_env() {
  # `DAEMON_TRANSPORT` lets one scenario boot daemons on both transports against
  # the same mock LLM, which is how `tests_acp.sh` compares them without
  # hardcoding what either client calls itself.
  [ "${DAEMON_TRANSPORT:-$TRANSPORT}" = "acp" ] || return 0
  printf 'export MEDULLA_HARNESS_PROTOCOL=acp\n'
  if [ -n "$ACP_NPM_CACHE" ]; then
    printf 'export NPM_CONFIG_CACHE=%q\n' "$ACP_NPM_CACHE"
    printf 'export NPM_CONFIG_OFFLINE=true\n'
    printf 'export NPM_CONFIG_UPDATE_NOTIFIER=false\n'
  fi
}

# The routing flags every daemon instance needs for this harness.
harness_daemon_routing_flags() {
  case "$HARNESS" in
    opencode) printf '' ;;
    claude | codex) printf -- '--config %q' "$MEDULLA_CONFIG" ;;
  esac
}

# The extra `medulla daemon` flags used by ordinary fleet-style scenarios.
#
# `--config` carries the preset. The permission bypass matches a fleet host;
# the attention-state scenario deliberately uses only the routing flags above
# so it can observe a real Claude permission request.
harness_daemon_flags() {
  local routing
  routing="$(harness_daemon_routing_flags)"
  case "$HARNESS" in
    opencode) printf '%s' "$routing" ;;
    claude | codex) printf -- '%s --dangerously-skip-permissions' "$routing" ;;
  esac
}

# The launcher body that starts this harness's interactive TUI (smoke leg),
# ending in the `exec` that replaces the shell.
#
# The TUI does not read the daemon's preset, so the smoke leg points the CLI at
# the mock itself. That is the one place this file configures a harness directly,
# and it is deliberate: the leg proves tmux can drive the CLI, not that Medulla
# routed it.
harness_tui_launch() {
  case "$HARNESS" in
    opencode) printf 'exec %q\n' "$HARNESS_BIN" ;;
    claude)
      # No `--dangerously-skip-permissions` here: the mock never asks for a
      # tool, and the flag would add a second consent screen to answer.
      printf 'export ANTHROPIC_BASE_URL=%q\nexport ANTHROPIC_AUTH_TOKEN=%q\nexec %q --model mock-model\n' \
        "http://127.0.0.1:$LLM_PORT" "$HARNESS_KEY_VALUE" "$HARNESS_BIN"
      ;;
    codex)
      printf 'export OPENAI_API_KEY=%q\nexec %q %s -m mock-model\n' \
        "$HARNESS_KEY_VALUE" "$HARNESS_BIN" "$(codex_override_args)"
      ;;
  esac
}

# The launcher body for `medulla <harness>` — the operator-facing wrapper, which
# runs the real CLI on a pseudo-terminal and bridges its session to the host
# link. Extra arguments are appended verbatim (`--no-bridge`, typically).
#
# The endpoint env is set here for the same reason `harness_tui_launch` sets it:
# a wrapper session is launched by the operator, not dispatched by a daemon, so
# there is no preset and no spawn seam to route it. What is under test is the
# wrapper's own PTY and bridge, not Medulla's routing.
harness_wrapper_launch() {
  local extra="${1:-}"
  case "$HARNESS" in
    opencode)
      printf 'exec %q opencode %s\n' "$MEDULLA_BIN" "$extra"
      ;;
    claude)
      printf 'export ANTHROPIC_BASE_URL=%q\nexport ANTHROPIC_AUTH_TOKEN=%q\nexec %q claude %s --model mock-model\n' \
        "http://127.0.0.1:$LLM_PORT" "$HARNESS_KEY_VALUE" "$MEDULLA_BIN" "$extra"
      ;;
    codex)
      printf 'export OPENAI_API_KEY=%q\nexec %q codex %s -- %s -m mock-model\n' \
        "$HARNESS_KEY_VALUE" "$MEDULLA_BIN" "$extra" "$(codex_override_args)"
      ;;
  esac
}

# Whether the wrapper bridges this harness's session to the host link.
#
# Claude and Codex write a flat JSONL transcript that the wrapper tails and
# normalizes into typed session events. OpenCode does not — its session log is
# not in that shape — so the wrapper runs it as a passthrough with input
# injection and no transcript bridging. See `src/sdk/src/wrapper/mod.rs`.
harness_wrapper_bridges() {
  [ "$HARNESS" != "opencode" ]
}

# The `-c` overrides a directly-launched Codex needs to reach the mock LLM.
#
# A hand-rolled copy of what `crate::codex_overrides` injects for a preset,
# because the smoke leg launches Codex itself rather than through the daemon.
# The catalog is derived from the same fixture the daemon's derivation reads.
codex_override_args() {
  local catalog="$RUN_DIR/smoke-catalog.json"
  "$PYTHON_BIN" - "$SCRIPT_DIR/codex_models_cache.json" "$catalog" <<'PY'
import json, sys
source, target = sys.argv[1], sys.argv[2]
entry = json.load(open(source))["models"][0]
entry.update({
    "slug": "mock-model",
    "display_name": "Mock Model",
    "description": "Mock model, routed through Codex by the e2e harness.",
    "context_window": 300000,
    "max_context_window": 300000,
    "priority": 1,
    "upgrade": None,
    "availability_nux": None,
    "apply_patch_tool_type": None,
    "tool_mode": None,
    "supports_search_tool": False,
})
json.dump({"models": [entry]}, open(target, "w"), indent=2)
PY
  printf -- '-c model_provider="medulla" -c model_providers.medulla.name="Medulla"'
  printf -- ' -c model_providers.medulla.base_url="http://127.0.0.1:%s/v1"' "$LLM_PORT"
  printf -- ' -c model_providers.medulla.env_key="OPENAI_API_KEY"'
  printf -- ' -c model_providers.medulla.wire_api="responses"'
  printf -- ' -c preferred_auth_method="apikey" -c model_catalog_json=%q' "$catalog"
}

# The label this harness is listed under in Medulla's own menus.
#
# The operator screen's first-run setup asks which coding agent should power the
# worker, and the options are labelled for a human rather than by wire name.
harness_screen_label() {
  case "$HARNESS" in
    opencode) printf 'OpenCode' ;;
    claude) printf 'Claude Code' ;;
    codex) printf 'Codex' ;;
  esac
}

# Whether this leg drives the operator screen in interactive mode — where a peer
# task becomes a live harness session in a pane — rather than headless.
#
# Claude only, and the reason is a real operational trap rather than a gap in
# this harness. Medulla injects its lifecycle hooks into every session it opens,
# and Codex refuses to run hooks it has not been told to trust: the first
# interactive session on a fresh Codex home therefore opens on a "hooks need
# review" prompt, and the worker's attempt to type the peer's task into it fails
# ("the harness never took the prompt") before an operator could answer. The
# trust is keyed by a per-hook content hash — the hooks carry per-run paths and
# session ids — so it cannot be pre-seeded into the fixture the way the model
# catalog and Claude's onboarding answers are.
#
# On a real host the operator answers once and Codex remembers. In CI every run
# is a fresh home, so the codex leg drives the screen headless instead: the
# screen and its dispatch are still covered, only the embedded-pane assertion is
# not.
harness_screen_interactive() {
  [ "$HARNESS" = "claude" ]
}

# Whether this harness's ACP server is a different program from its CLI.
#
# Claude and Codex are reached over ACP through `npx @agentclientprotocol/…-acp`
# — a separate implementation with its own HTTP client, so a request that
# travelled the ACP transport is distinguishable from one that did not. OpenCode
# serves ACP from its own binary (`opencode acp`), so both transports reach the
# model as literally the same client and no such comparison is possible.
harness_acp_is_separate_program() {
  [ "$HARNESS" != "opencode" ]
}

# The model every leg is configured to run.
#
# One name for all three harnesses: the opencode provider block, both Medulla
# presets, and the mock LLM's own catalog all use it, so a scenario asserting
# "the run used the configured model" has one thing to compare against.
harness_model() {
  printf 'mock-model'
}

# The `kind` the mock LLM logs this harness's completion requests under.
#
# Asserting on it rather than on "any completion" is what proves the task really
# travelled the dialect this harness is supposed to speak — a claude leg that
# somehow answered over chat-completions would be a routing bug, not a pass.
harness_llm_kind() {
  case "$HARNESS" in
    opencode) printf 'chat' ;;
    claude) printf 'messages' ;;
    codex) printf 'responses' ;;
  esac
}

# A provider this daemon is guaranteed *not* to offer.
#
# The daemon is booted with `--providers $HARNESS`, so the unavailable-provider
# error path has to name something else — hardcoding one would silently become a
# happy path on the leg that runs it.
harness_absent_provider() {
  case "$HARNESS" in
    claude) printf 'codex' ;;
    *) printf 'claude' ;;
  esac
}

# The pane text that says this harness's TUI is ready for input.
harness_tui_ready_regex() {
  case "$HARNESS" in
    # Each pattern is the composer's own placeholder text, which appears only
    # once the editor is accepting input — not the splash or a consent screen.
    opencode) printf 'Ask anything' ;;
    claude) printf 'for shortcuts|Try "' ;;
    codex) printf 'Improve documentation in|to change' ;;
  esac
}

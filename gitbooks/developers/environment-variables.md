---
description: >-
  An index of every environment variable Medulla reads, what it does, and its
  default.
---

# Environment variables

Every variable listed here is read somewhere in
[`src/`](https://github.com/tinyhumansai/medulla-src/tree/main/src). A `.env` file in
the current directory is loaded at startup before anything reads the
environment, and it never overrides a variable already set in the process, so
these can be set either way. Truthy values are `1` and `true`, case-insensitive.

## Home, account, and config

| Variable | What it does | Default |
| --- | --- | --- |
| `MEDULLA_HOME` | Overrides the Medulla root, the directory holding one subdirectory per account. | `~/.medulla`, or `./.medulla` under `MEDULLA_DEV` |
| `MEDULLA_DEV` | Truthy makes the root `./.medulla`, relative to the cwd. | unset |
| `MEDULLA_USER` | Selects the account for one process, ahead of the `active_user.toml` marker and without changing it. `local` reaches the pre-login home. | the marker, else `local` |
| `MEDULLA_CONFIG_PATH` | The `--config` path a subprocess should use. Set by the parent at startup so a spawned MCP tool server or ACP harness inherits the same config file instead of rediscovering one from its own cwd. | unset |
| `MEDULLA_STATE_DIR` | Where local state is written. Beats an explicit `stateDir` in config. | `<home>/state` |
| `MEDULLA_LOG_DIR` | Where log files are written. | `<home>/logs` |

## Backend and authentication

| Variable | What it does | Default |
| --- | --- | --- |
| `MEDULLA_API_URL` | Backend base URL. Beats config-file `backend.baseUrl` and the built-in default. | unset |
| `MEDULLA_STAGING` | Truthy flips the built-in default base URL from production to staging. | unset |
| `MEDULLA_TOKEN` | The bearer JWT, named by the default `backend.tokenEnv`. Config can point `tokenEnv` at a different variable. | unset |

## Halves of the process

| Variable | What it does | Default |
| --- | --- | --- |
| `MEDULLA_HOST` | `0` orchestrates only, this machine runs nothing; `1` forces hosting on. Beats the `[host].enabled` config key for one run. | the config value |
| `MEDULLA_HUB` | `0` disables the orchestrator uplink to the backend, leaving the host half; `1` is the redundant explicit opt-in. | on |
| `MEDULLA_HUB_POLL_MS` | The hub's poll interval, in milliseconds. | `1500` |
| `MEDULLA_HUB_WORKERS` | Pre-seeds the worker roster as a comma-separated `id=address` list (a bare token is used as both). | unset |
| `MEDULLA_LINK_PEER` | Pre-seeds a single worker by address, when `MEDULLA_HUB_WORKERS` is not set. | unset |
| `MEDULLA_WORKER_PROVIDER` | The harness recorded on workers seeded from the two variables above. | `claude` |
| `MEDULLA_DEMO_FLEET` | Truthy stands in a small fake fleet (two hosts, three harnesses, two workspaces, two agents, two templates) so every fleet surface can be exercised with no backend. It is the last fallback; any real capacity wins. | unset |

## Harness selection and transport

`<P>` is the uppercased provider: `CLAUDE`, `CODEX`, or `OPENCODE`. A
per-provider key always beats the generic `MEDULLA_HARNESS_*` key, which beats
the owner fallbacks and provider defaults. Within each tier the `MEDULLA_*` name
wins and the deprecated `TINYPLACE_*` spelling of the same name is read directly
behind it, so hosts configured before the rename keep working.

| Variable | What it does | Default |
| --- | --- | --- |
| `MEDULLA_HARNESS_PROTOCOL` | `acp` makes the daemon talk to harnesses over the Agent Client Protocol instead of the legacy provider JSONL. | unset (legacy JSONL) |
| `MEDULLA_HARNESS_TRANSPORT` | `app-server` selects the shared-process Codex path for a caller with no frame to state a flavor on. | unset |
| `MEDULLA_<P>_BIN` | Overrides the provider binary. Claude also honours the legacy `TINYVERSE_CLAUDE_BIN`. Treated as untrusted configuration: an overridden binary is withheld the fleet grant. | `claude`, `codex`, `opencode` |
| `MEDULLA_SHELL_BIN` | The shell the Sessions picker offers first. Falls back to `$SHELL`, then `sh`. | `$SHELL` |
| `MEDULLA_<P>_ARGS` | Extra arguments prepended to the child argv, whitespace-split. | none |
| `MEDULLA_<P>_DM_TO`, `MEDULLA_HARNESS_DM_TO` | The owner a wrapped session forwards envelopes to, and by default receives input from. Falls back to `MEDULLA_OPENHUMAN_OWNER` and then `OPENHUMAN_OWNER_AGENT`. | unset |
| `MEDULLA_<P>_RECEIVE_FROM`, `MEDULLA_HARNESS_RECEIVE_FROM` | The peer whose inbound frames are injected as input. Falls back to the DM recipient. | the DM recipient |
| `MEDULLA_<P>_RECEIVE`, `MEDULLA_HARNESS_RECEIVE` | `0` disables inbound input injection. | enabled |
| `MEDULLA_<P>_SESSIONS_DIR`, `MEDULLA_HARNESS_SESSIONS_DIR` | Where the wrapper looks for the harness's session transcript files. | the provider's own location |
| `MEDULLA_<P>_SESSION_POLL_MS` | Wrapper session-file poll interval, in milliseconds. | `500` |
| `MEDULLA_<P>_RECEIVE_POLL_MS` | Inbound-receive poll interval, in milliseconds. | `1500` |
| `MEDULLA_<P>_STATUS_HEARTBEAT_MS` | Status-heartbeat re-emit interval, in milliseconds. | `15000` |
| `MEDULLA_<P>_STATUS_IDLE_MS` | Silence before a session is reported idle, in milliseconds. | `30000` |
| `MEDULLA_<P>_DISPLAY_NAME` | Human-readable name shown by the harness for the routed model. | unset |
| `MEDULLA_OPENHUMAN_OWNER` | The owner fallback behind the `DM_TO` keys. | unset |
| `MEDULLA_LINK_OWNER` | The highest-precedence owner name for the daemon's onboarding. | unset |

## Codex config overrides

| Variable | What it does | Default |
| --- | --- | --- |
| `MEDULLA_CODEX_OVERRIDES` | Set by a preset that wants Medulla's generated Codex `-c` overrides. Anything but empty or `0` enables them. | unset |
| `MEDULLA_CODEX_REASONING_EFFORT` | The `model_reasoning_effort` declared to Codex. | unset |
| `MEDULLA_CODEX_CONTEXT_WINDOW` | The context window, in tokens, declared to Codex for the routed model. | unset |

## Inference proxy and routing

| Variable | What it does | Default |
| --- | --- | --- |
| `MEDULLA_PROXY_TOKEN` | The child environment variable naming the loopback token the [attribution proxy](attribution-and-routing.md) mints. Resolved at the spawn seam like any other `secret_env` name. | set per spawn |
| `MEDULLA_OPENROUTER_URL` | Overrides the upstream root the proxy forwards to. A test seam; operators should not normally set it. | `https://openrouter.ai/api` |
| `OPENROUTER_API_KEY` | The default OpenRouter credential a preset with no `apiKeyEnv` uses. Scrubbed from the harness's environment when the proxy is in front. | unset |

The `[router]` section injects `ANTHROPIC_BASE_URL` and `ANTHROPIC_AUTH_TOKEN`
into a Claude spawn, and `OPENAI_BASE_URL` and `OPENAI_API_KEY` into a Codex or
OpenCode spawn. Those are written by Medulla rather than read from your
environment, and the key is resolved from whatever variable `apiKeyEnv` names.

## Control socket, MCP, and hooks

These are written by Medulla onto the processes it spawns rather than set by an
operator. They are listed so a spawn's environment is readable.

| Variable | What it does | Default |
| --- | --- | --- |
| `MEDULLA_CONTROL_SOCKET` | An explicit control socket path. | a derived path |
| `MEDULLA_MCP_SOCKET` | The control socket a spawned MCP tool server may reach. Set by Medulla on that subprocess, never inherited from the ambient environment. | set per spawn |
| `MEDULLA_MCP_GRANT` | The grant token for that socket. The token is the authority: everything its holder may do is looked up server-side from it. | set per spawn |
| `MEDULLA_HOOK_SOCKET` | The control socket a launched harness's built-in hook commands may reach. Written into the harness's own environment, which is safe only because the token below carries no authority beyond attributing a report to its own session. | set per spawn |
| `MEDULLA_HOOK_GRANT` | The hook-only grant token for that socket. | set per spawn |
| `MEDULLA_FLEET_DEPTH` | A task's depth in the dispatch tree, read when minting the grant for the harness that runs it. Absent means depth zero, work an operator started. Not a security boundary on its own; the value it produces is written into a server-side grant. | `0` |
| `MEDULLA_MCP_COMMAND` | The binary a spawned tool server runs. Untrusted configuration: whatever it names is executed, so it is honoured only when it points at an existing file, and it never widens what a session is served. | this process's own path |
| `MEDULLA_ORIGIN_SESSION` | The harness session a tool server is serving, stamped onto every workflow run that session starts. An attribution hint, never an authorization. | set per registration |

## Workflows

| Variable | What it does | Default |
| --- | --- | --- |
| `MEDULLA_WORKFLOW_TOOLS` | How much of the `workflow_*` tool surface a spawned MCP server serves: `full`, `propose` (read, reason, note, propose; never write or run a graph), or `run` (read a workflow and run it). An unrecognised value fails closed to `propose`. | `full` |
| `MEDULLA_WORKFLOW_SCOPE` | Restricts evolution writes to the workflow being reviewed. | unset |
| `MEDULLA_INPUT` | The path to the JSON input file a workflow `code` node's script is handed. Set by the engine; a workflow's own `env` declaration wins over it. | set per run |

## Attribution

| Variable | What it does | Default |
| --- | --- | --- |
| `MEDULLA_ATTRIBUTION` | The `Co-authored-by` trailer line the commit-message hook reads at runtime. | set per spawn |
| `MEDULLA_GIT_CONFIG_BASE_COUNT` | How many `GIT_CONFIG_*` pairs came from the parent, so the shim drops only the one Medulla appended when resolving the repository's real hooks directory. | set per spawn |
| `MEDULLA_GIT_CONFIG_BASE_PARAMETERS` | The caller's legacy inline Git config before Medulla appends its hook path. | set per spawn |

## Updates

| Variable | What it does | Default |
| --- | --- | --- |
| `MEDULLA_NO_UPDATE_CHECK` | Truthy disables the background release check. The env kill-switch for the `update.check` config key. | unset |
| `MEDULLA_UPDATE_URL` | Overrides the release manifest URL. A test seam, also honoured by the install scripts. | the published `latest.json` |

## Read by the install scripts only

| Variable | What it does | Default |
| --- | --- | --- |
| `MEDULLA_HOME` | Install prefix. | `~/.medulla` |
| `MEDULLA_NO_MODIFY_PATH=1` | Do not touch shell profiles or the user `PATH`. | unset |
| `MEDULLA_UPDATE_URL` | Override the release manifest URL. | the published manifest |

## Read by the test and end-to-end harnesses only

These are not product configuration. They appear here so a harness environment is
readable.

| Variable | What it does |
| --- | --- |
| `MEDULLA_LINK_FORWARDER` | The forwarder address the coordination owner driver enrolls against. |
| `MEDULLA_LINK_STATE_DIR` | The link identity directory that driver uses. |
| `MEDULLA_LINK_HOME_<name>`, `MEDULLA_LINK_OWNER_DIR_<name>` | Provisioned identity directories for the live suite. |
| `MEDULLA_LIVE_COPILOT` | Opt-in for the live copilot suite, which otherwise skips loudly rather than starting a harness session nobody asked to pay for. |
| `E2E_LIVE`, `E2E_ALLOW_PROD`, `E2E_KEEP`, `E2E_SMOKE`, `LIVE_MODEL` | Live and offline end-to-end harness gates. See [Testing](testing.md). |
| `MEDULLA_BIN`, `FORWARDER_BIN`, `OWNER_BIN`, `OPENCODE_BIN` | Prebuilt binary overrides for the coordination harness. |
| `MOCK_LLM_MARKER`, `MOCK_LLM_MODEL`, `MOCK_LLM_PORT`, `MOCK_LLM_LOG` | Mock LLM knobs for the coordination harness. |

## Inert

| Variable | Status |
| --- | --- |
| `MEDULLA_CORE_SOCKET` | Named the external `medulla-serve` NDJSON socket before the core was embedded. `medulla run` rejects the matching `--core-socket` flag with that explanation, and a `[core]` config section is inert. |
| `TINYPLACE_*` | The deprecated spelling of the harness knobs above. Still read, directly behind the `MEDULLA_*` name in each tier. |

## Read next

* [Configuration](configuration.md): the config file these variables layer over.
* [Troubleshooting](troubleshooting.md): what a wrongly-set variable looks like.
* [Attribution and routing](attribution-and-routing.md): the proxy's variables in context.

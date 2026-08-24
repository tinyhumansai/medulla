---
description: >-
  Where tests live, what the shared stand-ins provide, how to run the offline and
  live suites, and the coverage gate.
---

# Testing

Every suite that runs by default is offline and deterministic. Nothing reaches a
network, and no real coding-agent CLI is spawned unless a test writes one itself.

## Where tests live

| Kind | Location |
| --- | --- |
| Unit tests for a directory module | `foo/tests.rs`, declared from `foo/mod.rs` with `#[cfg(test)] mod tests;` |
| Unit tests for a single-file leaf module | a sibling `foo_tests.rs`, until the module becomes a directory |
| Cross-module and end-to-end tests for the SDK | [`src/sdk/tests/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/tests/) |
| Cross-module and end-to-end tests for the app crate | [`src/tui/tests/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/tui/tests/) |
| Shared stand-ins | [`src/sdk/tests/support/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/tests/support/) |

Integration files are named by the behavior they cover, not by the module they
touch: `e2e_daemon.rs`, `feature_workers.rs`, `e2e_login.rs`. The `e2e_` prefix
marks a suite that drives a whole path (a spawned binary, a real PTY, a socket);
`feature_` marks one that exercises a feature's logic in-process.

The app crate's tests reach the SDK's support helpers with `#[path]`, so there is
one copy of each stand-in rather than two that drift.

## The shared stand-ins

Everything under `src/sdk/tests/support/` exists so a suite can exercise real
code against something that is not real infrastructure.

| File | What it stands in for |
| --- | --- |
| `mock_backend.rs` | The Medulla backend's HTTP and SSE API. Speaks enough HTTP/1.1 for `medulla::client::MedullaClient`: session create, list, detail, messages, abort, and a scripted SSE stream tests drive with `emit` and `close_stream` so the client's reconnect and `Last-Event-ID` replay are exercised. Every request is recorded for assertions. |
| `fake_provider.rs` | A coding-agent CLI, as small executable shell scripts in a self-cleaning temp dir that emit realistic provider JSONL (claude `stream-json`, codex `exec --json`, opencode `run --format json`). The daemon's real spawn path runs them through the `MEDULLA_*_BIN` overrides. |
| `mock_harness.rs`, `mock_harness_types.rs`, `mock_harness_script.rs`, `mock_harness_helpers.rs` | The richer successor to `fake_provider`: a `MockCli` builder that renders a `/bin/sh` script emitting the exact streaming-JSONL shapes the daemon mappers parse. A mock is a sequence of `Step`s (thinking, agent messages, tool call and result pairs, provider errors, garbage lines) plus a `Terminal` behavior (clean exit, non-zero exit with a stderr tail, or hang-until-killed for the idle watchdog), so one scenario replays against any of the three providers. |
| `fake_app_server.rs` | `codex app-server`. A small Python program speaking the same line-framed JSON-RPC: it answers `initialize`, `thread/start` and `thread/resume`, and on `turn/start` replays a canned notification script. It records every request it received, so a test can assert how many processes were spawned and whether two lanes shared one. |
| `mock_openrouter.rs` | OpenRouter. Binds `127.0.0.1:0`, records exactly what reached it (including chunked request bodies), and can answer with a deliberately slow SSE stream. Recording the received request is the point: the [attribution rewrite](attribution-and-routing.md) is only meaningful observed from the far side of the proxy. |
| `mod.rs` | Wires the modules together and holds `wait_until`, a polling helper that panics with a label on timeout. |

## Running the offline suites

```sh
cargo test                                  # unit, feature, and e2e suites for both crates
cargo clippy --all-targets -- -D warnings
cargo fmt --check
make ci                                     # the complete gate: boundary, fmt, clippy, test, build
```

Run all of them before handing work off. See
[Contributing](contributing.md#validate).

## The coordination end-to-end harness

A separate harness under `e2e/coordination/` drives real processes: the `medulla`
daemon binary, a real coding CLI, and an interactive TUI, over the
[host link](host-link-protocol.md), with no real keys and no network egress.

```
owner driver (src/sdk/examples/coordination_owner; a real medulla-link endpoint)
  → mock link forwarder (src/sdk/examples/mock_link_forwarder.rs; blind UDP)
    → medulla daemon (real binary, --providers <harness>, the host end)
      → the real coding CLI (spawned by the daemon as its provider)
        → mock LLM (e2e/coordination/mock_llm.py)
          → deterministic reply "COORDINATION_OK <echo of task>"
  ← Reply frame back over the link, asserted on content, usage, and delivery
```

### When CI runs them

Every pull request and every push to `main`, as a three-way matrix — one leg per
coding CLI, each running all five suites in `--network none` containers.

They spent a while gated behind the release workflow instead, because they were
the slowest check by a wide margin and dominated the wait on changes that could
not affect them. Both halves of that have been addressed: the coding CLIs now
arrive with a pinned base image rather than being downloaded per build, so a run
pays for the Rust stage and nothing else; and an `e2e-relevant` job diffs the
pull request against its base, so one that touches only documentation skips the
matrix entirely. A push to `main` always runs it, since `main` is what a release
builds from.

Gating a release on them was the wrong moment anyway: a harness regression
surfaced at release time is blocking a ship, when it could have blocked the pull
request that caused it.

### The two images

The harness image is built from two pieces, and the split is what keeps CI fast:

| Image | Holds | Rebuilt when |
| --- | --- | --- |
| `ghcr.io/tinyhumansai/medulla_e2e_base` | tmux, python3, node, opencode + claude + codex, the primed ACP npm cache, the unprivileged `e2e` user | a CLI version is bumped |
| `medulla-e2e` (local) | this checkout's `medulla` binary, the two link examples, the harness scripts | the source changes |

The CLIs are roughly 600 MB of downloads that have nothing to do with the source,
so a run whose layer cache had been evicted used to re-fetch all of them before
it could compile a line of Rust. They now live in a pinned image that CI pulls.

The base is tagged by the versions it contains
(`oc1.17.18-cc2.1.226-cx0.147.0-node22`), and `Dockerfile` pins that tag rather
than `latest` — a base moving under a green branch would leave the harness
testing something other than what it last reported. Bumping a CLI is therefore
three steps: change the `ARG` in `Dockerfile.base`, publish (the **Build E2E
Image** workflow, or `make e2e-image-base` with `PUSH=1`), and point
`Dockerfile`'s `BASE_IMAGE` default at the new tag.

Without GHCR access, build the base locally and pass it in:

```sh
make e2e-image-base
BASE_IMAGE=ghcr.io/tinyhumansai/medulla_e2e_base:latest \
  bash e2e/coordination/build-image.sh
```

### Choosing the coding CLI

`E2E_HARNESS` picks which CLI the daemon spawns — `opencode` (the default),
`claude` or `codex` — and every suite runs unchanged against all three. The
per-harness knowledge lives in one file, `e2e/coordination/harness.sh`.

The three differ in how they are pointed at the mock, and the difference is the
point:

| Harness | How Medulla routes it | Wire dialect |
| --- | --- | --- |
| `opencode` | its own provider block (`opencode.json`) | OpenAI chat completions |
| `claude` | a **custom harness preset** → `ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN` at the spawn seam | Anthropic Messages |
| `codex` | the same preset plus `codexOverrides` — the `-c` provider block and a derived model catalog, because a bare `OPENAI_BASE_URL` is ignored by Codex | OpenAI Responses |

So the claude and codex legs exercise Medulla's real preset-routing path
(`src/sdk/src/config/custom_harnesses.rs`, `src/sdk/src/codex_overrides/`); the
only fake in the chain is the model behind the endpoint. Each leg also asserts
that the task arrived on the dialect that harness is *supposed* to speak, so a
run that quietly fell back to another wire fails rather than passes.

Two fixtures exist because the stack has no network. `codex_models_cache.json`
stands in for the catalog Codex normally fetches on first run and a
`codexOverrides` preset derives its model entry from. Claude Code's interactive
first run is a wizard (theme, security notice, folder trust), so `harness.sh`
seeds the answers into a scratch `~/.claude.json` — headless runs never see it,
the TUI smoke leg would otherwise stall on it.

Only the transport's middle is mocked, and it is mocked as a blind box: the
forwarder authenticates the cleartext header with each node's forwarder key and
copies the ChaCha20-Poly1305 payload verbatim. Both endpoints run the real
`medulla-link` crate, so every byte of payload encryption, every state diff and
every retransmission is production code.

| File | Role |
| --- | --- |
| `e2e/coordination/lib.sh` | Shared boot, teardown, and assert helpers; harness-agnostic. |
| `e2e/coordination/harness.sh` | Everything specific to one coding CLI: binary, config, spawn env, TUI shape. |
| `e2e/coordination/run.sh` | Happy-path round trip plus a TUI smoke leg. |
| `e2e/coordination/tests.sh` | Five functional scenarios on top of `lib.sh`. |
| `e2e/coordination/tests_multi.sh` | Five multi-agent scenarios: two daemons, two workspaces. |
| `e2e/coordination/tests_acp.sh` | Four ACP-transport scenarios: the daemon spawns an ACP server instead of the CLI. |
| `e2e/coordination/tests_tui.sh` | Three terminal scenarios: the `medulla <harness>` wrapper and the operator screen. |
| `e2e/coordination/run-live.sh` | The same fleet against real staging and OpenRouter (opencode only). |
| `e2e/coordination/mock_llm.py` | Entrypoint for the mock LLM. |
| `e2e/coordination/mockllm/` | The mock itself, one module per wire dialect (chat, messages, responses). |
| `e2e/coordination/opencode.json` | opencode config template pointed at the mock LLM, `autoupdate: false`. |
| `e2e/coordination/medulla.claude.json`, `medulla.codex.json` | Daemon configs carrying the custom harness preset for each CLI. |
| `e2e/coordination/codex_models_cache.json` | Fixture stand-in for Codex's normally-fetched model catalog. |
| `e2e/coordination/Dockerfile.base` | The tools image: tmux, python3, node and all three coding CLIs, pinned. Published to GHCR. |
| `e2e/coordination/Dockerfile` | The harness image: a Rust build stage layered onto that base. |
| `e2e/coordination/build-image.sh` | Build (and optionally push) either image. |
| `e2e/coordination/run-docker.sh` | Build and run the whole harness in a container. |

```sh
bash e2e/coordination/run.sh          # happy path + TUI smoke leg
bash e2e/coordination/tests.sh        # 5 functional scenarios
bash e2e/coordination/tests_multi.sh  # 5 multi-agent scenarios
bash e2e/coordination/tests_acp.sh    # 4 ACP-transport scenarios
bash e2e/coordination/tests_tui.sh    # 3 terminal scenarios (wrapper + operator screen)
bash e2e/coordination/run-docker.sh   # the same, inside docker
make e2e-image                        # build the container image
make e2e-docker                       # build the image, then run all three offline suites
make e2e-docker E2E_HARNESS=claude    # the same suites, driving Claude Code
make e2e-docker-all                   # every suite against every coding CLI
make e2e-image-base                   # rebuild the tools base (only for a CLI bump)

E2E_HARNESS=codex bash e2e/coordination/run.sh   # a single leg on the host
```

Optional knobs:

| Variable | Effect |
| --- | --- |
| `E2E_KEEP=1` | Keep the run directory, tmux session, and container for debugging. |
| `E2E_SMOKE=0` | Skip the interactive TUI leg. |
| `E2E_HARNESS` | Which coding CLI to drive: `opencode` (default), `claude`, `codex`. |
| `E2E_TRANSPORT` | How the daemon reaches it: `cli` (default) or `acp`. `tests_acp.sh` sets it per daemon. |
| `MEDULLA_BIN`, `FORWARDER_BIN`, `OWNER_BIN` | Prebuilt binary overrides; unset means `cargo build --release`. The docker image bakes all three. |
| `OPENCODE_BIN`, `CLAUDE_BIN`, `CODEX_BIN` | Coding-CLI overrides; unset means `$PATH`. The docker image bakes all three. |
| `IMAGE=`, `NO_CACHE=1`, `NET=host` | Docker knobs; the default runtime is `--network none`. |
| `MOCK_LLM_MARKER`, `MOCK_LLM_MODEL`, `MOCK_LLM_PORT`, `MOCK_LLM_LOG` | Mock LLM knobs. |

### The ACP transport suite

`MEDULLA_HARNESS_PROTOCOL=acp` switches the daemon from spawning the harness's
own headless mode to spawning an **Agent Client Protocol server**, which then
spawns the harness. For claude and codex that is a different program entirely —
`npx @agentclientprotocol/…-acp`, not the CLI binary — so everything the CLI
seam does at spawn time (select the preset's model, point the run at the routed
endpoint, carry the operator's hooks) has to be done again by other means.

`tests_acp.sh` boots two daemons against one mock LLM, one per transport, and
compares them:

| Scenario | Asserts |
| --- | --- |
| round trip | A task dispatched over ACP comes back with the marker. |
| client identity | The ACP leg's request reached the mock from a *different client* than the CLI leg's — proof the switch took effect. Skipped for opencode, whose ACP server is its own binary. |
| routed model | The ACP leg's request names the preset's model, not the harness's default. |
| transport parity | Both legs answer the same task the same way. |

Client identity is the load-bearing one. A reply frame is identical whichever
transport produced it, so a suite that asserted only on the reply would pass
through an ACP switch that was silently ignored — which is exactly what happened
twice: Codex's routed provider block was passed on the argv, and `codex-acp`
parses argv only for its `login` and `cli` subcommands. In server mode it reads
`CODEX_CONFIG` and `MODEL_PROVIDER` from the environment and ignores the rest, so
the session opened on the operator's own account and default model while the
preset's endpoint sat unused beside it.

Note that ACP runs report no token usage. ACP's `usage_update` carries the
context window (`used` of `size`), not an input/output split, so there is
nothing to fill `usage` with that would not be invented — an orchestrator that
bills or throttles on reported tokens gets nothing from an ACP run.

### The terminal suite

`tests_tui.sh` covers the two surfaces an operator actually looks at, both
full-screen TUIs on a real pseudo-terminal, driven under tmux:

| Scenario | Asserts |
| --- | --- |
| wrapper transparency | `medulla <harness> --no-bridge` paints the real CLI's own TUI and answers a prompt typed into it. |
| wrapper bridging | An enrolled wrapper answers Medulla's worker-setup wizard, then forwards its session to the orchestrator — state datagrams leave the wrapper's own node. Claude and Codex only; the wrapper does not tail opencode transcripts. |
| operator screen | `medulla daemon --tui` answers its setup menus, paints its worker screen, and still serves a dispatched task. For claude the task is also asserted to appear in the live embedded session pane. |

These break in ways headless tests cannot see: a harness TUI refuses to start
with a pipe on stdin (Codex says "stdin is not a terminal"), so the wrapper
allocates a PTY, and whether it did is only observable in what the terminal
painted.

Two limitations are worth knowing, because both are real rather than artefacts
of the harness. Codex's screen leg runs **headless**: Medulla injects lifecycle
hooks into every session, Codex refuses to run hooks it has not been told to
trust, and the trust is keyed by a per-hook content hash — so a fresh Codex home
opens the first interactive session on a "hooks need review" prompt that the
worker's typing attempt cannot get past. On a real host the operator answers
once; in CI every run is a fresh home. And the wrapper's opencode leg skips the
bridging scenario, because opencode's session log is not the flat JSONL the
wrapper tails.

### The multi-agent suite

The daemon is one workspace per process: `RunTaskOptions.cwd` comes from
`config.workspace`, and nothing on the wire overrides it per task. A fleet is
therefore N daemon processes, not one daemon with N directories.
`tests_multi.sh` boots two (`alpha` and `beta`), each with its own workspace,
`MEDULLA_HOME`, and separately enrolled link identity, against one shared
forwarder and mock LLM.

| Scenario | Asserts |
| --- | --- |
| fleet registration | Two daemons serve as distinct enrolled hosts on one forwarder. |
| workspace binding | Each reports its own `cwd`; sentinels prove each read only its own directory. |
| concurrent routing | Two parallel legs each get their own marker back, with no cross-talk. |
| crash containment | Killing `beta` mid-task leaves `alpha` serving. |
| crash recovery | Restarting `beta` comes back on the same enrolled identity and serves again. |

Workspace binding is the subtle one. Asserting the reported `cwd` only proves the
daemon says the right thing. To prove it read the right directory,
`make_workspace` plants a unique sentinel in each workspace's `AGENTS.md`, which
`dir_context` folds into the capabilities probe prompt, and the suite asserts
against `MOCK_LLM_LOG` that both sentinels reached the LLM and never co-occurred
in a single request. Co-occurrence would mean a daemon read outside its own
workspace. The daemons share a filesystem, so this is a behavioural guarantee,
not an enforced one; enforcing it would take separate containers with separate
volumes.

### What a green round trip asserts

1. Output leg: the terminal frame is `kind == "Reply"` and contains the marker.
   For `tests.sh`, `usage.inputTokens` and `outputTokens` must also be present.
2. Input leg: the mock LLM journal contains at least one completion request embedding
   the task text, on the wire dialect the selected harness speaks.
3. Transport leg: the forwarder log shows at least one state-carrying datagram in
   each direction between that pair's two node ids.

## The live suite

`run-live.sh` runs the same fleet and the same assertions in spirit, with the two
mocks swapped for real infrastructure: a deployed forwarder and OpenRouter. It
exists because a green mocked suite can still break against real infrastructure.

```sh
E2E_LIVE=1 OPENROUTER_API_KEY=sk-or-... make e2e-live
```

It fails closed on every axis, so it cannot start by accident:

| Variable | Requirement |
| --- | --- |
| `E2E_LIVE=1` | Required; the deliberate opt-in. |
| `OPENROUTER_API_KEY` | Required; billed per token. |
| `MEDULLA_STAGING=1` | The default. Targeting production additionally needs `E2E_ALLOW_PROD=1`. |
| `LIVE_MODEL` | Defaults to a cheap small model. |
| `MEDULLA_LINK_FORWARDER` | A deployed forwarder implementing section 5 of the [link protocol](host-link-protocol.md#5-forwarder-rules). |
| `MEDULLA_LINK_HOME_<name>`, `MEDULLA_LINK_OWNER_DIR_<name>` | Provisioned `node.json` identity directories. |

Two prerequisites do not exist in this repository, so the suite does not run here
and says so rather than pretending. The transport half needs the deployed
forwarder, and it needs identities from
[enrollment](host-link-protocol.md#72-enroll-token-and-forwarder-key); there is
no enrollment client here, so each end's `node.json` has to be provisioned out of
band and handed in. Without them `preflight` exits 2 with that explanation. The
scenarios are maintained so they can run the moment that infrastructure exists.

Two assertions necessarily change shape against a real model. Routing is asserted
on `taskId` correlation rather than a verbatim marker echo, because a real model
will not parrot a marker back. Capabilities asserts that the model populated
`tools`, which proves the probe round-tripped through OpenRouter instead of
falling back to the local digest.

The rendered opencode config holds a live API key, so it is written mode `600`.
Identity directories are symlinked, never copied: `node.json` holds the sequence
reservation of [section 3.1](host-link-protocol.md#31-seq), and two processes
advancing two copies of one counter under one pair key reuses AEAD nonces. This
target is deliberately not part of `make ci`.

## Coverage

Coverage uses [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov),
which needs `cargo install cargo-llvm-cov` and
`rustup component add llvm-tools-preview`:

```sh
cargo llvm-cov                                # run the suite with coverage, print a summary
cargo llvm-cov report --show-missing-lines
```

CI gates line coverage at 80% (`--fail-under-lines 80`). The gate's
`--ignore-filename-regex` drops the vendored tree and the process, terminal, and
network drivers themselves: the TUI's `app_loop`, `commands`, `main`,
`event_loop`, `terminal`, `hub_relay`, `worker_loop`, `harness_pty`, and
onboarding run modules; the SDK's daemon entry and transport, wrapper run and
bridge, and hub socket, relay, boot and handle modules; and the `#[ignore]`d
interactive-codex live suite. Those depend on OS terminal state or a live
connection and are exercised by the end-to-end suites instead, while their pure
state machines, routing, and data types remain counted.

`vendor/` path dependencies are local packages under the workspace root, so
`cargo-llvm-cov`'s default registry filter does not drop them; that is why the
regex starts with `(^|/)vendor/`. See [Vendoring](vendoring.md#coverage-and-the-vendored-tree).

## Read next

* [Contributing](contributing.md): the full development loop and release process.
* [Architecture](architecture.md): what the suites are testing.
* [Troubleshooting](troubleshooting.md): when a run fails rather than a test.

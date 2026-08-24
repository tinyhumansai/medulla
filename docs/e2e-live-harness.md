# E2E Live Harnesses: docker + tmux + opencode

How the coordination e2e suite drives **real processes** — the `medulla` daemon, the
real `opencode` CLI, and an interactive TUI — deterministically and offline, and how
to build more suites like it. Written for agents: every pattern here was needed to
make the suite green, and each is transferable.

## What the suite proves

One end-to-end round trip over the **host link** (`docs/host-link-protocol.md`),
with no real keys and no network egress:

```
owner driver (examples/coordination_owner.rs; a real medulla-link endpoint)
  → mock link forwarder (examples/mock_link_forwarder.rs; blind UDP, §5)
    → medulla daemon (real binary, `--providers opencode`, the host end)
      → real opencode CLI (spawned by the daemon as its provider)
        → mock OpenAI-compatible LLM (e2e/coordination/mock_llm.py)
          → deterministic reply "COORDINATION_OK <echo of task>"
  ← Reply frame back over the link, asserted on content + usage + delivery
```

Only the transport's *middle* is mocked, and it is mocked as a blind box: the
forwarder authenticates the 58-byte cleartext header with each node's forwarder
key and copies the ChaCha20-Poly1305 payload verbatim. Both endpoints run the
real `medulla-link` crate, so every byte of payload encryption, every state
diff and every retransmission is production code.

A second tmux window drives an **interactive opencode TUI** with `send-keys` /
`capture-pane` against the same mock LLM, proving tmux controls opencode as well as
medulla.

## Layout

| File | Role |
| --- | --- |
| `e2e/coordination/lib.sh` | shared boot/teardown/assert helpers (the harness kernel) |
| `e2e/coordination/run.sh` | happy-path round trip + TUI smoke leg; exit 0 on PASS |
| `e2e/coordination/tests.sh` | 5 functional scenarios on top of `lib.sh` |
| `e2e/coordination/tests_multi.sh` | 5 multi-agent scenarios: two daemons, two workspaces |
| `e2e/coordination/run-live.sh` | the same fleet against real staging + OpenRouter |
| `e2e/coordination/mock_llm.py` | stdlib-only OpenAI-compatible mock (SSE + unary) |
| `e2e/coordination/opencode.json` | opencode config template → mock LLM; `autoupdate: false` |
| `e2e/coordination/opencode.live.json` | opencode config template → OpenRouter (live suite) |
| `e2e/coordination/Dockerfile` | multi-stage image: rust build stage → slim runtime |
| `e2e/coordination/run-docker.sh` | build + run the whole harness in a container |
| `examples/mock_link_forwarder.rs` | blind UDP forwarder implementing protocol §5 rules 1–8 |
| `examples/coordination_owner.rs` | owner-side driver: enrolls pairs, serves legs, prints terminal frame JSON |

## Running

```sh
bash e2e/coordination/run.sh          # happy path + TUI smoke leg (~1-2 min)
bash e2e/coordination/tests.sh        # 5 functional scenarios (~40s + boots)
bash e2e/coordination/tests_multi.sh  # 5 multi-agent scenarios (~30s + boots)
bash e2e/coordination/run-docker.sh   # the same, inside Linux/arm64 docker
make e2e-docker                       # build the image, then run all three offline suites
```

Knobs (all optional):

- `E2E_KEEP=1` — keep the run dir + tmux session (and container) for debugging.
- `E2E_SMOKE=0` — skip the interactive TUI leg.
- `MEDULLA_BIN` / `FORWARDER_BIN` / `OWNER_BIN` / `OPENCODE_BIN` — prebuilt binary
  overrides; unset means `cargo build --release` (the docker image bakes all four).
- Docker: `IMAGE=`, `NO_CACHE=1`, `NET=host` (default is `--network none`).
- Mock LLM: `MOCK_LLM_MARKER`, `MOCK_LLM_MODEL`, `MOCK_LLM_PORT`, `MOCK_LLM_LOG`.

## The multi-agent suite (`tests_multi.sh`)

The daemon is **one workspace per process** — `RunTaskOptions.cwd` comes from
`config.workspace`, and nothing on the wire overrides it per task. So a fleet is
N daemon processes, not one daemon with N directories. `tests_multi.sh` boots two
(`alpha`, `beta`), each with its own workspace, `MEDULLA_HOME` and separately
enrolled link identity, against **one shared** forwarder and mock LLM:

```
mock forwarder ──┬── daemon alpha (work-alpha) ── opencode ──┐
                 └── daemon beta  (work-beta)  ── opencode ──┴─→ mock LLM
```

| Scenario | Asserts |
| --- | --- |
| fleet registration | two daemons serve as *distinct* enrolled hosts on one forwarder |
| workspace binding | each reports its own `cwd`; sentinels prove each read only its own dir |
| concurrent routing | two parallel legs each get their own marker back, no cross-talk |
| crash containment | killing `beta` mid-task leaves `alpha` serving |
| crash recovery | restarting `beta` comes back on the *same* enrolled identity and serves again |

Workspace binding is the subtle one. Asserting the reported `cwd` only proves the
daemon *says* the right thing. To prove it *read* the right directory,
`make_workspace` plants a unique sentinel in each workspace's `AGENTS.md` —
which `dir_context` folds into the capabilities probe prompt — and the suite then
asserts against `MOCK_LLM_LOG` that both sentinels reached the LLM and **never
co-occurred in a single request**. Co-occurrence would mean a daemon read outside
its own workspace.

Note the daemons share a filesystem, so this is a *behavioural* guarantee (the
daemon stays in its workspace), not an enforced one. Enforcing it would take
separate containers with separate volumes.

## The live suite (`run-live.sh`) — currently unavailable

Same fleet, same assertions in spirit, with the two mocks swapped for real
infrastructure — a deployed forwarder and OpenRouter. It exists because a green
mocked suite can still break against real infrastructure.

**It cannot run today, and says so instead of pretending.** The transport half
needs two things this repository does not have: a deployed forwarder implementing
§5 (`MEDULLA_LINK_FORWARDER=<host:port>`), and identities from §7.2 enrollment —
for which there is no client here, so each end's `node.json` has to be
provisioned out of band and handed in via `MEDULLA_LINK_HOME_<name>` /
`MEDULLA_LINK_OWNER_DIR_<name>`. Without them `preflight` exits 2 with that
explanation. The scenarios are kept because they are what should run the moment
it is possible; they have never been run against real infrastructure.

Two assertions necessarily change shape, because a real model does not behave like
the echo mock:

- **Routing** is asserted on `taskId` correlation rather than a verbatim marker
  echo — a real model will not parrot `TASKALPHA-123` back.
- **Capabilities** asserts the model populated `tools`, which proves the probe
  round-tripped through OpenRouter instead of falling back to the local digest.

It fails closed on every axis, because a live suite that runs by accident is worse
than one that is annoying to start:

```sh
E2E_LIVE=1 OPENROUTER_API_KEY=sk-or-… make e2e-live
```

- `E2E_LIVE=1` — required; the deliberate opt-in.
- `OPENROUTER_API_KEY` — required; billed per token.
- `MEDULLA_STAGING=1` — the default. Targeting production additionally needs
  `E2E_ALLOW_PROD=1`.
- `LIVE_MODEL` — defaults to a cheap small model.
- `MEDULLA_LINK_FORWARDER`, `MEDULLA_LINK_HOME_<name>`,
  `MEDULLA_LINK_OWNER_DIR_<name>` — the transport prerequisites above.

The rendered opencode config holds a live API key, so it is written mode `600`.
Identity directories are symlinked, never copied: `node.json` holds the sequence
reservation of §3.1, and two processes advancing two copies of one counter under
one pair key reuses AEAD nonces. This target is deliberately **not** part of
`make ci`.

## The patterns that made it work

These are the load-bearing decisions. Reuse them when building a live harness for
any TUI/daemon/CLI combination.

### 1. tmux is the process supervisor *and* the TUI driver

Every process gets its own tmux window via `launch <name>` (lib.sh): the window runs
a generated launcher script whose stdout/stderr go to `$RUN_DIR/<name>.log` and whose
exit code lands in `$RUN_DIR/<name>.rc`. That gives you three observation channels
per process:

- the **log file** — greppable, used by `wait_for_regex` for readiness;
- the **rc file** — its *existence* means the process exited (poll it, don't `wait`);
- the **pane** — `tmux capture-pane -p -t "$SESSION:<name>"` for anything that paints
  a TUI instead of writing lines (this is the only way to assert on opencode's UI).

Interactive driving is `tmux send-keys -t "$SESSION:smoke" "the prompt"` then a
separate `send-keys ... Enter` — send text and Enter as two calls; some TUIs drop a
trailing `C-m` glued onto pasted text.

### 2. Never sleep — poll for readiness markers

Every wait in the harness is a poll with a deadline (`wait_for_regex file regex
timeout`), keyed on a printed marker:

- servers print `listening on …` at startup (see below);
- the daemon prints `serving providers ... as <name> for <owner> ...` — the
  harness scrapes the advertised *name* out of that line; the worker's node id
  comes from enrollment, because only ids travel on the wire (§2);
- the TUI leg polls `capture-pane` for the editor prompt (`Ask anything`) before
  typing, and for the reply marker after. The original fixed `sleep 8` was the #1
  source of flakiness; readiness polling fixed it.

### 3. Ephemeral ports, printed and scraped

The mock LLM binds port 0 and prints the real address; the harness scrapes it from
the log. No fixed ports → suites can run concurrently and never collide with dev
servers. Config that needs the port (opencode.json) is a template with a
`MOCK_LLM_PORT` placeholder substituted per run with `sed`.

The forwarder is the exception, and the reason is instructive: its address is
written into every `node.json` at enrollment (§7.3) and the daemon has no flag to
override it, so the address must be known *before* anything starts. The harness
picks a free port itself (`free_port`), releases it, and lets the forwarder bind
it a moment later; a lost race fails the bind loudly rather than pointing
endpoints at nothing.

### 4. Mock the LLM at the HTTP boundary, deterministically

opencode supports any OpenAI-compatible endpoint via config, so the mock is ~190
lines of stdlib Python implementing just `GET /v1/models` and
`POST /v1/chat/completions` (unary + SSE). Two properties matter:

- **Deterministic, assertable output**: every completion is
  `COORDINATION_OK <echo of last user message>`. The marker is unique enough to grep
  through any layer (TUI pane, encrypted reply, logs); the echo proves the *task
  text* traversed the chain, not just any request. `MOCK_LLM_MARKER` makes the
  marker itself an injectable test vector.
- **A request journal**: every request appends one JSON line to `llm.jsonl`.
  Assertions then check the *input* side too ("the task text appeared in a chat
  request"), not only the visible output.

Gotcha: with a threaded single-request-per-connection server, force
`Connection: close` on every response — the AI SDK's keep-alive pooling can
deadlock a thread-per-request mock.

### 5. Give mocks a read surface — but only over what they may see

The forwarder logs one line per datagram it moves: source, destination, sequence,
size, and heartbeat-or-not. `assert_bidirectional_delivery` counts those lines per
direction, which turns "did both legs deliver?" into two greps.

It stops exactly where the mock's knowledge stops. A blind forwarder cannot tell
a task frame from an acknowledgement, so the assertion is "state-carrying
datagrams crossed in both directions" and not "a reply frame came back" — that
part is asserted from the owner's terminal-frame JSON instead. A debug surface
that knew more would be a mock that is lying about its own blindness.

### 6. Hermetic HOME + kill auto-update

The real opencode CLI will **auto-update itself mid-test** and paint a blocking
"restart" dialog over the TUI (this broke the first baseline run, and the updated
binary leaked into `~/.opencode`). Defense in depth:

- `"autoupdate": false` in the opencode config template;
- `OPENCODE_DISABLE_AUTOUPDATE=1` in every launcher env;
- `HOME=$RUN_DIR/ochome` so caches/state/any surviving update land in the ephemeral
  run dir, not the user's real home;
- the TUI leg still dismisses unexpected `update complete / restart` dialogs as a
  safety net.

Generalize this: any real third-party CLI in a harness needs its updater disabled,
its HOME sandboxed, and its version printed into the log (`opencode --version`) so
drift is visible in failures.

### 7. Owner drivers print machine-readable terminal frames

`coordination_owner` prints its terminal frame as one JSON line
(kind/text/usage/frameKinds/ownerId) and writes the same JSON plus an exit code to
`<label>.json` / `<label>.rc`; assertions are tiny `python3 -c` / heredoc scripts
over that JSON, not brittle greps over prose logs. It supports `--kind
capabilities`, `--provider`, and per-leg `--task-id`/`--timeout-ms`. A leg
"exits" 1 on a terminal Error frame or a timeout; `run_owner` captures the rc +
JSON without failing the suite, so error-path scenarios assert on the Error frame
instead of dying on the nonzero exit.

### 7b. One long-lived endpoint per peer, not one process per leg

The owner used to be a process per leg. Over the host link it must not be: SSP
state lives in memory, so an owner that exits and comes back is at state 0 while
the daemon still holds state *n*, and from then on neither side's diffs apply —
the link is wedged with no error anywhere. So the harness boots one
`coordination_owner --serve` per enrolled pair and each leg is a request file
dropped into its queue (one argument per line, written under a temporary name and
renamed into place so a half-written request is never read).

The same asymmetry bites when a *host* restarts, which the crash-recovery
scenario does deliberately. There is no in-band way for either end to discover
that its peer lost its state, so the harness — which knows, because it did the
killing — calls `reset_owner_link <name>` between killing a daemon and starting
its replacement. That rebuilds the orchestrator's link while nothing is
listening, so frames the old session was still retransmitting cannot land on the
new process. Worth knowing outside the harness too: a real orchestrator needs
some answer here, and today the answer is "restart both ends".

### 8. Scenario suites share one booted stack when isolation allows

Booting the stack (signal → llm → daemon, which spawns opencode) dominates
wall-clock. `tests.sh` runs four scenarios against one stack — capabilities probe,
token-usage propagation, second round trip from a fresh owner identity,
unavailable-provider error path — and boots a fresh stack only for the scenario that
must change boot-time state (a custom `MOCK_LLM_MARKER`). Factor your harness into
`boot_*` helpers first (lib.sh), then scenarios become ~15 lines each.

### 9. Docker wraps the *same* script — no second harness

The image bakes prebuilt binaries (multi-stage: `rust:1.96-bookworm` build →
`debian:bookworm-slim` + tmux + python3 + a pinned opencode release tarball) and sets
the `*_BIN` env overrides; the container entry just runs `run.sh` unchanged. One
source of truth for the test logic; docker is only an environment.

- Runtime is `--network none` by default and passes — proof the harness is fully
  loopback. Network is needed at build time only.
- Build natively (`--platform linux/$(uname -m ...)`) — never force amd64 emulation
  on Apple Silicon; qemu makes Rust builds and TUIs slow and flaky.
- Pin the third-party CLI version in the Dockerfile; download with
  `curl --http1.1 --retry 5 --retry-all-errors -C -` (large release tarballs hit
  HTTP/2 protocol errors surprisingly often).
- Copy every tree rustc will read: the workspace keeps examples and tests under
  `src/`, so `COPY src ./src` covers them (and `src/link`, which the examples link
  against).
- `.dockerignore` aggressively (25 GB `target/`, `.git`, unused vendor trees) but
  keep path-dependency sources from `Cargo.toml`.

### 10. Diagnostics on failure, cleanup on success

`fail()` dumps the tail of every log, the owner JSON, the LLM journal, and a
`capture-pane` of every window before exiting — a failed CI run is debuggable from
its output alone. The EXIT trap kills the tmux session and removes the run dir
unless `E2E_KEEP=1`. Sessions are named `medulla-e2e-$$` so stragglers are findable
(`tmux ls | grep medulla-e2e`).

## Assertion checklist for a full round trip

A green run asserts all three legs, not just the final answer:

1. **Output leg** — terminal frame is `kind == "Reply"` and contains the marker
   (and for `tests.sh`: `usage.inputTokens/outputTokens` present — the regression
   guard for opencode's nested `tokens:{input,output}` usage shape).
2. **Input leg** — the mock LLM journal contains ≥1 chat request embedding the
   task text.
3. **Transport leg** — the forwarder log shows ≥1 state-carrying datagram in
   *each* direction between that pair's two node ids.

## Known caveats

- Timing bounds: TUI editor-ready poll 60 s, reply render 120 s, owner leg up to
  220 s. Ample locally (~6 s observed readiness) but they are wall-clock bounds on a
  loaded CI box.
- The `Ask anything` readiness string and the update-dialog text are opencode-UI
  coupling; a future opencode redesign moves them. The version is printed at the top
  of every run for exactly this reason.
- Token usage *values* from the mock are 0 (opencode doesn't map the mock's
  `prompt_tokens`); the assertion is presence/propagation, not magnitude.
- Pre-existing failure unrelated to this suite: `cargo test` shows
  `e2e_daemon_providers::stdin_input_reaches_child_and_echoes_in_reply` failing at
  HEAD before this work (opencode stdin-echo drift); tracked separately.

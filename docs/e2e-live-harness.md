# E2E Live Harnesses: docker + tmux + opencode

How the coordination e2e suite drives **real processes** — the `medulla` daemon, the
real `opencode` CLI, and an interactive TUI — deterministically and offline, and how
to build more suites like it. Written for agents: every pattern here was needed to
make the suite green, and each is transferable.

## What the suite proves

One encrypted round trip through the entire stack, with no real keys and no network
egress:

```
owner driver (examples/coordination_owner.rs)
  → mock tiny.place Signal server (examples/mock_signal_server.rs; real X3DH/double-ratchet)
    → medulla daemon (real binary, `--providers opencode`)
      → real opencode CLI (spawned by the daemon as its provider)
        → mock OpenAI-compatible LLM (e2e/coordination/mock_llm.py)
          → deterministic reply "COORDINATION_OK <echo of task>"
  ← encrypted Reply frame back to the owner, asserted on content + usage + delivery
```

A second tmux window drives an **interactive opencode TUI** with `send-keys` /
`capture-pane` against the same mock LLM, proving tmux controls opencode as well as
medulla.

## Layout

| File | Role |
| --- | --- |
| `e2e/coordination/lib.sh` | shared boot/teardown/assert helpers (the harness kernel) |
| `e2e/coordination/run.sh` | happy-path round trip + TUI smoke leg; exit 0 on PASS |
| `e2e/coordination/tests.sh` | 5 functional scenarios on top of `lib.sh` |
| `e2e/coordination/mock_llm.py` | stdlib-only OpenAI-compatible mock (SSE + unary) |
| `e2e/coordination/opencode.json` | opencode config template → mock LLM; `autoupdate: false` |
| `e2e/coordination/Dockerfile` | multi-stage image: rust build stage → slim runtime |
| `e2e/coordination/run-docker.sh` | build + run the whole harness in a container |
| `examples/mock_signal_server.rs` | runnable mock Signal server with `/debug/stored` |
| `examples/coordination_owner.rs` | owner-side driver; prints terminal frame JSON |

## Running

```sh
bash e2e/coordination/run.sh          # happy path + TUI smoke leg (~1-2 min)
bash e2e/coordination/tests.sh        # 5 functional scenarios (~40s + boots)
bash e2e/coordination/run-docker.sh   # the same, inside Linux/arm64 docker
```

Knobs (all optional):

- `E2E_KEEP=1` — keep the run dir + tmux session (and container) for debugging.
- `E2E_SMOKE=0` — skip the interactive TUI leg.
- `MEDULLA_BIN` / `MOCK_SIGNAL_BIN` / `OWNER_BIN` / `OPENCODE_BIN` — prebuilt binary
  overrides; unset means `cargo build --release` (the docker image bakes all four).
- Docker: `IMAGE=`, `NO_CACHE=1`, `NET=host` (default is `--network none`).
- Mock LLM: `MOCK_LLM_MARKER`, `MOCK_LLM_MODEL`, `MOCK_LLM_PORT`, `MOCK_LLM_LOG`.

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

- servers print `listening on http://127.0.0.1:<port>` at startup (see below);
- the daemon prints `serving providers ... as <agent-id> on ...` — the harness
  *scrapes the worker id out of that log line* rather than configuring one;
- the TUI leg polls `capture-pane` for the editor prompt (`Ask anything`) before
  typing, and for the reply marker after. The original fixed `sleep 8` was the #1
  source of flakiness; readiness polling fixed it.

### 3. Ephemeral ports, printed and scraped

Every mock binds port 0 and prints the real address; the harness scrapes it from the
log. No fixed ports → suites can run concurrently and never collide with dev
servers. Config that needs the port (opencode.json) is a template with a
`MOCK_LLM_PORT` placeholder substituted per run with `sed`.

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

### 5. Give mocks a debug read surface

The mock Signal server exposes `GET /debug/stored?to=<agent>` returning envelope
counts. That turns "did the encrypted leg actually deliver in both directions?" into
two curls. When you write a mock, add a read-only debug endpoint for whatever the
tests will want to count.

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
(kind/text/usage/frameKinds/ownerId); assertions are tiny `python3 -c` / heredoc
scripts over that JSON, not brittle greps over prose logs. It also supports
`--kind capabilities`, `--provider`, and per-run `--task-id`/`--timeout-ms`. The
binary itself exits 1 on a terminal Error frame or timeout; it is the `run_owner`
shell helper that captures the rc + JSON without failing the suite, so error-path
scenarios can assert on the Error frame instead of dying on the nonzero exit.

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
- Mind `#[path]` includes: `examples/mock_signal_server.rs` pulls a module from
  `tests/support/`, so the build stage must `COPY tests ./tests` too. Copy every
  tree rustc will read, not just `src/`.
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
3. **Transport leg** — `/debug/stored` shows ≥1 envelope in *each* direction.

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

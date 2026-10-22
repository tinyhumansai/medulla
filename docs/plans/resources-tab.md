# Plan: Resources tab with a btop-style Agents pane

> **Status: Implemented.** The UI and shared sampler landed through
> [PR #58](https://github.com/senamakel/medulla/pull/58),
> [PR #59](https://github.com/senamakel/medulla/pull/59),
> [PR #60](https://github.com/senamakel/medulla/pull/60),
> [PR #61](https://github.com/senamakel/medulla/pull/61), and its Windows test
> follow-up [PR #62](https://github.com/senamakel/medulla/pull/62). The user
> guide is [Resources](../../gitbooks/features/resources.md).

## Problem

Medulla has resource snippets in the Sessions view, but no host-wide history, no dedicated overview of CPU/memory/network activity, and no inventory of the coding-agent processes running on a host. The Resources tab will bring those views together: familiar btop-shaped resource boxes, with an Agents pane that classifies Medulla sessions and externally started coding agents. The tab must remain honest about platform data gaps and must not block drawing while it samples processes.

This is a design contract for issues [#2–#12](https://github.com/senamakel/medulla/issues/2), not a claim that every field is available on every OS. `ship` means implement in this epic when the listed source can provide it; `later` means retain a clean model/UI seam but do not fake a value; `never` means intentionally out of product scope.

## What was verified before writing this

Code facts were checked at Medulla commit [`080bb3745e4cc0a4fe9a9d14967e2164cdf4e47c`](https://github.com/tinyhumansai/medulla/commit/080bb3745e4cc0a4fe9a9d14967e2164cdf4e47c):

- `src/tui/src/ui/app/types/model.rs:71-86` has separate full/slim `TABS`; Resources must be added to both. `src/tui/src/ui/app/render/mod.rs:247-293,334-349` owns shortcut hints and tab rendering. `src/tui/src/ui/app/state.rs:576-610` owns tab-entry commands.
- `src/tui/src/ui/app/keys/mod.rs:155-335` dispatches Ctrl-C/O/Y/X/N/T globally before page handlers; Tab/BackTab are global; bare arrows are claimed by Sessions only; PageUp/PageDown have Sessions-only behavior. No global plain `r`, `e`, `f`, `/`, `t`, `k`, `s`, `p`, `1`–`4`, `+`, or `-` binding exists there. An attached harness owns every key (`render/mod.rs:252-267`).
- `src/sdk/src/protocol/frames/types.rs:87-118` defines Claude, Codex, OpenCode, in-process OpenHuman and shell provider kinds. `src/sdk/src/protocol/env.rs:193-204` gives `openhuman-core` and `sh` as their default binaries. `src/sdk/src/daemon/providers/acp/execution.rs:461-490` constructs Claude via `npx -y @agentclientprotocol/claude-agent-acp@latest`, Codex via `npx -y @agentclientprotocol/codex-acp@latest`, and OpenCode via its configured binary plus `acp`; OpenHuman is not an ACP child.
- The existing session sampler is draw-triggered, rate-limited to `REFRESH_INTERVAL`, refreshes memory/process CPU/memory/disk usage, and explicitly skips Linux tasks/threads (`src/tui/src/ui/resources/session/monitor.rs:42-77`). The existing dependency is `sysinfo 0.37.2` with `disk,system` features (`Cargo.toml:87`).
- `RemoteMessage::Sessions` is documented as a latest-wins whole-list update because it changes rarely and is small (`src/sdk/src/protocol/remote/types.rs:208-216`). `RemoteSessionRow` currently carries session identity and state but no process usage (`:53-110`); capabilities are exchanged once in `RemoteCapabilities` (`:133-171`).
- Theme config uses optional camelCase values and defaults missing fields (`src/sdk/src/config/types/presentation.rs:5-38`). Host resource reporting already has a small `HostResources` model (`src/sdk/src/runtime/fleet/mod.rs:62-81`).
- The open issue `tinyhumansai/medulla-src#320` proposes a typed action registry and key overrides, but it is not landed in this checkout; there is no registry API to target yet.

The btop reference is upstream [`aristocratos/btop` commit `89fcdf4094e89f8ac215ee6c3ee74aeffb618e86`](https://github.com/aristocratos/btop/commit/89fcdf4094e89f8ac215ee6c3ee74aeffb618e86). Its [`calcSizes`](https://github.com/aristocratos/btop/blob/89fcdf4094e89f8ac215ee6c3ee74aeffb618e86/src/btop_draw.cpp#L2217-L2410) supplies the layout reference; its [default config](https://github.com/aristocratos/btop/blob/89fcdf4094e89f8ac215ee6c3ee74aeffb618e86/src/btop_config.cpp) documents preset syntax and defaults. The following adapts the layout and interactions, not btop's process-list purpose or platform-specific collector implementation.

## Design

### 1. Parity decisions

A cell names whether the implementation ships, is deferred, or is excluded on that OS. All values unavailable from a collector are rendered as unavailable/omitted; never substitute zero. GPU is later on every platform.

| Box | Element | Linux | macOS | Windows |
|---|---|---|---|---|
| CPU | Upper and lower graphs of total CPU usage | ship | ship | ship |
| CPU | Selectable upper/lower fields: total, user, nice, system, idle, iowait, irq, softirq, steal, guest, guest_nice | ship (`/proc/stat` deltas) | later (sysinfo does not expose this field breakdown) | later (sysinfo does not expose this field breakdown) |
| CPU | Per-core usage rows/mini-graphs | ship (sysinfo) | ship (sysinfo) | ship (sysinfo) |
| CPU | Frequency and CPU brand | ship (sysinfo; Linux frequency where reported) | ship (sysinfo where reported; otherwise omit frequency) | ship (sysinfo where reported; otherwise omit frequency) |
| CPU | Package temperature | ship (hwmon labels and sensors) | later (no verified portable mapping in this spec) | later (no verified portable mapping in this spec) |
| CPU | Per-core temperatures | ship (hwmon label/core mapping; omit unmapped sensors) | later (no verified portable mapping) | later (no verified portable mapping) |
| CPU | Load average | ship (sysinfo) | ship (sysinfo) | later (no portable load-average API verified here) |
| CPU | Uptime | ship (sysinfo) | ship (sysinfo) | ship (sysinfo) |
| CPU | Battery | never (not a host-CPU field; excluded from this tab) | never | never |
| Mem | RAM used / available / cached / free | ship (used, available, free from sysinfo; cached Linux-only) | ship (used, available, free; cached omitted) | ship (used, available, free; cached omitted) |
| Mem | Swap used / available / free | ship (sysinfo) | ship (sysinfo) | ship (sysinfo) |
| Mem | Mounted disks: used / free / total | ship (sysinfo disk list) | ship (sysinfo disk list) | ship (sysinfo disk list) |
| Mem | Per-disk read/write throughput and activity | ship (sysinfo deltas; `/proc/diskstats` if needed) | later (source and semantics need verification) | later (source and semantics need verification) |
| Mem | IO display mode (combined or split read/write) | ship | ship when disk IO ships | ship when disk IO ships |
| Net | Per-interface download/upload rate graphs | ship (sysinfo network deltas) | ship (sysinfo network deltas) | ship (sysinfo network deltas) |
| Net | Autoscale, zero offset, synchronized scales | ship (pure btop-style scaler) | ship | ship |
| Net | Cumulative download/upload totals | ship (sysinfo counters) | ship | ship |
| Net | Interface selection and up/down state | ship (sysinfo interface list; hide unusable interfaces) | ship (same) | ship (same) |
| Net | Interface IP address | ship when sysinfo exposes it; otherwise omit | ship when sysinfo exposes it; otherwise omit | ship when sysinfo exposes it; otherwise omit |
| Agents | Classified process tree, usage and history | ship | ship where OS exposes process metadata; unknown argv stays unclassified | ship where OS exposes process metadata; unknown argv stays unclassified |
| GPU | GPU box/telemetry | later | later | later |

`ship` in the CPU frequency row means display the reported frequency when present, not promise all machines expose it. Linux CPU field breakdown and hwmon temperatures are Linux-specific implementation paths. A platform gap never makes a row disappear if other supported fields are available.

### 2. Layout, resize and presets

Use terminal content area (excluding tab chrome) for percentages, round each percentage up, then clamp against available rects. Reference shares follow btop `calcSizes`: CPU is full width and 32% high (minimum 60×8); with all boxes visible, Mem is 45% wide and 40% high, Net 45% wide and 28% high, Agents 55% wide and 68% high (minimum 44×16). The complementary width belongs to Agents. `cpuBottom`, `memBelowNet`, and `agentsLeft` change only placement. If Agents is hidden, Mem and Net use full width; if only CPU is shown, CPU uses full content height. Keep the requested shares, but give rounding remainder to the right/bottom pane so there are no gaps.

A shown box below its own minimum is not partially drawn: show a centered `Terminal too small for Resources` message with the current required minimum. When the combined arrangement does not fit, preserve CPU first, then Agents, then Mem/Net; suppress lower-priority boxes as whole panels and include their toggles in the hint. At a terminal smaller than CPU's 60×8 minimum, show only the message. Never panic or create negative rects. Resizing recomputes all rects and retains box visibility, sort/filter, and selected row by stable identity.

Presets use btop's string shape: whitespace separates presets; commas separate entries; each entry is `<box>:<position>:<graph>`. Medulla box names are `cpu`, `mem`, `net`, `agents`; position is `0` or `1`; graph is `default`, `braille`, `block`, or `tty`. Example: `cpu:0:default,mem:0:default,net:0:default,agents:1:braille cpu:0:block,agents:0:tty`. Preset 0 is always the default all-box layout; up to 9 user presets follow. Reject unknown boxes, position outside 0/1, graph symbols outside the enum, duplicate box names, or empty presets; keep last valid config on parse failure.

### 3. Agents pane: identity, rows and actions

`AgentKind` is `Claude`, `Codex`, `CodexAppServer`, `ClaudeAcp`, `CodexAcp`, `Opencode`, `OpenhumanCore`, `EmbeddedOpenhuman`, or `Shell`. Match only exact executable basenames and conservative argv signatures:

| Kind | Verified match signature |
|---|---|
| Claude | exe basename `claude` |
| Codex | exe basename `codex`, unless argv has the app-server signature |
| CodexAppServer | exe basename `codex` and argv tokens `app-server` |
| ClaudeAcp | argv contains `@agentclientprotocol/claude-agent-acp` or `claude-agent-acp`; allow `npx` wrapper as ancestor |
| CodexAcp | argv contains `@agentclientprotocol/codex-acp` or `codex-acp`; allow `npx` wrapper as ancestor |
| Opencode | exe basename `opencode` |
| OpenhumanCore | exe basename `openhuman-core` |
| EmbeddedOpenhuman | SDK registry says an OpenHuman turn is executing in-process; never infer it from the medulla process name |
| Shell | exe basename is a recognized shell (`sh`, `bash`, `zsh`, `fish`, `pwsh`, `powershell`) and it is a known Medulla session or an explicitly shown external shell |

For `env` wrappers and other launchers, walk descendants and classify the deepest verified harness process; collapse the wrapper chain into that row. Do not classify a generic `node` from its executable alone. Preserve raw `exe`/argv for classification only; display a bounded, redacted command line (strip environment assignments and known secret-bearing arguments). If argv is unreadable, report `Unknown` / `external` with unavailable command, never guess a kind.

Origins are `Session{id}`, `Task{id}`, `Headless`, `Remote{host}`, `External`. Attribution uses registered `(pid, startTime)` roots first, then known PTY session roots with the same start-time guard; a mismatched start time is PID reuse and must not attribute. The nearest matching owned root wins. Collapse each launcher subtree to one agent row; show descendant processes as children only in tree mode, and exclude Medulla itself and its non-agent tool children from External. A direct child belonging to EmbeddedOpenhuman is nested under one synthetic EmbeddedOpenhuman row. De-duplicate remote processes against local ids only within their host namespace.

Columns are `Pid · Agent (kind) · Origin · Workspace · Threads · Mem · Cpu% · CPU history · Runtime`. On narrow terminals, remove Workspace then Threads then history label (keep the graph); never remove kind/origin. Sort keys are pid, kind/program, origin, workspace, threads, memory, CPU, runtime. Initial sort is CPU descending, stable tie-break by pid then start time. Left/Right cycles sort keys; `r` reverses. `e` toggles tree; `f` opens a literal substring filter; `/` opens a regex filter, with leading `!` negating either filter. Invalid regex leaves the previous filter active and shows an error. Up/Down and PgUp/PgDn move selection; Home/End jump. Preserve selection by `(host,pid,startTime)` across samples.

Enter on a row attributed to a local/remote Medulla Session focuses that session on Sessions tab; Enter on an external/headless/task/embedded row opens the Agents detail view. `F` follows its owning session when one exists; otherwise show a disabled-action status. `t`, `k`, `s` request SIGTERM, SIGKILL, or SIGSTOP/SIGCONT on supported Unix systems. Every signal opens a confirmation showing kind, PID, command, origin, signal, and target host. External targets require a second explicit confirmation. Never signal a process not owned by the current user; never signal remote processes in this epic; Windows signal actions are disabled. Cancel is the default. This explicit policy avoids a TUI keypress killing an unrelated or remotely owned process.

### 4. Remote reporting and grouping

Each remote daemon adds an optional, additive `usage` to each `RemoteSessionRow`: classified kind, remote PID, CPU%, RSS, thread count, start time, and at most 32 CPU history samples. This reports per-session process usage only; do not send host-wide totals or external process inventory. The client groups rows under a non-selectable host heading and labels their origin `Remote{host}`. Older clients ignore the extra serde field; newer clients treat missing usage as unavailable. Do not put argv, environment, command text, or workspace paths beyond the session's existing `cwd` on the wire.

Send on the existing latest-wins Sessions update, no faster than `updateMs` (default 2000 ms), and only when a row or its usage sample changes. Include usage fields only for sessions with a classified process; cap serialized usage to 1 KiB per session and 16 KiB total per host update, dropping oldest history points first. Do not advertise a new `RemoteCapabilities` flag: the optional field is additive and clients can render its absence. Before implementation, measure actual serialized worst-case lists and host-link datagram fragmentation; if the existing message can exceed the link channel budget, split telemetry into a separate latest-wins channel rather than fragment a datagram.

### 5. Keys and conflict check

Bindings are scoped to the Resources tab and are checked against `src/tui/src/ui/app/keys/mod.rs` at the verified commit above. Global Ctrl-C/O/Y/X/N/T and Tab/BackTab retain precedence. No Resources action binds those chords. Sessions-only arrows and page-scroll keys do not conflict off that tab. A Resources handler runs before generic selection handling; it must never capture keys while an interactive harness has focus.

| Key | Resources action | Conflict result |
|---|---|---|
| `1`–`4` | Toggle CPU, Mem, Net, Agents | Free in global dispatcher; settings numeric navigation is tab-scoped |
| `p` / `P` | Previous / next preset | Free globally; Agents filter captures only while its editor is open |
| `+` / `-` | Increase / decrease update interval by 100 ms | Free globally; clamp 100–10000 ms |
| `←` / `→` | Cycle Agents sort key when Agents list focused | No global binding; use component focus so layout/preset border actions do not steal it |
| `r` | Reverse sort | Free globally; Resources-only |
| `e` | Toggle process tree | Free globally; Sessions rail's Enter behavior is distinct |
| `f` / `/` | Open literal / regex filter editor | Free globally; `/` is literal except this explicit editor action |
| `↑` / `↓`, `PgUp` / `PgDn`, `Home` / `End` | Select Agents row | Global generic movement is only fallback; Resources handler consumes on its focused list |
| `Enter` | Focus owning session or open agent detail | Existing tab entry only changes tabs; selection is page-owned |
| `F` | Follow attributed session | Free globally; uppercase required |
| `t` / `k` / `s` | Confirmed terminate / kill / stop-continue | Free globally in keys/mod.rs; no signal before confirmation |
| `Esc` | Close detail/filter/confirmation, then return to tab | Page-local overlay precedence; never exits app |
| `Tab` / `BackTab` | Change top-level tab | Reserved global; Resources does not intercept |
| `Ctrl-C/O/Y/X/N/T` | Existing global actions | Reserved; never reused |

Issue medulla-src#320 has not landed at this checkout, so no stable action IDs or registry API can be named yet. When it lands, register each binding with Resources-page scope and its current default; retain registry collision validation as authoritative. If registry scope cannot express focus/overlay precedence, defer configurable overrides for this tab until it can; do not invent a conflicting parallel key system.

### 6. Configuration and theme

Add optional `[resources]` with camelCase serde keys. Missing section/keys use these defaults; unknown keys retain normal config behavior.

| Key | Type | Default |
|---|---|---|
| `updateMs` | integer, clamp 100–10000 | `2000` |
| `shownBoxes` | string | `"cpu,mem,net,agents"` |
| `presets` | string | `"cpu:0:default,mem:0:default,net:0:default,agents:1:default"` |
| `cpuBottom` | bool | `false` |
| `memBelowNet` | bool | `false` |
| `agentsLeft` | bool | `false` |
| `cpuGraphUpper` / `cpuGraphLower` | enum field selector | `"total"` / `"total"` |
| `cpuInvertLower` / `cpuSingleGraph` | bool | `true` / `false` |
| `tempScale` | `"celsius"` / `"fahrenheit"` | `"celsius"` |
| `freqMode` | `"current"` / `"average"` / `"max"` | `"current"` |
| `showBattery` | bool (ignored; field is never) | `false` |
| `memDisks` / `memIo` / `memIoMode` | bool / bool / `"combined"` or `"split"` | `true` / `false` / `"combined"` |
| `netAuto` / `netSync` | bool | `true` / `false` |
| `netDownload` / `netUpload` | bool | `true` / `true` |
| `netIface` | string (`"auto"` or interface name) | `"auto"` |
| `swapUploadDownload` | bool | `false` |
| `agentsTree` | bool | `false` |
| `agentsSort` / `agentsReverse` | enum / bool | `"cpu"` / `true` |
| `agentsShowExternal` | bool | `true` |

The theme extension is `[theme.resources]`, also camelCase, and every color accepts the existing theme syntax (`#rrggbb` or named ratatui color). Box colors: `cpuBox=#556d59`, `memBox=#6c6c4b`, `netBox=#5c588d`, `agentsBox=#805252`, `divLine`, `meterBg`, `graphText`, `inactiveFg`. Each gradient has `Start`, optional `Mid`, `End`: `cpu`, `temp`, `used`, `free`, `cached`, `available`, `download`, `upload`, `process`. Default CPU gradient stops are `#77ca9b`, `#cbc06c`, `#dc4c4c`; remaining stop colors port btop's pinned default theme and are listed in issue #4's implementation tests. Empty mid means interpolate start-to-end; empty end means solid start. Precompute 101 colors. Respect terminal color depth: truecolor, nearest 256-color cube/gray, then nearest 16-color palette; color depth affects rendering only, never config values.

### 7. Sampler budget and ownership

One background sampler owns one `sysinfo::System`; it sends immutable `Arc<ResourceSnapshot>` values to the event loop at the configured interval. Rendering reads snapshots/history only. Do not create or refresh `sysinfo::System` inside any render module. Build the process parent index once per sample (O(processes + edges)), classify in that pass, and reuse it for session aggregates and Agents rows. Keep history capacity to `maxGraphWidth * 2`, cap retained per-process histories to currently visible agent rows plus a short LRU, and avoid per-process filesystem probes.

Budget at 2000 ms with 1000 processes: sampler CPU time p95 ≤40 ms per tick and ≤2% of one core averaged over a minute; sampler allocation ≤2 MiB per tick excluding sysinfo-owned buffers; event-loop message delivery and snapshot install ≤5 ms p95. Measure CPU time separately from wall time on a documented Linux fixture and one macOS/Windows smoke host. If over budget, reduce refreshed fields or split platform-specific collectors; never move the walk to drawing. Pause deep process detail work when neither Resources nor a process-usage footer is visible, while keeping any demanded footer cadence.

### 8. Follow-on research decisions

These answer every `Research first` prompt in senamakel/medulla#2–#12; deferred items stay explicit so implementers do not turn an unknown into a compatibility claim.

| Issue | Decision / evidence |
|---|---|
| #2 sampler | `run_cmd`/`AppMsg` event-loop seams exist (see `event_loop/mod.rs`, anchors in the issue); sampler posts one immutable snapshot message and owns sysinfo off draw path. Keep locked sysinfo 0.37.2 for this epic; no need to accept 0.39 churn to add network/component features. Exact 0.37→0.39 field/API changes, thread/task behavior and `MINIMUM_CPU_UPDATE_INTERVAL` are deferred to the collector implementer, who must check locked crate docs/source before choosing refresh flags. |
| #3 plot | Use btop's paired-sample graph lookup and ship braille/block/tty; default `braille` for parity, with `block`/`tty` user choice. Subconscious's dropped braille comment is a font/terminal concern, not evidence that braille rendering works in every terminal; add a focused width test against ratatui's buffer before enabling it by default. `plot` remains separate from `ui/app/render/graph`. |
| #4 theme | Port gradient behavior and default stops from pinned btop source/theme. The current theme parser has no `COLORTERM` detection (verified by `rg COLORTERM src/tui` returning no matches); implement terminal color-depth selection in the theme layer and test the 16/256/truecolor mappings rather than assume the terminal setting. |
| #5 layout | Port box proportions/minimums above, not incidental btop pixel math. `workflows` is currently a submodule under `ui/app` and provides a state organization precedent; exact files evolve with the tab implementation. |
| #6 CPU | Linux `/proc/stat`, `scaling_cur_freq`, hwmon labels and core mapping are specified for ship. `sysinfo` is the generic source for per-core/load/frequency/uptime; macOS/Windows sensor labels and core mapping are not verified, so temperature detail is later there. Do not promise common sensor names. |
| #7 memory | Linux disk IO can use sysinfo deltas, cross-checked with `/proc/diskstats`; ZFS remains out of scope. Verify `Disk::usage()` as cumulative counters vs delta against the locked crate source before calculating rates. macOS/Windows disk IO source is deferred. |
| #8 network | Use received/transmitted counter differences; rate is delta divided by elapsed sample time. The `NetScaler` follows the described btop thresholds (five over-limit samples, five below one tenth, 10 KiB floor, 1.3/3 multipliers). Interface addresses/state are displayed only when the locked sysinfo API reports them; collector must verify `NetworkData::ip_networks` per platform. |
| #9 process discovery | Matching signatures above follow checked Medulla launch command construction, not a claim that every installed CLI has a fixed process tree. Running all third-party harnesses under `ps --forest` is deferred because those executables/accounts are not guaranteed in this checkout; implementation must capture available launch fixtures and never broaden a match based on generic `node`. On macOS unreadable argv means Unknown/external with no command, not a guessed classification. |
| #10 registry | ACP code constructs `AcpAgentConfig` and passes it into `AcpAgent::new`; it does not expose a child PID at the Medulla call site. Whether ACP 2.x exposes a caller-spawn seam and whether every other spawn API yields PID must be checked against the locked crate/API at implementation time. Until then, do not claim registered attribution; fall back to verified PTY roots and label unowned rows external. |
| #11 pane | `btop_draw.cpp` Proc and `btop_input.cpp` define reference interactions; this spec lists the intended reduced columns/actions and signal safety. Sessions rail APIs can be reused after verifying selection-by-session support. The #320 scroll helpers are unavailable because that issue has not landed; implement local bounded selection scrolling. |
| #12 remote | Sessions is a whole-list, latest-wins message that changes rarely (`remote/types.rs:208-216`). Send optional bounded usage on that stream at no faster than `updateMs`. Do not advertise capability because the field is additive. Exact worst-case encoded payload measurement is deferred to the protocol change; the 16 KiB limit and separate-channel fallback are requirements, not asserted current link guarantees. |

## Delivery order

1. Land this decision document before implementation issues #6–#12.
2. Build shared sampler/config and plot/theme foundations (#2–#4).
3. Add Resources shell/layout/presets (#5).
4. Add CPU, memory and network panels (#6–#8) and process classification/registry (#9–#10).
5. Add Agents pane and additive remote usage (#11–#12); validate key behavior against the landed action registry if #320 has merged by then.

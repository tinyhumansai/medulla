//! A load harness for the PTY session layer.
//!
//! Answers the question the audit could not: what actually happens to the worker
//! TUI when several harnesses are all producing output at once. It stands up N
//! real children on real pseudo-terminals, each flooding its pty, and then
//! measures the things an operator feels:
//!
//! - **frame time** — one render pass' worth of work (`rows()` for the session
//!   list plus `screen_rows()` for the visible pane), which is what the 40 ms
//!   tick has to fit inside;
//! - **allocations per frame** — counted by a wrapping global allocator, since
//!   the per-cell `String` was the largest single cost;
//! - **snapshot throughput** — how many screen reads a second the layer sustains
//!   while every session is flooding;
//! - **head-of-line blocking** — frame time observed while a write is parked
//!   against a child that has stopped draining its input. This is the freeze the
//!   audit's B1 describes, and it is the one number that cannot be recovered
//!   from a microbenchmark.
//!
//! Deliberately written against only the stable `PtyManager` surface — `open`,
//! `rows`, `screen_rows`, `write`, `shutdown` — so the same file runs unchanged
//! on either side of a refactor and the two runs are comparable.
//!
//! ```text
//! cargo run --release --example pty_load -- [sessions] [seconds]
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla::protocol::HarnessProvider;
use medulla_tui::worker::pty::{LaunchSpec, PtyManager, SessionControl};

// ------------------------------------------------------------- allocator ---

/// Counts allocations so a frame's cost can be stated in allocations, not only
/// in nanoseconds. The per-cell `String` was invisible in a wall-clock number on
/// an idle machine and dominant on a loaded one; the count is the same either
/// way.
///
/// **Per thread**, which matters here: ten flooding children keep ten reader
/// threads allocating continuously inside `vt100`, and a process-wide counter
/// attributes all of that to whichever frame happened to be in flight. Only the
/// measuring thread's own allocations answer "what does a render pass cost".
struct Counting;

thread_local! {
    /// This thread's allocation count.
    static ALLOCATIONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Add one to the calling thread's count, if its local is still alive.
///
/// `try_with` rather than `with`: a thread-local can be touched during TLS
/// teardown, and panicking inside the allocator would abort.
fn bump() {
    let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump();
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump();
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Allocations made by the calling thread so far.
fn allocations() -> u64 {
    ALLOCATIONS.with(|count| count.get())
}

// --------------------------------------------------------------- harness ---

/// A session whose child floods its pty with full-width lines.
///
/// `yes` is the whole trick: it writes one line per iteration as fast as the pty
/// will take it, which is the same shape as a harness repainting a spinner or
/// streaming tokens, and it never reads its stdin — so the same child also
/// serves as the wedged-input case in [`head_of_line`].
fn flooding(label: &str) -> LaunchSpec {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    LaunchSpec {
        // Codex takes no preset session id, so its interactive argv is empty and
        // the script below is the whole command.
        provider: HarnessProvider::Codex,
        bin: "/bin/sh".to_string(),
        cwd: "/".to_string(),
        env,
        extra_args: vec![
            "-c".to_string(),
            // ~56 columns of text on a 120-column screen, so a little under half
            // of each row is blank. A harness streaming tokens or repainting a
            // status block looks like this; a wall of full-width text does not,
            // and measuring against one would flatter any change that makes
            // blank cells cheaper.
            "exec yes \"$(printf '%0.sX' $(seq 1 56))\"".to_string(),
        ],
        skip_permissions: false,
        label: label.to_string(),
        model: None,
        session_id: None,
        // The orchestrator's own sessions, as a task frame opens them: this
        // measures the dispatch path, and an operator-held session is one
        // `claim_idle` skips entirely.
        control: SessionControl::Orchestrator,
        origin: medulla_tui::worker::pty::SessionOrigin::Orchestrator,
        name: None,
        mcp_grant_session: None,
    }
}

/// Percentile of an already-sorted slice.
fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let index = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[index]
}

/// Print a duration distribution.
fn report(what: &str, mut samples: Vec<Duration>) {
    samples.sort();
    println!(
        "  {what:<28} p50 {:>8.3}ms   p99 {:>8.3}ms   max {:>8.3}ms   n={}",
        percentile(&samples, 0.50).as_secs_f64() * 1e3,
        percentile(&samples, 0.99).as_secs_f64() * 1e3,
        percentile(&samples, 1.0).as_secs_f64() * 1e3,
        samples.len(),
    );
}

/// One render pass: the session list, then the visible pane's screen.
fn frame(sessions: &PtyManager, visible: &str) {
    let _rows = sessions.rows();
    let _snapshot = sessions.screen_rows(visible);
}

// ----------------------------------------------------------------- phases ---

/// Frame time and per-frame allocations at the render loop's own cadence.
fn render_loop(sessions: &PtyManager, visible: &str, seconds: u64) {
    const TICK: Duration = Duration::from_millis(40);

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut samples = Vec::new();
    // Accumulated per frame rather than across the whole loop, so the idle time
    // between ticks contributes nothing.
    let mut allocated = 0u64;
    while Instant::now() < deadline {
        let before = allocations();
        let started = Instant::now();
        frame(sessions, visible);
        let elapsed = started.elapsed();
        allocated += allocations() - before;
        samples.push(elapsed);
        std::thread::sleep(TICK);
    }
    let frames = samples.len() as u64;

    println!("\nrender loop (40ms tick, one visible pane)");
    report("frame time", samples);
    println!(
        "  {:<28} {} per frame  ({} total over {frames} frames)",
        "allocations",
        allocated / frames.max(1),
        allocated
    );
}

/// How many screen reads a second the layer sustains under the flood.
fn throughput(sessions: &PtyManager, visible: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut frames = 0u64;
    let before = allocations();
    while Instant::now() < deadline {
        frame(sessions, visible);
        frames += 1;
    }
    let allocated = allocations() - before;

    println!("\nsaturated snapshot throughput (no tick)");
    println!("  {:<28} {} frames/sec", "throughput", frames / 2);
    println!(
        "  {:<28} {} per frame",
        "allocations",
        allocated / frames.max(1)
    );
}

/// One poll tick of the prompt injector, old way versus new.
///
/// Both are measured here, on one tree and one machine, because the comparison
/// is about the *shape* of the work rather than about the refactor: the "old"
/// arm is exactly what `inject.rs` used to do on every 25 ms tick — build a full
/// `ScreenSnapshot`, join it into one string, then squash the whole screen once
/// per needle — and it is still expressible through the public `screen_rows`.
/// The "new" arm is what it does now: one pass off the emulator into buffers the
/// caller reuses.
///
/// This path matters out of proportion to how it looks, because it runs at 40 Hz
/// *per session that is still starting up* — so its cost is multiplied by
/// exactly the fan-out that already has the machine busy.
fn injector_tick(sessions: &PtyManager, visible: &str) {
    const TICKS: usize = 200;
    let needle = "somepromptneedletolookfor";

    // --- the old shape: snapshot, join, squash, squash again ---
    let mut old_samples = Vec::with_capacity(TICKS);
    let mut old_allocs = 0u64;
    for _ in 0..TICKS {
        let before = allocations();
        let started = Instant::now();
        let text = match sessions.screen_rows(visible) {
            Some(snapshot) => snapshot
                .cells
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n"),
            None => String::new(),
        };
        let squash = |screen: &str| -> String {
            screen
                .chars()
                .filter(|c| !c.is_whitespace())
                .flat_map(char::to_lowercase)
                .collect()
        };
        let hits =
            squash(&text).matches(needle).count() + squash(&text).matches("pastedtext").count();
        let elapsed = started.elapsed();
        old_allocs += allocations() - before;
        old_samples.push(elapsed);
        std::hint::black_box(hits);
    }

    // --- the new shape: one pass into reused buffers ---
    let mut new_samples = Vec::with_capacity(TICKS);
    let mut new_allocs = 0u64;
    let mut squashed = String::new();
    for _ in 0..TICKS {
        let before = allocations();
        let started = Instant::now();
        sessions.screen_squashed_into(visible, &mut squashed);
        let hits = squashed.matches(needle).count() + squashed.matches("pastedtext").count();
        let elapsed = started.elapsed();
        new_allocs += allocations() - before;
        new_samples.push(elapsed);
        std::hint::black_box(hits);
    }

    println!("\ninjector poll tick (runs at 40Hz per starting session)");
    report("old: snapshot+join+squash", old_samples);
    println!(
        "  {:<28} {} per tick",
        "old: allocations",
        old_allocs / TICKS as u64
    );
    report("new: one pass, reused buf", new_samples);
    println!(
        "  {:<28} {} per tick",
        "new: allocations",
        new_allocs / TICKS as u64
    );
}

/// Frame time while other threads are reading *other* sessions' screens.
///
/// This is the contention the audit's P2 describes, isolated. In the worker,
/// screens are read concurrently from several places — the render loop, one
/// screen sampler per subscriber, and the injector polling every session that is
/// still starting. Whether those interfere with each other is entirely a
/// property of how the layer is locked: with one registry mutex held across the
/// snapshot they serialise, so the render loop's frame time grows with the
/// number of other readers; with per-session locks they do not.
///
/// The flood keeps running underneath, so the reader threads are contending for
/// the same emulators the children are feeding.
/// `pace` is how often each reader samples. `None` means as fast as it can,
/// which measures *capacity* rather than latency — with ten flooding children,
/// ten reader threads and ten cores, an unpaced run is CPU-saturated by
/// construction, so its own tail says more about the scheduler than about the
/// locking. The paced run is the honest latency number: 10 Hz is what
/// `stream::sampler` actually caps a subscriber at.
fn concurrent_readers(
    sessions: &PtyManager,
    ids: &[String],
    visible: &str,
    pace: Option<Duration>,
) {
    let stop = Arc::new(AtomicBool::new(false));
    let mut readers = Vec::new();
    // One reader per session, each on a *different* screen from the one the
    // render loop is watching — so any interference measured is the layer's,
    // not two threads genuinely wanting the same emulator.
    for id in ids.iter().skip(1) {
        let sessions = sessions.clone();
        let id = id.clone();
        let stop = stop.clone();
        readers.push(std::thread::spawn(move || {
            let mut reads = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let _ = sessions.screen_rows(&id);
                reads += 1;
                if let Some(pace) = pace {
                    std::thread::sleep(pace);
                }
            }
            reads
        }));
    }

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut samples = Vec::new();
    while Instant::now() < deadline {
        let started = Instant::now();
        frame(sessions, visible);
        samples.push(started.elapsed());
        std::thread::sleep(Duration::from_millis(40));
    }
    stop.store(true, Ordering::Relaxed);
    let total: u64 = readers.into_iter().filter_map(|r| r.join().ok()).sum();

    println!(
        "\ncontention ({} concurrent readers on other sessions, {})",
        ids.len().saturating_sub(1),
        match pace {
            Some(pace) => format!("{}Hz each", 1000 / pace.as_millis().max(1)),
            None => "unpaced".to_string(),
        }
    );
    report("render frame time", samples);
    println!(
        "  {:<28} {} snapshots/sec across all readers",
        "aggregate reads",
        total / 3
    );
}

/// What a large write into a child that is not draining its input does.
///
/// The audit's B1: `write_all` on a pty master blocks once the target's tty
/// input buffer fills, and under a single registry lock that parks every other
/// session and the whole render loop behind it. `yes` never reads its stdin, so
/// the write has nowhere to go — though whether it *blocks* or is refused
/// depends on the platform's pty, which is why the outcome is reported rather
/// than assumed.
fn wedged_write(sessions: &PtyManager, wedged: &str, visible: &str) {
    // Comfortably past any pty's input buffer, so the write cannot drain.
    let payload = vec![b'A'; 1 << 20];
    let done = Arc::new(AtomicBool::new(false));
    let outcome = Arc::new(std::sync::Mutex::new(String::new()));

    {
        let sessions = sessions.clone();
        let wedged = wedged.to_string();
        let done = done.clone();
        let outcome = outcome.clone();
        // Detached: the write may never return, and a benchmark must not hang on
        // the very failure it is measuring.
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = sessions.write(&wedged, &payload);
            *outcome.lock().unwrap() = format!(
                "{} after {:.1}ms",
                match result {
                    Ok(()) => "accepted".to_string(),
                    Err(err) => format!("refused ({err})"),
                },
                started.elapsed().as_secs_f64() * 1e3
            );
            done.store(true, Ordering::Release);
        });
    }

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut samples = Vec::new();
    while !done.load(Ordering::Acquire) && Instant::now() < deadline {
        let started = Instant::now();
        frame(sessions, visible);
        samples.push(started.elapsed());
        std::thread::sleep(Duration::from_millis(40));
    }

    println!("\nwedged write (1MB into a child that never reads its stdin)");
    if samples.is_empty() {
        println!("  the write settled immediately; no window to sample");
    } else {
        report("frame time during write", samples);
    }
    let settled = outcome.lock().unwrap().clone();
    println!(
        "  {:<28} {}",
        "write outcome",
        if settled.is_empty() {
            "still parked at 3s".to_string()
        } else {
            settled
        }
    );
    sessions.close(wedged);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let count: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(10);
    let seconds: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(5);

    println!("pty load: {count} flooding sessions, {seconds}s render sampling");

    let sessions = PtyManager::new();
    let mut ids = Vec::new();
    let opening = Instant::now();
    for i in 0..count {
        match sessions.open(flooding(&format!("peer-{i}"))) {
            Ok(id) => ids.push(id),
            Err(err) => {
                eprintln!("could not open session {i}: {err}");
                break;
            }
        }
    }
    println!(
        "opened {} sessions in {:.1}ms ({:.1}ms each)",
        ids.len(),
        opening.elapsed().as_secs_f64() * 1e3,
        opening.elapsed().as_secs_f64() * 1e3 / ids.len().max(1) as f64,
    );
    if ids.is_empty() {
        eprintln!("nothing to measure");
        return;
    }

    // Let every child get past exec and start flooding, so the measurements are
    // of the steady state rather than of startup.
    std::thread::sleep(Duration::from_millis(750));
    println!("{} still running", sessions.running_count());

    let visible = ids[0].clone();
    render_loop(&sessions, &visible, seconds);
    throughput(&sessions, &visible);
    injector_tick(&sessions, &visible);
    if ids.len() > 1 {
        // The realistic shape first (a subscriber per session at the sampler's
        // own ceiling), then the unpaced one for capacity.
        concurrent_readers(&sessions, &ids, &visible, Some(Duration::from_millis(100)));
        concurrent_readers(&sessions, &ids, &visible, None);
        wedged_write(&sessions, &ids[ids.len() - 1], &visible);
    }

    sessions.shutdown();
    println!("\ndone");
}

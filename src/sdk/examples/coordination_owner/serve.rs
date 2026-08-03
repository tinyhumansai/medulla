//! Serve mode: run one leg per request file, on a link that stays up.
//!
//! The link's SSP state lives in memory, so an endpoint that exits and restarts
//! comes back at state 0 while its peer still holds state *n* and neither side's
//! diffs apply again. A long-lived orchestrator is what the protocol assumes, so
//! legs arrive as files rather than as processes.

use std::path::{Path, PathBuf};

use medulla::bridge::LinkBridge;
use medulla_link::keys::NodeId;

use crate::leg::{connect, run_leg};
use crate::{parse_args, Args, POLL};

/// Run legs from a request queue until the process is killed.
///
/// A request is a file `<label>.req` holding one argument per line. The harness
/// writes it under a temporary name and renames it into place, so this loop
/// never reads a half-written file. Each finished leg writes `<label>.json` (the
/// terminal-frame report) and then `<label>.rc` (the exit code) into the results
/// directory — in that order, because the harness waits on the `.rc`.
pub async fn serve(
    connected: LinkBridge,
    state_dir: &Path,
    args: &Args,
    owner_id: NodeId,
    queue: &Path,
    results: &Path,
) -> Result<i32, String> {
    std::fs::create_dir_all(queue)
        .map_err(|e| format!("could not create {}: {e}", queue.display()))?;
    std::fs::create_dir_all(results)
        .map_err(|e| format!("could not create {}: {e}", results.display()))?;
    println!(
        "coordination_owner serving {owner_id} from {}",
        queue.display()
    );
    // Held in an `Option` so a rebuild can drop the old link *before* opening the
    // new one: the identity lock (§7.3) is exclusive, and a reassignment would
    // hold both at once and deadlock against itself.
    let mut link = Some(connected);

    loop {
        for (label, path) in pending(queue)? {
            let taken = path.with_extension("taken");
            if std::fs::rename(&path, &taken).is_err() {
                continue; // another pass took it, or it vanished
            }
            let body = std::fs::read_to_string(&taken)
                .map_err(|e| format!("could not read {}: {e}", taken.display()))?;
            let lines = body
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            let request = match parse_args(lines.into_iter()) {
                Ok(request) => request,
                Err(err) => {
                    let report = serde_json::json!({ "kind": "None", "text": err });
                    write_result(results, &label, 2, &report)?;
                    continue;
                }
            };
            if request.leg.reset_link {
                // The peer restarted, so its SSP state is back at 0 while ours is
                // not. Rebuilding the link puts both ends back on a shared origin;
                // the persisted sequence reservation (§3.1) means no nonce repeats.
                eprintln!("coordination_owner: rebuilding the link for leg {label}");
                drop(link.take());
                link = Some(connect(state_dir, args.forwarder.as_deref()).await?);
            }
            if request.leg.reset_only {
                let report = serde_json::json!({
                    "kind": "LinkReset",
                    "text": "link rebuilt; nothing dispatched",
                    "taskId": request.leg.task_id,
                    "ownerId": owner_id.to_string(),
                });
                println!("{report}");
                write_result(results, &label, 0, &report)?;
                continue;
            }
            let handle = link.as_ref().expect("the link is only ever briefly absent");
            let (code, report) = run_leg(handle, owner_id, &request.leg).await;
            println!("{report}");
            write_result(results, &label, code, &report)?;
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Request files waiting in the queue, oldest name first.
fn pending(queue: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let entries =
        std::fs::read_dir(queue).map_err(|e| format!("could not read {}: {e}", queue.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("req") {
            continue;
        }
        let Some(label) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        out.push((label.to_string(), path.clone()));
    }
    out.sort();
    Ok(out)
}

/// Write `<label>.json` then `<label>.rc`.
fn write_result(
    results: &Path,
    label: &str,
    code: i32,
    report: &serde_json::Value,
) -> Result<(), String> {
    let json = results.join(format!("{label}.json"));
    std::fs::write(&json, format!("{report}\n"))
        .map_err(|e| format!("could not write {}: {e}", json.display()))?;
    let rc = results.join(format!("{label}.rc"));
    std::fs::write(&rc, format!("{code}\n"))
        .map_err(|e| format!("could not write {}: {e}", rc.display()))
}

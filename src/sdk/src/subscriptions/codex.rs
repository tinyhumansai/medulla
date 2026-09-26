//! Codex rate-limit windows, read from the CLI's own rollout transcripts.
//!
//! Codex records the rate limits the server hands back on every turn into the
//! session rollout it is already writing, so the numbers are on disk and no
//! second request to OpenAI is needed to read them. That also means the reading
//! is only as fresh as the last Codex turn on this machine — which is the right
//! staleness: a window nobody has spent against has not moved.
//!
//! The transcripts are append-only and can run to megabytes, so only the tail
//! of the newest one is read.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::config::MeterId;

use super::types::MeterReading;

/// Bytes of the newest rollout read looking for the last rate-limit record.
///
/// Generous next to one record (a few hundred bytes) and cheap next to the
/// whole file. A rollout whose last quarter-megabyte carries no rate limits is
/// one where the turn that would have written them has not happened.
const TAIL_BYTES: u64 = 256 * 1024;

/// Windows at or under this many minutes are the short "session" limit; longer
/// ones are the weekly. Codex names neither, and the actual lengths differ by
/// plan, so the split is by magnitude rather than by an exact match.
const SESSION_WINDOW_MAX_MINUTES: u64 = 1440;

/// Sample the Codex meters from the newest rollout on this machine.
pub fn sample() -> Result<Vec<(MeterId, MeterReading)>, String> {
    let sessions = sessions_dir().ok_or_else(|| "no Codex home".to_string())?;
    let newest = newest_rollout(&sessions).ok_or_else(|| "no Codex sessions".to_string())?;
    let tail = read_tail(&newest, TAIL_BYTES).map_err(|error| error.to_string())?;
    let limits = last_rate_limits(&tail).ok_or_else(|| "no rate limits recorded".to_string())?;
    Ok(readings(&limits))
}

/// Map one `rate_limits` object onto the meters it describes.
pub fn readings(limits: &Value) -> Vec<(MeterId, MeterReading)> {
    let mut out: Vec<(MeterId, MeterReading)> = Vec::new();
    for key in ["primary", "secondary"] {
        let Some(window) = limits.get(key).filter(|value| !value.is_null()) else {
            continue;
        };
        let minutes = window
            .get("window_minutes")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX);
        let id = if minutes <= SESSION_WINDOW_MAX_MINUTES {
            MeterId::CodexSession
        } else {
            MeterId::CodexWeekly
        };
        if out.iter().any(|(existing, _)| *existing == id) {
            continue;
        }
        out.push((
            id,
            MeterReading {
                used_fraction: window
                    .get("used_percent")
                    .and_then(Value::as_f64)
                    .map(|value| (value / 100.0).clamp(0.0, 1.0)),
                resets_at: window.get("resets_at").and_then(Value::as_i64),
                ..MeterReading::default()
            },
        ));
    }
    out
}

/// The last complete `rate_limits` object in a chunk of rollout text.
///
/// Scans backwards so a tail that begins mid-line — which it usually does —
/// costs nothing: the newest record is at the end, and a record split by the
/// read boundary is simply the one before it.
fn last_rate_limits(tail: &str) -> Option<Value> {
    const KEY: &str = "\"rate_limits\":";
    let mut search_from = tail.rfind(KEY);
    while let Some(at) = search_from {
        let after = &tail[at + KEY.len()..];
        if let Some(object) = balanced_object(after) {
            if let Ok(value) = serde_json::from_str::<Value>(object) {
                return Some(value);
            }
        }
        search_from = tail[..at].rfind(KEY);
    }
    None
}

/// The first brace-balanced JSON object in `text`, ignoring braces in strings.
fn balanced_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in text[start..].bytes().enumerate() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + offset + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Read at most `limit` bytes from the end of `path`, as lossy UTF-8.
fn read_tail(path: &Path, limit: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(limit);
    file.seek(SeekFrom::Start(start))?;
    let mut buffer = Vec::with_capacity(size.saturating_sub(start) as usize);
    file.read_to_end(&mut buffer)?;
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

/// The most recently modified `*.jsonl` under `root`, at any depth.
///
/// Codex files its rollouts under `sessions/<year>/<month>/<day>/`, but the
/// layout has changed before, so the walk does not assume it. Depth is bounded
/// so a symlinked or unexpectedly deep tree cannot turn one sidebar refresh
/// into a filesystem crawl.
fn newest_rollout(root: &Path) -> Option<PathBuf> {
    const MAX_DEPTH: usize = 6;

    let mut best: Option<(SystemTime, PathBuf)> = None;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
                continue;
            };
            if best.as_ref().is_none_or(|(best, _)| modified > *best) {
                best = Some((modified, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

/// `$CODEX_HOME/sessions`, else `~/.codex/sessions`.
fn sessions_dir() -> Option<PathBuf> {
    let home = std::env::var("CODEX_HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(dirs::home_dir()?.join(".codex")))?;
    Some(home.join("sessions"))
}

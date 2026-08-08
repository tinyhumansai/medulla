with open('src/sdk/src/session_history/summary.rs', 'r') as f:
    content = f.read()

old = '    })\n}\n\n/// Read the first [`HEAD_BYTES`] of `path` as UTF-8 (lossy) and split into'
new = '''    })
}

/// Load the Codex session-index into an id-to-label map.
///
/// Callers that need to resolve thread names for many sessions (e.g. the
/// recent-session list) can load the index once and look up every session
/// against the same map, rather than re-reading and re-parsing the file
/// per session.
pub fn codex_index_map(env: &HashMap<String, String>) -> HashMap<String, String> {
    let Some(index_path) = (|| {
        super::scan::codex_sessions_dir(env)
            .parent()?
            .join("session_index.jsonl")
            .into()
    })() else {
        return HashMap::new();
    };
    let Ok(contents) = std::fs::read_to_string(index_path) else {
        return HashMap::new();
    };
    contents
        .lines()
        .filter_map(|line| {
            let record: Value = serde_json::from_str(line).ok()?;
            let object = record.as_object()?;
            let id = object.get("id").and_then(Value::as_str)?;
            let label = object
                .get("thread_name")
                .and_then(Value::as_str)
                .map(slug_label)
                .filter(|l| !l.is_empty())?;
            Some((id.to_string(), label))
        })
        .collect()
}

/// Read the first [`HEAD_BYTES`] of `path` as UTF-8 (lossy) and split into'''

content = content.replace(old, new)
with open('src/sdk/src/session_history/summary.rs', 'w') as f:
    f.write(content)
print("Done")

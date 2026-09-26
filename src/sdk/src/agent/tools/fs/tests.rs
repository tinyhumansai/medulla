//! Read and listing bounds. Both caps have to bound the *work*, not only the
//! answer — a model can point either tool at a generated tree.

use serde_json::json;
use tinyagents::tool::Tool;
use tinyinference::tool::ToolCall;

use super::{ListDirTool, ReadFileTool, Workspace, MAX_ENTRIES, MAX_READ_BYTES};

fn workspace(dir: &tempfile::TempDir) -> Workspace {
    Workspace {
        root: dir.path().to_path_buf(),
    }
}

fn call(args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "c1".into(),
        name: "t".into(),
        arguments: args,
        invalid: None,
    }
}

/// The cap bounds what is read, not just what is returned. Reading the whole
/// file and slicing afterwards makes a `read_file` on a multi-gigabyte log an
/// out-of-memory abort of the host.
#[tokio::test]
async fn an_oversized_file_is_truncated_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![b'x'; MAX_READ_BYTES * 2];
    std::fs::write(dir.path().join("big.txt"), &body).unwrap();

    let out = ReadFileTool::new(workspace(&dir))
        .call(&(), call(json!({ "path": "big.txt" })))
        .await
        .unwrap();

    assert!(out.content.contains("truncated"), "the cut must be named");
    assert!(
        out.content.contains(&(MAX_READ_BYTES * 2).to_string()),
        "the real size is reported: {}",
        &out.content[out.content.len().saturating_sub(120)..]
    );
    // The notice adds a little, so the bound is on the payload not the string.
    assert!(out.content.len() < MAX_READ_BYTES + 200);
}

#[tokio::test]
async fn a_file_within_the_cap_comes_back_whole() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("small.txt"), b"hello").unwrap();

    let out = ReadFileTool::new(workspace(&dir))
        .call(&(), call(json!({ "path": "small.txt" })))
        .await
        .unwrap();
    assert_eq!(out.content, "hello");
}

/// A directory at or under the cap is read whole, so the listing is complete
/// and deterministic — the case that stopping early must not disturb.
#[tokio::test]
async fn a_small_directory_lists_completely_and_in_order() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["c.txt", "a.txt", "b.txt"] {
        std::fs::write(dir.path().join(name), b"").unwrap();
    }

    let out = ListDirTool::new(workspace(&dir))
        .call(&(), call(json!({})))
        .await
        .unwrap();
    assert_eq!(out.content, "a.txt\nb.txt\nc.txt");
    assert!(!out.content.contains("truncated"));
}

/// Over the cap, the listing stops rather than scanning to the end — so it
/// reports incompleteness without an exact total, which is the trade the bound
/// buys.
#[tokio::test]
async fn an_oversized_directory_stops_at_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..(MAX_ENTRIES + 50) {
        std::fs::write(dir.path().join(format!("f{i:05}.txt")), b"").unwrap();
    }

    let out = ListDirTool::new(workspace(&dir))
        .call(&(), call(json!({})))
        .await
        .unwrap();

    assert!(out.content.contains("truncated"), "{}", out.content);
    assert!(
        out.content.contains("more were omitted"),
        "incompleteness must be stated without claiming a total: {}",
        out.content
    );
    let listed = out.content.lines().filter(|l| l.ends_with(".txt")).count();
    assert_eq!(listed, MAX_ENTRIES, "exactly the cap is listed");
}

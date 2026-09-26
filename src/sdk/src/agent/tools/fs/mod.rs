//! Filesystem tools: read, write, and list, all rooted at the turn's checkout.
//!
//! Every path argument goes through [`crate::agent::tools::guard::contained`] before it is opened, so
//! a model that asks for `../../etc/passwd` gets a refusal the run can see
//! rather than a file it should not have.
//!
//! # Reads are truncated, and the truncation says so
//!
//! A tool result is model context. Returning a 40 MB file would blow the window
//! and evict the very instructions the turn is following, so a read is capped —
//! and the cap is reported in the returned text rather than silently applied,
//! because a model that cannot tell it received a prefix will reason about the
//! file as though it saw all of it.
//!
//! The cap bounds what is *read*, not just what is returned. Reading the whole
//! file and slicing afterwards would make a model's `read_file` on a multi-
//! gigabyte log an out-of-memory abort of the host process, which is a worse
//! outcome than any answer the tool could have given.

use std::sync::Arc;

use tinyagents::tool::Tool;

mod list;
mod read;
mod types;
mod write;

pub use list::ListDirTool;
pub use read::ReadFileTool;
pub use types::Workspace;
pub use write::WriteFileTool;

/// Most bytes a single `read_file` returns.
///
/// Roughly 16k tokens of dense source at four bytes per token — large enough for
/// nearly every real file, small enough that one unlucky read cannot evict the
/// turn's own instructions.
const MAX_READ_BYTES: usize = 64 * 1024;

/// Most entries a single `list_dir` returns, for the same reason.
const MAX_ENTRIES: usize = 500;

/// Every filesystem tool, rooted at one checkout.
pub fn all(workspace: Workspace) -> Vec<Arc<dyn Tool<()>>> {
    vec![
        Arc::new(ReadFileTool::new(workspace.clone())),
        Arc::new(WriteFileTool::new(workspace.clone())),
        Arc::new(ListDirTool::new(workspace)),
    ]
}

#[cfg(test)]
mod tests;

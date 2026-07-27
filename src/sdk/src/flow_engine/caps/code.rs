//! The `code` node's runner — refusing by default.
//!
//! The sibling `openhuman` host runs `code` nodes inside its sandbox, with an
//! autonomy tier deciding whether the call needs approval. Medulla has no
//! equivalent sandbox: a daemon here already has the privileges of the user who
//! started it, so executing a workflow author's JavaScript would execute it with
//! those privileges and no boundary at all.
//!
//! Rather than pretend otherwise, this adapter refuses unless a host explicitly
//! opts in, and says why. A workflow that needs computation should use a
//! `transform` node's jq expressions, which the engine evaluates itself.

use async_trait::async_trait;
use serde_json::Value;
use tinyflows::caps::{CodeLanguage, CodeRunner};
use tinyflows::error::{EngineError, Result};

/// A [`CodeRunner`] that refuses every request, explaining the missing sandbox.
pub struct DeniedCodeRunner;

#[async_trait]
impl CodeRunner for DeniedCodeRunner {
    async fn run(&self, _language: CodeLanguage, _source: &str, _input: Value) -> Result<Value> {
        Err(EngineError::Capability(
            "code nodes are disabled: this host has no sandbox, so workflow code would run with \
             the daemon's full privileges. Enable `workflows.allowCode` only where that is \
             acceptable, or use a `transform` node's expressions instead."
                .to_string(),
        ))
    }
}

/// A [`CodeRunner`] that runs code out-of-process, for hosts that opted in.
///
/// Still not a sandbox — it is the operator's explicit decision to trust the
/// workflow author — but at least bounded: the process is given the source on a
/// temporary file, the input on stdin, and a wall-clock limit.
pub struct ProcessCodeRunner {
    /// How long one execution may take.
    timeout: std::time::Duration,
}

impl ProcessCodeRunner {
    /// A runner with the given per-execution timeout.
    pub fn new(timeout: std::time::Duration) -> Self {
        Self { timeout }
    }
}

#[async_trait]
impl CodeRunner for ProcessCodeRunner {
    async fn run(&self, language: CodeLanguage, source: &str, input: Value) -> Result<Value> {
        let (program, extension) = match language {
            CodeLanguage::JavaScript => ("node", "js"),
            CodeLanguage::Python => ("python3", "py"),
        };

        let dir =
            tempfile::tempdir().map_err(|err| EngineError::Capability(format!("code: {err}")))?;
        let script = dir.path().join(format!("script.{extension}"));
        std::fs::write(&script, source)
            .map_err(|err| EngineError::Capability(format!("code: {err}")))?;
        let input_path = dir.path().join("input.json");
        let body = serde_json::to_vec(&input)
            .map_err(|err| EngineError::Capability(format!("code: {err}")))?;
        std::fs::write(&input_path, body)
            .map_err(|err| EngineError::Capability(format!("code: {err}")))?;

        let run = tokio::process::Command::new(program)
            .arg(&script)
            .arg(&input_path)
            .current_dir(dir.path())
            .output();
        let output = tokio::time::timeout(self.timeout, run)
            .await
            .map_err(|_| EngineError::Capability("code: timed out".to_string()))?
            .map_err(|err| EngineError::Capability(format!("code: {program}: {err}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(EngineError::Capability(format!(
                "code: {program} exited with {}: {}",
                output.status,
                stderr.trim()
            )));
        }

        // Structured output when the script printed JSON, the raw text
        // otherwise — a script that just prints a line should still be usable.
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(serde_json::from_str(stdout.trim())
            .unwrap_or_else(|_| Value::String(stdout.trim().to_string())))
    }
}

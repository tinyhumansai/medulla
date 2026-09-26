//! What the scrub keeps and what it drops.

use std::collections::HashMap;

use super::{is_secret_name, scrubbed};

#[test]
fn credential_shaped_names_are_dropped() {
    for name in [
        "MEDULLA_TOKEN",
        "OPENROUTER_API_KEY",
        "AWS_SECRET_ACCESS_KEY",
        "GITHUB_TOKEN",
        "DB_PASSWORD",
        "SSH_AUTH_SOCK",
        "AWS_SESSION_TOKEN",
    ] {
        assert!(is_secret_name(name), "{name} must be treated as a secret");
    }
}

/// A build needs these. Dropping one is a failure the model can neither see nor
/// fix, which is why the rule is a denylist.
#[test]
fn ordinary_build_variables_survive() {
    for name in [
        "PATH",
        "HOME",
        "LANG",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "TMPDIR",
        "SystemRoot",
        "ComSpec",
        "npm_config_registry",
    ] {
        assert!(!is_secret_name(name), "{name} must survive the scrub");
    }
}

/// Windows reports names as stored, so a case-sensitive rule would pass
/// `Github_Token` straight through.
#[test]
fn the_rule_is_case_insensitive() {
    assert!(is_secret_name("Github_Token"));
    assert!(is_secret_name("medulla_token"));
}

/// The account home is where the session store lives; a command that can read
/// it does not need the bearer in its environment to find one.
#[test]
fn the_account_home_is_dropped_despite_looking_innocuous() {
    assert!(is_secret_name("MEDULLA_HOME"));
    assert!(is_secret_name("MEDULLA_USER"));
}

#[test]
fn scrubbing_keeps_values_intact_for_what_it_retains() {
    let env: HashMap<String, String> = [
        ("PATH".to_string(), "/usr/bin".to_string()),
        ("MEDULLA_TOKEN".to_string(), "bearer".to_string()),
    ]
    .into_iter()
    .collect();

    let out = scrubbed(&env);
    assert_eq!(out.get("PATH").map(String::as_str), Some("/usr/bin"));
    assert!(!out.contains_key("MEDULLA_TOKEN"));
}

/// The scrub must be on the path a real turn takes, not merely available.
///
/// This is the check that was missing when `env_clear` was added without it:
/// the call was changed, the map it installed was never traced, and the
/// credential path stayed open while the fix read as complete. Asserting at the
/// seam — the tool set a turn is actually built with — is what would have caught
/// it.
#[tokio::test]
async fn a_turns_shell_cannot_read_a_credential_from_the_task_environment() {
    use tinyinference::tool::ToolCall;

    let dir = tempfile::tempdir().unwrap();
    // The shape a daemon hands down: the ambient environment, credentials and all.
    let mut task_env: HashMap<String, String> = std::env::vars().collect();
    task_env.insert("MEDULLA_TOKEN".to_string(), "leaked-bearer".to_string());
    task_env.insert("OPENROUTER_API_KEY".to_string(), "leaked-key".to_string());

    let tools = crate::agent::tools::all(
        crate::agent::tools::Workspace {
            root: dir.path().to_path_buf(),
        },
        std::time::Duration::from_secs(10),
        task_env,
    );
    let shell = tools
        .iter()
        .find(|t| t.name() == "shell")
        .expect("the tool set includes a shell");

    let out = shell
        .call(
            &(),
            ToolCall {
                id: "c1".into(),
                name: "shell".into(),
                arguments: serde_json::json!({ "command": "env" }),
                invalid: None,
            },
        )
        .await
        .expect("the command runs");

    assert!(
        !out.content.contains("leaked-bearer"),
        "MEDULLA_TOKEN reached the shell: {}",
        out.content
    );
    assert!(
        !out.content.contains("leaked-key"),
        "a router key reached the shell: {}",
        out.content
    );
    // The turn still has a usable environment, or the scrub broke every build.
    assert!(
        out.content.contains("PATH="),
        "the shell lost its PATH: {}",
        out.content
    );
}

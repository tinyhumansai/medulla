//! (Unix-only: exercises spawned `/bin/sh` fake-provider scripts.)
//!
//! End-to-end coverage for the daemon's client-side OpenAI-compatible router at
//! the provider spawn seam ([`medulla::daemon::providers::run_provider_task`]):
//! endpoint injection, API-key resolution by env-var *name*, the no-router
//! passthrough, and the config-file → daemon-population wiring. The secrecy
//! invariant — the resolved key value never rides the reply frame — is pinned
//! here. Split out of `e2e_daemon_providers.rs` to keep each test file within
//! the mandatory 500-line ceiling.

mod support;

use std::collections::HashMap;

use medulla::daemon::providers::{run_provider_task, Abort, RunTaskOptions};
use medulla::protocol::HarnessProvider;

use support::fake_provider::TempDir;

/// Parse a `RouterConfig` from JSON for the spawn-seam tests.
fn router_cfg(json: &str) -> medulla::config::RouterConfig {
    serde_json::from_str(json).expect("valid router config")
}

/// Build an env carrying host `PATH` plus explicit overrides.
fn env_with(overrides: &[(&str, &str)]) -> HashMap<String, String> {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    for (k, v) in overrides {
        env.insert((*k).to_string(), (*v).to_string());
    }
    env
}

fn router_options(
    provider: HarnessProvider,
    bin: &str,
    env: HashMap<String, String>,
    router: Option<medulla::config::RouterConfig>,
) -> RunTaskOptions {
    let _ = bin;
    RunTaskOptions {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        transport: Default::default(),
        conversation: String::new(),
        session_class: medulla::sessions::SessionClass::Bounded,
        resume_session_id: None,
        workspace_context: Default::default(),
        provider,
        prompt: "do it".to_string(),
        cwd: ".".to_string(),
        env,
        timeout_ms: 5_000,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        abort: Abort::new(),
        router,
        attribution: true,
        on_event: None,
        on_stdin: None,
        on_session: None,
        on_workspace_context: None,
    }
}

#[tokio::test]
async fn router_injects_claude_endpoint_and_resolves_key_by_name_without_leaking() {
    // A distinctive, clearly-synthetic marker (not token-shaped, so it never
    // trips secret scanning) so any leak into the reply frame is unmistakable.
    const SECRET: &str = "ROUTER-KEY-VALUE-DO-NOT-LEAK-9f3a";
    let dir = TempDir::new();
    let marker = dir.path().join("key-marker");
    let marker = marker.to_string_lossy().into_owned();
    // The fake claude echoes the ROUTED endpoint into its result (safe to log)
    // and writes the resolved AUTH TOKEN to a marker file — out of band, so the
    // token never rides the reply frame.
    let script = format!(
        "#!/bin/sh\n\
         printf '%s' \"$ANTHROPIC_AUTH_TOKEN\" > '{marker}'\n\
         printf '{{\"type\":\"result\",\"result\":\"endpoint=%s\"}}\\n' \"$ANTHROPIC_BASE_URL\"\n",
    );
    let bin = dir.write_script("router_claude.sh", &script);

    // The daemon's own environment holds the key under its configured name.
    let env = env_with(&[("MEDULLA_CLAUDE_BIN", &bin), ("MEDULLA_ROUTER_KEY", SECRET)]);
    let router =
        router_cfg(r#"{"baseUrl":"https://gw/anthropic","apiKeyEnv":"MEDULLA_ROUTER_KEY"}"#);
    let options = router_options(HarnessProvider::Claude, &bin, env, Some(router));
    let result = run_provider_task(options).await.expect("router run ok");

    // The child saw the routed endpoint...
    assert_eq!(result.reply, "endpoint=https://gw/anthropic");
    // ...and received the key resolved from the daemon env BY NAME.
    let seen = std::fs::read_to_string(&marker).expect("marker written");
    assert_eq!(seen, SECRET, "child received the resolved AUTH token");
    // ...but the secret NEVER appears in the reply frame.
    assert!(
        !result.reply.contains(SECRET),
        "the key value must not leak into the reply frame"
    );
}

#[tokio::test]
async fn router_codex_endpoint_only_preserves_existing_credentials() {
    // No apiKeyEnv → the router steers OPENAI_BASE_URL and leaves the harness's
    // own OPENAI_API_KEY untouched (endpoint-only routing).
    let dir = TempDir::new();
    let marker = dir.path().join("codex-env");
    let marker = marker.to_string_lossy().into_owned();
    let script = format!(
        "#!/bin/sh\n\
         printf 'base=%s key=%s' \"$OPENAI_BASE_URL\" \"$OPENAI_API_KEY\" > '{marker}'\n\
         printf '{{\"type\":\"event_msg\",\"timestamp\":\"2026-07-05T00:00:00Z\",\"payload\":{{\"type\":\"agent_message\",\"message\":\"ok\"}}}}\\n'\n",
    );
    let bin = dir.write_script("router_codex.sh", &script);
    let env = env_with(&[
        ("MEDULLA_CODEX_BIN", &bin),
        ("OPENAI_API_KEY", "pre-existing-key"),
    ]);
    let router = router_cfg(r#"{"baseUrl":"https://gw/v1"}"#);
    let options = router_options(HarnessProvider::Codex, &bin, env, Some(router));
    let result = run_provider_task(options).await.expect("router run ok");
    assert_eq!(result.reply, "ok");
    let seen = std::fs::read_to_string(&marker).expect("marker written");
    assert_eq!(seen, "base=https://gw/v1 key=pre-existing-key");
}

#[tokio::test]
async fn router_missing_key_env_is_a_hard_error_not_a_silent_empty_key() {
    // apiKeyEnv names a var that is absent from the daemon env → explicit error
    // (surfaced upstream as an error frame), never a silent unauthenticated spawn.
    let env = env_with(&[("MEDULLA_CODEX_BIN", "/nonexistent/codex")]);
    let router = router_cfg(r#"{"baseUrl":"https://gw/v1","apiKeyEnv":"ABSENT_ROUTER_KEY"}"#);
    let options = router_options(HarnessProvider::Codex, "codex", env, Some(router));
    let err = run_provider_task(options)
        .await
        .expect_err("missing router key must error");
    assert!(
        err.contains("ABSENT_ROUTER_KEY"),
        "names the missing var: {err}"
    );
    assert!(err.contains("not set"), "explains the failure: {err}");
}

#[tokio::test]
async fn no_router_config_spawns_child_unchanged() {
    // router: None → zero behaviour change; the endpoint env is never set.
    let dir = TempDir::new();
    let marker = dir.path().join("no-router");
    let marker = marker.to_string_lossy().into_owned();
    let script = format!(
        "#!/bin/sh\n\
         printf 'base=[%s]' \"$ANTHROPIC_BASE_URL\" > '{marker}'\n\
         printf '{{\"type\":\"result\",\"result\":\"done\"}}\\n'\n",
    );
    let bin = dir.write_script("no_router_claude.sh", &script);
    let env = env_with(&[("MEDULLA_CLAUDE_BIN", &bin)]);
    let options = router_options(HarnessProvider::Claude, &bin, env, None);
    let result = run_provider_task(options).await.expect("run ok");
    assert_eq!(result.reply, "done");
    let seen = std::fs::read_to_string(&marker).expect("marker written");
    assert_eq!(seen, "base=[]", "no router → ANTHROPIC_BASE_URL is unset");
}

#[tokio::test]
async fn router_loaded_from_config_file_reaches_the_spawned_child() {
    // The config-load → daemon-population wiring, end to end: a `[router]` section
    // on disk is parsed by `load_config` into the exact `RouterConfig` the spawn
    // seam accepts, and its endpoint reaches the child. This is the seam the
    // standalone daemon and worker TUI use to populate `DaemonConfig.router`.
    let dir = TempDir::new();
    // An explicit config file with a router endpoint (camelCase on the wire).
    let config_path = dir.path().join("medulla.tui.json");
    std::fs::write(
        &config_path,
        r#"{"router":{"baseUrl":"https://gw/anthropic","apiKeyEnv":"MEDULLA_ROUTER_KEY"}}"#,
    )
    .expect("write config");

    // Load it the way the daemon does; env only carries a home so discovery is
    // deterministic (the explicit path bypasses layer discovery regardless).
    let load_env: HashMap<String, String> = [(
        "MEDULLA_HOME".to_string(),
        dir.path().to_string_lossy().into_owned(),
    )]
    .into_iter()
    .collect();
    let loaded = medulla::config::load_config(
        Some(config_path.to_string_lossy().as_ref()),
        &load_env,
        dir.path(),
    )
    .expect("config loads");
    let router = loaded
        .config
        .router
        .clone()
        .expect("[router] section populated the daemon config");

    // Feed the loaded router into the real spawn path; the child must see it.
    // A clearly-synthetic, non-token-shaped marker (avoids secret scanning).
    const SECRET: &str = "CONFIG-FILE-KEY-VALUE-2b7c";
    let marker = dir.path().join("cfg-key-marker");
    let marker = marker.to_string_lossy().into_owned();
    let script = format!(
        "#!/bin/sh\n\
         printf '%s' \"$ANTHROPIC_AUTH_TOKEN\" > '{marker}'\n\
         printf '{{\"type\":\"result\",\"result\":\"endpoint=%s\"}}\\n' \"$ANTHROPIC_BASE_URL\"\n",
    );
    let bin = dir.write_script("cfg_router_claude.sh", &script);
    let env = env_with(&[("MEDULLA_CLAUDE_BIN", &bin), ("MEDULLA_ROUTER_KEY", SECRET)]);
    let options = router_options(HarnessProvider::Claude, &bin, env, Some(router));
    let result = run_provider_task(options).await.expect("router run ok");

    assert_eq!(result.reply, "endpoint=https://gw/anthropic");
    let seen = std::fs::read_to_string(&marker).expect("marker written");
    assert_eq!(seen, SECRET, "child received the key resolved by name");
}

#[tokio::test]
async fn an_openrouter_router_reaches_the_child_as_the_local_proxy_never_the_key() {
    // The invariant this whole feature rests on, observed from inside a real
    // spawned child: an OpenRouter-bound run gives the harness a loopback
    // endpoint and a machine-local token, and the operator's key is nowhere in
    // its environment. A harness that still held the key could ignore the
    // endpoint and call OpenRouter directly, unattributed.
    const SECRET: &str = "OPENROUTER-KEY-VALUE-DO-NOT-LEAK-4c1e";
    let dir = TempDir::new();
    let marker = dir.path().join("openrouter-marker");
    let marker = marker.to_string_lossy().into_owned();
    // Dumps the whole environment out of band so the assertions can prove a
    // negative: that the key is absent under *any* name, not merely the one we
    // thought to check.
    let script = format!(
        "#!/bin/sh\n\
         env > '{marker}'\n\
         printf '{{\"type\":\"result\",\"result\":\"endpoint=%s token=%s\"}}\\n' \
         \"$ANTHROPIC_BASE_URL\" \"$ANTHROPIC_AUTH_TOKEN\"\n",
    );
    let bin = dir.write_script("openrouter_claude.sh", &script);

    let env = env_with(&[
        ("MEDULLA_CLAUDE_BIN", &bin),
        ("OPENROUTER_API_KEY", SECRET),
        // Keeps the proxy off the network. It is never called in this test — the
        // fake harness does not make a request — but a listener that *could*
        // reach openrouter.ai has no business existing in the suite.
        ("MEDULLA_OPENROUTER_URL", "http://127.0.0.1:1/api"),
    ]);
    let router =
        router_cfg(r#"{"baseUrl":"https://openrouter.ai/api","apiKeyEnv":"OPENROUTER_API_KEY"}"#);
    let options = router_options(HarnessProvider::Claude, &bin, env, Some(router));
    let result = run_provider_task(options).await.expect("router run ok");

    // Pointed at the local proxy rather than at OpenRouter.
    assert!(
        result.reply.contains("endpoint=http://127.0.0.1:"),
        "child should be routed through the loopback proxy: {}",
        result.reply
    );
    assert!(
        result.reply.contains("/anthropic"),
        "claude should land on the Anthropic-dialect mount: {}",
        result.reply
    );
    assert!(
        result.reply.contains("token=mdl-"),
        "child should hold a loopback token: {}",
        result.reply
    );

    // And the operator's key is gone from the child's environment entirely.
    let child_env = std::fs::read_to_string(&marker).expect("marker written");
    assert!(
        !child_env.contains(SECRET),
        "the OpenRouter key must not survive anywhere in the child environment"
    );
    assert!(
        !child_env.contains("OPENROUTER_API_KEY="),
        "the key variable itself must be scrubbed"
    );
}

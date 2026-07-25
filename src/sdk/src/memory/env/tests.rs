//! Tests for the env module.

use super::*;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn section() -> MemoryConfigSection {
    MemoryConfigSection {
        enabled: Some(true),
        workspace: Some("/cfg/ws".into()),
        identity: Some("cfg@example.com".into()),
        claude_root: Some("/cfg/claude".into()),
        codex_root: Some("/cfg/codex".into()),
        project_roots: vec!["/cfg/a".into(), "/cfg/b".into()],
        model: Some("cfg/model".into()),
        max_cost_usd: Some(2.5),
    }
}

#[test]
fn enabled_env_beats_config_both_ways() {
    // Config on, env off → off.
    assert!(!enabled(Some(&section()), &env(&[("MEDULLA_MEMORY", "0")])));
    // Config absent, env on → on.
    assert!(enabled(None, &env(&[("MEDULLA_MEMORY", "true")])));
    // Config on, no env → on.
    assert!(enabled(Some(&section()), &env(&[])));
    // Nothing → ON by default.
    assert!(enabled(None, &env(&[])));
    // Config off, no env → off.
    let mut off = section();
    off.enabled = Some(false);
    assert!(!enabled(Some(&off), &env(&[])));
}

#[test]
fn workspace_precedence() {
    let home = PathBuf::from("/home/u/.medulla");
    // Default is `<medulla_home>/memory`.
    assert_eq!(
        workspace(None, &env(&[]), &home),
        PathBuf::from("/home/u/.medulla/memory")
    );
    // Config beats default.
    assert_eq!(
        workspace(Some(&section()), &env(&[]), &home),
        PathBuf::from("/cfg/ws")
    );
    // Env beats config.
    assert_eq!(
        workspace(
            Some(&section()),
            &env(&[("TINYCORTEX_WORKSPACE", "/env/ws")]),
            &home
        ),
        PathBuf::from("/env/ws")
    );
}

#[test]
fn identity_and_model_precedence() {
    assert_eq!(identity(None, &env(&[])), "");
    assert_eq!(identity(Some(&section()), &env(&[])), "cfg@example.com");
    assert_eq!(
        identity(Some(&section()), &env(&[("PERSONA_IDENTITY", "env@x")])),
        "env@x"
    );
    assert_eq!(llm_model(None, &env(&[])), None);
    assert_eq!(
        llm_model(Some(&section()), &env(&[])).as_deref(),
        Some("cfg/model")
    );
    assert_eq!(
        llm_model(Some(&section()), &env(&[("TINYCORTEX_LLM_MODEL", "env/m")])).as_deref(),
        Some("env/m")
    );
}

#[test]
fn roots_precedence_and_split() {
    assert!(project_roots(None, &env(&[])).is_empty());
    assert_eq!(
        project_roots(Some(&section()), &env(&[])),
        vec![PathBuf::from("/cfg/a"), PathBuf::from("/cfg/b")]
    );
    assert_eq!(
        project_roots(
            Some(&section()),
            &env(&[("PERSONA_PROJECT_ROOTS", " /env/x , /env/y ,")])
        ),
        vec![PathBuf::from("/env/x"), PathBuf::from("/env/y")]
    );
    assert_eq!(
        claude_root(Some(&section()), &env(&[])),
        Some(PathBuf::from("/cfg/claude"))
    );
    assert_eq!(
        codex_root(None, &env(&[("PERSONA_CODEX_ROOT", "/env/codex")])),
        Some(PathBuf::from("/env/codex"))
    );
    assert_eq!(claude_root(None, &env(&[])), None);
}

#[test]
fn max_cost_precedence_and_guards() {
    assert_eq!(max_cost_usd(None, &env(&[])), DEFAULT_MAX_COST_USD);
    assert_eq!(max_cost_usd(Some(&section()), &env(&[])), 2.5);
    assert_eq!(
        max_cost_usd(Some(&section()), &env(&[("PERSONA_MAX_COST_USD", "9.0")])),
        9.0
    );
    // Non-positive / garbage env falls back to config.
    for bad in ["0", "-1", "abc"] {
        assert_eq!(
            max_cost_usd(Some(&section()), &env(&[("PERSONA_MAX_COST_USD", bad)])),
            2.5
        );
    }
}

#[test]
fn api_key_presence() {
    assert_eq!(openrouter_api_key(&env(&[])), None);
    assert_eq!(
        openrouter_api_key(&env(&[("OPENROUTER_API_KEY", "  ")])),
        None
    );
    assert_eq!(
        openrouter_api_key(&env(&[("OPENROUTER_API_KEY", "sk-x")])).as_deref(),
        Some("sk-x")
    );
}

#[test]
fn resolve_with_backend_attaches_token_from_env() {
    use crate::config::BackendConfig;
    let home = PathBuf::from("/home/u/.medulla");
    let backend = BackendConfig::default();
    // Token from the env var → backend inference target is attached.
    let s = resolve_with_backend(
        None,
        &backend,
        &env(&[(&backend.token_env, "jwt-from-env")]),
        &home,
    );
    let attached = s.backend.expect("backend attached");
    assert_eq!(attached.base_url, backend.base_url);
    assert_eq!(attached.jwt, "jwt-from-env");
}

#[test]
fn resolve_with_backend_leaves_backend_none_without_token() {
    use crate::config::BackendConfig;
    // A temp home with no credentials file and no env token → no backend.
    let home = tempfile::tempdir().unwrap();
    let s = resolve_with_backend(None, &BackendConfig::default(), &env(&[]), home.path());
    assert_eq!(s.backend, None);
}

#[test]
fn resolve_composes_all_knobs() {
    let home = PathBuf::from("/home/u/.medulla");
    let s = resolve(
        Some(&section()),
        &env(&[("OPENROUTER_API_KEY", "sk-x"), ("MEDULLA_MEMORY", "1")]),
        &home,
    );
    assert!(s.enabled);
    assert_eq!(s.workspace, PathBuf::from("/cfg/ws"));
    assert_eq!(s.identity, "cfg@example.com");
    assert_eq!(s.openrouter_api_key.as_deref(), Some("sk-x"));
    assert_eq!(s.max_cost_usd, 2.5);
    assert_eq!(s.project_roots.len(), 2);
    assert_eq!(s.backend, None);
    let s = s.with_backend("http://b:1", "jwt-x");
    let backend = s.backend.unwrap();
    assert_eq!(backend.base_url, "http://b:1");
    assert_eq!(backend.jwt, "jwt-x");
}

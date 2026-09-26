//! Tests for the worker profile module.

use super::*;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn username_prefers_user_then_username_then_fallback() {
    assert_eq!(env_username(&env(&[("USER", "ada")])), "ada");
    assert_eq!(env_username(&env(&[("USERNAME", "grace")])), "grace");
    assert_eq!(
        env_username(&env(&[("USER", "ada"), ("USERNAME", "grace")])),
        "ada"
    );
    // Blank USER falls through to USERNAME.
    assert_eq!(
        env_username(&env(&[("USER", "  "), ("USERNAME", "grace")])),
        "grace"
    );
    assert_eq!(env_username(&env(&[])), "worker");
}

#[test]
fn hostname_reads_env_and_skips_blank() {
    assert_eq!(
        env_hostname(&env(&[("HOSTNAME", "box-1")])).as_deref(),
        Some("box-1")
    );
    assert_eq!(env_hostname(&env(&[("HOSTNAME", "  ")])), None);
    assert_eq!(env_hostname(&env(&[])), None);
}

#[test]
fn compose_matches_the_documented_shape() {
    assert_eq!(
        compose_worker_name("ada", "box", "10.0.0.4"),
        "ada@box/10.0.0.4"
    );
}

#[test]
fn default_worker_name_uses_injected_env() {
    let name = default_worker_name(&env(&[("USER", "ada"), ("HOSTNAME", "box-1")]));
    // Username + hostname are deterministic; the IP part is best-effort.
    assert!(name.starts_with("ada@box-1/"), "got {name}");
    assert!(name.contains('/'), "has an ip segment: {name}");
}

#[test]
fn default_worker_name_falls_back_when_env_missing() {
    // No USER/USERNAME/HOSTNAME: username defaults to "worker"; the hostname
    // may come from the `hostname` command or the "localhost" fallback.
    let name = default_worker_name(&env(&[]));
    assert!(name.starts_with("worker@"), "got {name}");
}

#[test]
fn primary_ipv4_is_always_a_string() {
    // Best-effort: never panics, always returns something ip-shaped or the
    // loopback fallback.
    let ip = primary_ipv4();
    assert!(!ip.is_empty());
}

#[test]
fn profile_path_is_worker_json_under_home() {
    let e = env(&[("MEDULLA_HOME", "/home/me/.medulla")]);
    let p = profile_path(&e);
    assert!(p.ends_with(".medulla/local/worker.json"), "got {p:?}");
}

#[test]
fn profile_round_trips_through_disk() {
    let dir = std::env::temp_dir().join(format!("medulla-wp-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("worker.json");
    let profile = WorkerProfile {
        name: "ada@box/10.0.0.4".to_string(),
        address: "AgentAddr111".to_string(),
        owner: Some("@overseer".to_string()),
        registered_at: Some("2026-07-18T00:00:00Z".to_string()),
    };
    profile.save(&path).unwrap();
    let loaded = WorkerProfile::load(&path).expect("profile loads");
    assert_eq!(loaded, profile);
    // The on-disk file uses the camelCase key.
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("\"registeredAt\""), "camelCase key: {raw}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn load_missing_or_corrupt_is_none() {
    assert_eq!(WorkerProfile::load(Path::new("/no/such/worker.json")), None);

    let dir = std::env::temp_dir().join(format!("medulla-wp-bad-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("worker.json");
    std::fs::write(&path, b"{ not json").unwrap();
    assert_eq!(WorkerProfile::load(&path), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn registered_requires_profile_and_identity() {
    let profile = WorkerProfile {
        name: "w".to_string(),
        ..Default::default()
    };
    assert!(is_registered(Some(&profile), true));
    assert!(!is_registered(Some(&profile), false));
    assert!(!is_registered(None, true));
    assert!(!is_registered(None, false));
}

#[test]
fn owner_is_omitted_from_json_when_absent() {
    let profile = WorkerProfile {
        name: "w".to_string(),
        ..Default::default()
    };
    let json = serde_json::to_string(&profile).unwrap();
    assert!(!json.contains("owner"), "no owner key: {json}");
    assert!(
        !json.contains("registeredAt"),
        "no registeredAt key: {json}"
    );
    assert!(!json.contains("address"), "no address key: {json}");
}

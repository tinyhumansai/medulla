//! End-to-end tests for the release update checker and self-updater
//! ([`medulla::update`]) against a hand-rolled stub HTTP server (the same
//! TcpListener pattern the other e2e mocks use). No real network, no GitHub.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use medulla::update::{check_for_update, download_and_stage, platform_key, update_url};

/// A one-body stub HTTP server: every GET on any path gets the same bytes and
/// content type. Returns the base `http://127.0.0.1:<port>` URL and keeps
/// serving until dropped.
struct StubServer {
    base_url: String,
    _handle: tokio::task::JoinHandle<()>,
}

impl StubServer {
    async fn serve(body: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = Arc::new(body);
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let body = body.clone();
                tokio::spawn(async move {
                    // Drain the request headers (best effort; one small read).
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(header.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
                    let _ = sock.flush().await;
                });
            }
        });
        StubServer {
            base_url: format!("http://{addr}"),
            _handle: handle,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }
}

fn manifest_json(version: &str, target: &str, asset_url: &str, sha256: &str) -> String {
    format!(
        r#"{{
            "version": "{version}",
            "tag": "v{version}",
            "pubDate": "2026-07-18T00:00:00Z",
            "notes": "https://github.com/tinyhumansai/medulla/releases/tag/v{version}",
            "platforms": {{
                "{target}": {{ "url": "{asset_url}", "sha256": "{sha256}" }}
            }}
        }}"#
    )
}

#[tokio::test]
async fn check_reports_newer_version_for_this_platform() {
    let body = manifest_json(
        "999.0.0",
        platform_key(),
        "https://example/medulla.tar.gz",
        "deadbeef",
    );
    let server = StubServer::serve(body.into_bytes()).await;

    let info = check_for_update(&server.url("/latest.json"), "1.0.0")
        .await
        .unwrap()
        .expect("a newer version should be reported");
    assert_eq!(info.version, "999.0.0");
    assert_eq!(info.tag, "v999.0.0");
    assert_eq!(info.url, "https://example/medulla.tar.gz");
    assert_eq!(info.sha256, "deadbeef");
}

#[tokio::test]
async fn check_returns_none_when_current_is_latest() {
    let body = manifest_json(
        "1.0.0",
        platform_key(),
        "https://example/medulla.tar.gz",
        "deadbeef",
    );
    let server = StubServer::serve(body.into_bytes()).await;

    let info = check_for_update(&server.url("/latest.json"), "1.0.0")
        .await
        .unwrap();
    assert!(info.is_none(), "same version must not be an update");
}

#[tokio::test]
async fn check_returns_none_when_no_asset_for_platform() {
    // A newer version, but only ships an asset for a bogus platform key.
    let body = manifest_json(
        "999.0.0",
        "some-other-triple",
        "https://example/medulla.tar.gz",
        "deadbeef",
    );
    let server = StubServer::serve(body.into_bytes()).await;

    let info = check_for_update(&server.url("/latest.json"), "1.0.0")
        .await
        .unwrap();
    assert!(info.is_none(), "no asset for this platform → no update");
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn check_honors_medulla_update_url_env() {
    let body = manifest_json(
        "999.0.0",
        platform_key(),
        "https://example/medulla.tar.gz",
        "deadbeef",
    );
    let server = StubServer::serve(body.into_bytes()).await;

    let _guard = UPDATE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("MEDULLA_UPDATE_URL", server.url("/latest.json"));
    let resolved = update_url();
    assert_eq!(resolved, server.url("/latest.json"));
    let info = check_for_update(&resolved, "1.0.0").await.unwrap();
    std::env::remove_var("MEDULLA_UPDATE_URL");
    assert!(info.is_some());
}

/// Build a real `.tar.gz` containing a `medulla` binary, serve it, then run the
/// full download → sha-verify → extract → atomic-install path against an
/// injectable "current exe" location.
#[cfg(unix)]
#[tokio::test]
async fn self_update_downloads_verifies_extracts_and_installs() {
    use medulla::update::{install_binary, sha256_hex};
    use std::process::Command;

    let work = temp_dir("selfupdate");
    // Lay out medulla-v9.9.9-<triple>/medulla and tar it up.
    let pkg = format!("medulla-v9.9.9-{}", platform_key());
    let pkg_dir = work.join(&pkg);
    std::fs::create_dir_all(&pkg_dir).unwrap();
    std::fs::write(pkg_dir.join("medulla"), b"#!/bin/sh\necho NEW-BINARY\n").unwrap();
    let tarball = work.join("asset.tar.gz");
    let status = Command::new("tar")
        .arg("-czf")
        .arg(&tarball)
        .arg("-C")
        .arg(&work)
        .arg(&pkg)
        .status()
        .unwrap();
    assert!(status.success());
    let bytes = std::fs::read(&tarball).unwrap();
    let sha = sha256_hex(&bytes);

    // The asset URL must end in .tar.gz so the stager picks tar extraction.
    let server = StubServer::serve(bytes).await;
    let asset_url = server.url("/medulla-v9.9.9.tar.gz");

    let stage = work.join("stage");
    std::fs::create_dir_all(&stage).unwrap();
    let staged = download_and_stage(&asset_url, &sha, &stage).await.unwrap();
    assert_eq!(staged.file_name().unwrap(), "medulla");
    assert_eq!(
        std::fs::read(&staged).unwrap(),
        b"#!/bin/sh\necho NEW-BINARY\n"
    );

    // Install over an injected "current exe" path (the seam).
    let target = work.join("installed").join("medulla");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, b"OLD-BINARY").unwrap();
    install_binary(&staged, &target).unwrap();
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"#!/bin/sh\necho NEW-BINARY\n"
    );
    // The previous binary is preserved for rollback.
    let backup = medulla::update::backup_path(&target);
    assert_eq!(std::fs::read(&backup).unwrap(), b"OLD-BINARY");

    let _ = std::fs::remove_dir_all(&work);
}

#[tokio::test]
async fn self_update_rejects_sha_mismatch() {
    let server = StubServer::serve(b"not a real archive".to_vec()).await;
    let stage = temp_dir("shamismatch");
    let err = download_and_stage(
        &server.url("/medulla.tar.gz"),
        "0000000000000000000000000000000000000000000000000000000000000000",
        &stage,
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("sha256 mismatch"),
        "expected sha256 mismatch, got: {err}"
    );
    let _ = std::fs::remove_dir_all(&stage);
}

/// Serializes the process-env mutation the `run_update` tests share with
/// `check_honors_medulla_update_url_env`.
static UPDATE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// The lock is intentionally held across awaits to serialize process-env access
// against other tests in this binary.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn run_update_reports_up_to_date() {
    use medulla::update::run_update;
    // Serve a manifest whose version equals the running binary's version.
    let current = env!("CARGO_PKG_VERSION");
    let body = manifest_json(current, platform_key(), "https://example/a.tar.gz", "aa");
    let server = StubServer::serve(body.into_bytes()).await;

    let _guard = UPDATE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("MEDULLA_UPDATE_URL", server.url("/latest.json"));
    // Both check-only and full run take the "up to date" branch (no install).
    run_update(true).await.unwrap();
    run_update(false).await.unwrap();
    std::env::remove_var("MEDULLA_UPDATE_URL");
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn run_update_check_only_reports_available() {
    use medulla::update::run_update;
    let body = manifest_json("999.0.0", platform_key(), "https://example/a.tar.gz", "aa");
    let server = StubServer::serve(body.into_bytes()).await;

    let _guard = UPDATE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("MEDULLA_UPDATE_URL", server.url("/latest.json"));
    // check_only path: prints the available update + notes without installing.
    run_update(true).await.unwrap();
    std::env::remove_var("MEDULLA_UPDATE_URL");
}

#[test]
fn exe_is_writable_reflects_directory_permissions() {
    use medulla::update::exe_is_writable;
    let dir = temp_dir("writable");
    // A file inside a writable temp dir → we can install alongside it.
    let exe = dir.join("medulla");
    std::fs::write(&exe, b"bin").unwrap();
    assert!(exe_is_writable(&exe));

    // A path whose parent does not exist → not writable.
    let missing = dir.join("no-such-dir").join("medulla");
    assert!(!exe_is_writable(&missing));
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[tokio::test]
async fn download_and_stage_rejects_corrupt_archive() {
    // Correct sha, but the bytes are not a valid tar.gz → extraction fails.
    use medulla::update::sha256_hex;
    let bytes = b"this is not a tarball".to_vec();
    let sha = sha256_hex(&bytes);
    let server = StubServer::serve(bytes).await;
    let stage = temp_dir("corrupt");
    let err = download_and_stage(&server.url("/asset.tar.gz"), &sha, &stage)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("extraction failed"),
        "expected extraction failure, got: {err}"
    );
    let _ = std::fs::remove_dir_all(&stage);
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "medulla-update-e2e-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

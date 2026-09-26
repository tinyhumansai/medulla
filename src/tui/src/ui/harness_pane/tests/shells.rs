//! Tests for the plain-shell session: which shells the picker offers, and what
//! a shell is spawned with.
//!
//! The second half asserts against a real child, not the launch spec, because
//! the whole point of the shell door is what it *omits* — an MCP registration
//! or a commit-attribution variable that leaked into it would be invisible in
//! any assertion made before the spawn.
//!
//! Unix-only, like its siblings: it runs stand-in scripts on a real pty.

use std::collections::HashMap;

use medulla::protocol::HarnessProvider;

use crate::worker::pty::{PtyManager, SessionControl, SessionOrigin};

use super::super::shells::{available, ShellChoice};
use super::super::HarnessChoice;
use super::session::{harnesses, wait_for};

/// A lookup that finds exactly the named binaries.
fn installed(names: &'static [&'static str]) -> impl Fn(&str) -> bool {
    move |bin: &str| names.contains(&bin)
}

#[test]
fn the_operators_own_shell_is_offered_first() {
    let env = HashMap::from([("SHELL".to_string(), "/usr/bin/zsh".to_string())]);

    let offered = available(&env, &installed(&["/usr/bin/zsh", "bash", "sh"]));

    assert_eq!(
        offered,
        vec![
            ShellChoice {
                name: "zsh".to_string(),
                bin: "/usr/bin/zsh".to_string(),
            },
            ShellChoice {
                name: "bash".to_string(),
                bin: "bash".to_string(),
            },
            ShellChoice {
                name: "sh".to_string(),
                bin: "sh".to_string(),
            },
        ],
        "$SHELL leads, and only installed shells are offered"
    );
}

/// `$SHELL=/bin/zsh` and the `zsh` on `PATH` are one shell wearing two names.
/// Offering both would make the picker's rows indistinguishable — two entries
/// reading `zsh`, one of which is the other.
#[test]
fn a_shell_named_twice_is_offered_once() {
    let env = HashMap::from([("SHELL".to_string(), "/bin/zsh".to_string())]);

    let offered = available(&env, &installed(&["/bin/zsh", "zsh"]));

    assert_eq!(offered.len(), 1, "{offered:?}");
    assert_eq!(offered[0].bin, "/bin/zsh", "the operator's own path wins");
}

/// A host that pins `MEDULLA_SHELL_BIN` gets that shell first, without the pin
/// hiding the interactive shells someone would rather type in.
#[test]
fn a_pinned_shell_leads_without_hiding_the_others() {
    let env = HashMap::from([
        ("MEDULLA_SHELL_BIN".to_string(), "/bin/sh".to_string()),
        ("SHELL".to_string(), "/bin/zsh".to_string()),
    ]);

    let offered = available(&env, &installed(&["/bin/sh", "bash"]));

    assert_eq!(offered[0].name, "sh");
    assert!(
        offered.iter().any(|shell| shell.name == "bash"),
        "{offered:?}"
    );
}

#[test]
fn a_machine_with_no_shell_on_path_offers_none() {
    assert!(available(&HashMap::new(), &installed(&[])).is_empty());
}

/// The picker lists shells after every coding agent: Enter on a freshly opened
/// modal must still start the harness the operator opened it for.
#[test]
fn the_picker_offers_shells_after_the_coding_agents() {
    let mut harnesses = harnesses(PtyManager::new());
    harnesses.providers = vec![HarnessProvider::Codex];
    harnesses
        .env
        .insert("SHELL".to_string(), "/bin/sh".to_string());

    let choices = harnesses.choices();
    let labels = choices
        .iter()
        .map(HarnessChoice::display_name)
        .collect::<Vec<_>>();

    assert_eq!(labels, ["Codex", "sh"], "{labels:?}");
    assert!(choices[1].is_shell());
    assert_eq!(choices[1].provider, HarnessProvider::Shell);
    assert_eq!(choices[1].id(), "sh");
}

/// What a shell is *not* given. Every one of these belongs to a coding agent:
/// an MCP registration it can call tools through, and a commit trailer saying
/// Medulla wrote the code. A person typing at a prompt is doing neither, and
/// the trailer would put Medulla's name on their own commits.
#[cfg(unix)]
#[test]
fn a_shell_session_is_spawned_bare() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let record = dir.path().join("record");
    let bin = dir.path().join("fake-shell");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\n{{ printf 'argc:%s\\n' \"$#\"; env; }} > {}\nsleep 30\n",
            record.display()
        ),
    )
    .expect("the stand-in shell is writable");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .expect("the stand-in shell is executable");
    }

    let sessions = PtyManager::new();
    let mut harnesses = harnesses(sessions.clone());
    harnesses.workspace = dir.path().to_string_lossy().into_owned();
    // The one variable a shell must not inherit: a `cargo test` started from it
    // would otherwise resolve the live credential store as its keyring.
    harnesses.env.insert(
        "OPENHUMAN_WORKSPACE".to_string(),
        "/the/live/core".to_string(),
    );

    let choice = HarnessChoice::shell(ShellChoice {
        name: "fake-shell".to_string(),
        bin: bin.to_string_lossy().into_owned(),
    });
    let id = harnesses
        .open_unmanaged(&choice, "", false)
        .expect("the stand-in shell starts");
    wait_for("the child to record its environment", || record.exists());
    let recorded = std::fs::read_to_string(&record).expect("the record was written");
    let row = sessions.row(&id).expect("the session is listed");
    sessions.close(&id);

    assert!(
        recorded.contains("argc:0"),
        "a shell is handed a tty and nothing else — no argv at all: {recorded}"
    );
    assert!(
        !recorded.contains("MEDULLA_ATTRIBUTION"),
        "commits typed in a shell are the operator's own: {recorded}"
    );
    assert!(
        !recorded.contains(medulla::control_socket::MCP_GRANT_ENV),
        "a shell calls no Medulla tools, so it is minted no grant: {recorded}"
    );
    assert!(
        !recorded.contains("OPENHUMAN_WORKSPACE"),
        "the embedded core's state must not reach a shell: {recorded}"
    );
    assert_eq!(row.provider, HarnessProvider::Shell);
    assert_eq!(row.control, SessionControl::User);
    assert_eq!(row.origin, SessionOrigin::User);
    assert_eq!(row.cwd, dir.path().to_string_lossy());
}

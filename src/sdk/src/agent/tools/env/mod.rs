//! Deciding what a model-authored command may see in its environment.
//!
//! # Why `env_clear` was not enough
//!
//! The shell runs under `env_clear()` plus an explicit map, which is the right
//! shape — but the map a daemon hands down is
//! `std::env::vars().collect()`, so installing it verbatim reinstates exactly
//! what clearing removed. Disabling inheritance and then passing the ambient
//! environment through is not a boundary; it is the same environment with extra
//! steps.
//!
//! # Denylist, not allowlist
//!
//! An allowlist is the safer instinct and the wrong tool here. A command the
//! model writes is an ordinary build or test invocation — it needs `PATH`,
//! `HOME`, `LANG`, `CARGO_HOME`, `npm_config_*`, the platform's dozen
//! Windows-isms, and whatever else the operator's toolchain reads. Enumerating
//! that set means guessing, and every miss is a build that fails for a reason
//! the model cannot see or fix. The cost of a miss in the other direction is
//! bounded and inspectable: a credential-shaped name that is not a credential
//! gets dropped, and the fix is to rename it.
//!
//! # What counts as a credential
//!
//! A name is treated as one when it contains a secret-bearing word. That is a
//! heuristic over a convention, and it is deliberately broad — `TOKEN`, `KEY`,
//! `SECRET`, `PASSWORD`, `PASSWD`, `CREDENTIAL`, `AUTH`, `SESSION`. Matching a
//! word inside the name rather than a suffix catches `AWS_SECRET_ACCESS_KEY`
//! and `MEDULLA_TOKEN` alike.
//!
//! Two consequences worth stating rather than discovering:
//!
//! * `SSH_AUTH_SOCK` is dropped, so a command cannot reach the operator's ssh
//!   agent — which means `git push` over ssh will not authenticate from a turn.
//!   That is the intended reading: an agent socket is a credential that happens
//!   to be a file descriptor.
//! * `GITHUB_TOKEN` and friends are dropped even though a build might want one.
//!   A turn that genuinely needs to authenticate should be given a scoped
//!   credential deliberately, not inherit the operator's.

use std::collections::HashMap;

/// Words whose presence in a name means the value is a credential.
const SECRET_WORDS: [&str; 8] = [
    "TOKEN",
    "KEY",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "AUTH",
    "SESSION",
];

/// Names dropped regardless of shape.
///
/// These carry no secret-bearing word but still hand a turn something it should
/// not have: the account home is where the session store lives, and a command
/// that can read it does not need the bearer in its environment to find one.
const ALWAYS_DROPPED: [&str; 2] = ["MEDULLA_HOME", "MEDULLA_USER"];

/// Whether `name` looks like it holds a credential.
///
/// Case-insensitive: the OS treats environment names case-insensitively on
/// Windows and `std::env::vars()` reports them as stored, so a case-sensitive
/// rule would pass `Github_Token` straight through.
pub fn is_secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ALWAYS_DROPPED.contains(&upper.as_str()) || SECRET_WORDS.iter().any(|word| upper.contains(word))
}

/// `env` with every credential-shaped entry removed.
///
/// The result is what a model-authored command may see. Everything a build
/// legitimately needs survives; see the module docs for why the rule is a
/// denylist and what it deliberately costs.
pub fn scrubbed(env: &HashMap<String, String>) -> HashMap<String, String> {
    env.iter()
        .filter(|(name, _)| !is_secret_name(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

#[cfg(test)]
mod tests;

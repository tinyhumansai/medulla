//! Who may use Medulla, and why.
//!
//! The product is open to any registered account that either pays for at least
//! the Basic tier or is still inside its first 30 days. That rule lives here, in
//! the binary, rather than on the backend:
//! [`crate::client::MedullaClient::me`] reports *facts* — the plan, the
//! registration instant, the server's clock — and [`decide`] turns them into a
//! verdict.
//!
//! # The rule
//!
//! In order, first match wins:
//!
//! 1. The admin allowlist flag (`hasMedullaAccess`) is on → in. This predates
//!    the plan gate and keeps staff, partners, and comped accounts working.
//! 2. The account's address is on [`STAFF_DOMAIN`] → in. We are not the
//!    audience for our own paywall, and an internal account should not be
//!    stopped at it because somebody forgot to set the allowlist flag.
//! 3. The plan is [`Plan::Basic`] or [`Plan::Pro`] → in.
//! 4. The account is younger than [`TRIAL`] → in, for the remainder of it.
//! 5. Past those 30 days on no paid tier → out, with [`Denial::TrialExpired`].
//! 6. Anything else could not be determined → out, with
//!    [`Denial::Undetermined`].
//!
//! # The undetermined case fails closed
//!
//! A response with no `entitlement` block, an unresolvable plan, or no
//! registration instant is refused rather than waved through. The backend this
//! binary ships against always sends the block, so a missing one is a fault, and
//! a gate that opens on its own faults is not a gate.
//!
//! It is refused *as its own reason*, never as an expired trial. The two are
//! different news — one is about the operator's account, the other is about us —
//! and a screen that blamed the operator for our outage would be lying.
//!
//! # This is still not the enforcement point
//!
//! It decides what the terminal shows. What an account may actually spend is
//! metered by the backend on the inference and orchestration routes, where the
//! client cannot argue with it. A patched binary walks straight through this
//! module; treat it as the thing that explains the situation to the operator,
//! not as the thing that holds the line.
//!
//! Split by responsibility: `types` holds the data model, this file the decision
//! and the `/auth/me` extraction.

use std::time::Duration;

mod types;

#[cfg(test)]
mod tests;

pub use types::{Access, Denial, Entitlement, Grant, Plan, ServerClock};

/// How long a newly registered account may use Medulla without paying.
///
/// The first 30 days. Encoded here, in the binary, because this is where the
/// decision is made — the backend reports when an account was created and says
/// nothing about what that entitles it to.
pub const TRIAL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The number of whole days in [`TRIAL`], for the copy that has to name it.
///
/// Derived from the constant rather than written out again, so the screen and
/// the startup note cannot drift from the rule they describe.
pub const TRIAL_DAYS: u64 = TRIAL.as_secs() / (24 * 60 * 60);

/// The email domain whose accounts are ours, and are never shown the paywall.
///
/// Held in lowercase with no leading `@`; [`Entitlement::is_staff`] matches it
/// and any subdomain of it. Like [`TRIAL`], it is a client-side courtesy rather
/// than an entitlement — the backend still meters what these accounts spend.
pub const STAFF_DOMAIN: &str = "tinyhumans.ai";

/// Decide whether `entitlement` may use Medulla, as of `local_now_ms`.
///
/// `local_now_ms` is this machine's clock in milliseconds since the Unix epoch;
/// it is only consulted when the backend sent no clock of its own (see
/// [`Entitlement::account_age_ms`]).
///
/// Pure, and the whole policy: everything above it fetches inputs and
/// everything below it renders the answer.
pub fn decide(entitlement: &Entitlement, local_now_ms: i64) -> Access {
    if entitlement.admin_granted {
        return Access::Granted(Grant::AdminGranted);
    }
    // The subscription flag is not required to agree. A plan that Stripe reports
    // as `BASIC` while `hasActiveSubscription` is false is a billing state we
    // should not resolve against the operator mid-session — the backend's spend
    // limits are what actually meter them.
    // Our own people, ahead of the plan check: an internal account on `Free`
    // whose allowlist flag was never set should still open the app. This is a
    // display decision only — the backend meters their spend like anyone's.
    if entitlement.is_staff() {
        return Access::Granted(Grant::StaffDomain);
    }
    if entitlement.plan.is_paid() {
        return Access::Granted(Grant::Paid(entitlement.plan));
    }
    // A plan nobody could resolve is not a plan anybody may run on, and it is
    // not an expired trial either. Checked before the age so an account that is
    // both unresolvable and old is refused for the reason that is actually true.
    if entitlement.plan == Plan::Unknown {
        return Access::Denied(Denial::Undetermined);
    }

    let trial_ms = TRIAL.as_millis() as i64;
    match entitlement.account_age_ms(local_now_ms) {
        Some(age) if age < trial_ms => Access::Granted(Grant::Trial {
            remaining_ms: trial_ms - age,
        }),
        Some(_) => Access::Denied(Denial::TrialExpired),
        // Confirmed unpaid, but the backend did not say when the account was
        // made — so the free window can neither be granted nor declared spent.
        None => Access::Denied(Denial::Undetermined),
    }
}

/// Extract the [`Entitlement`] facts from an `/auth/me` response.
///
/// Accepts both the flat and `{"user": …}` shapes, the same way
/// [`crate::auth::user_id_from_me`] does and for the same reason: which one a
/// deployment returns has changed before.
///
/// A response with no `entitlement` block at all yields [`Plan::Unknown`] with
/// no timestamps, which [`decide`] refuses as [`Denial::Undetermined`]. Reading
/// the block is the only supported shape: this binary ships against a backend
/// that always sends it.
pub fn entitlement_from_me(me: &serde_json::Value) -> Entitlement {
    let obj = me.get("user").unwrap_or(me);
    let block = obj.get("entitlement");
    let field = |name: &str| block.and_then(|b| b.get(name));

    Entitlement {
        plan: field("plan")
            .and_then(serde_json::Value::as_str)
            .map_or(Plan::Unknown, Plan::parse),
        has_active_subscription: field("hasActiveSubscription")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        admin_granted: field("adminGranted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        // Read from the user object, not the entitlement block: the address is
        // an identity fact, and `/auth/me` reports it alongside the id.
        email: obj
            .get("email")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        registered_at_ms: field("registeredAt")
            .and_then(serde_json::Value::as_str)
            .and_then(crate::clock::epoch_millis_from_iso),
        // `field("serverTime")` being `None` and it holding an unparseable
        // value are different facts — see `ServerClock` — so the two are kept
        // apart here rather than collapsed the way `.and_then` alone would.
        server_time: match field("serverTime") {
            None => ServerClock::Absent,
            Some(value) => match value.as_str().and_then(crate::clock::epoch_millis_from_iso) {
                Some(ms) => ServerClock::Present(ms),
                None => ServerClock::Malformed,
            },
        },
    }
}

/// Decide directly from an `/auth/me` response, against this machine's clock.
///
/// The convenience form callers actually use; [`decide`] and
/// [`entitlement_from_me`] stay public so the policy and the parsing can each be
/// tested without the other.
pub fn access_from_me(me: &serde_json::Value) -> Access {
    decide(&entitlement_from_me(me), crate::clock::now_millis())
}

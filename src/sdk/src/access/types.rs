//! Data types for the `access` module: the plan tiers, the facts `/auth/me`
//! reports, and the verdict [`decide`](super::decide) reaches from them.

#[allow(unused_imports)]
use super::*;

/// A billing tier, as the backend's `SubscriptionPlan` enum spells it.
///
/// [`Unknown`](Plan::Unknown) is a real state, not a parse failure to paper
/// over: the backend sends `null` when the billing lookup itself failed, and
/// that has to stay distinguishable from a confirmed `Free` so the refusal can
/// say which of the two happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// Registered, paying nothing.
    Free,
    /// The entry paid tier.
    Basic,
    /// The full paid tier.
    Pro,
    /// The backend could not say.
    Unknown,
}

impl Plan {
    /// Parse the wire spelling, case-insensitively.
    ///
    /// An unrecognized name is [`Plan::Unknown`] rather than [`Plan::Free`]:
    /// a name this binary cannot place is one it cannot bill against either
    /// way, and calling it `Free` would put a confident label on a value nobody
    /// confirmed.
    pub fn parse(name: &str) -> Plan {
        match name.trim().to_ascii_lowercase().as_str() {
            "free" => Plan::Free,
            "basic" => Plan::Basic,
            "pro" => Plan::Pro,
            _ => Plan::Unknown,
        }
    }

    /// Whether this tier is one the operator pays for.
    ///
    /// This is the "paid or basic plan" half of the access rule.
    pub fn is_paid(self) -> bool {
        matches!(self, Plan::Basic | Plan::Pro)
    }
}

/// The access facts an `/auth/me` response reports, already extracted.
///
/// Facts only — the verdict is [`decide`](super::decide)'s job. Keeping the two
/// apart is what lets the whole policy be tested without a backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entitlement {
    /// The account's billing tier.
    pub plan: Plan,
    /// Whether a live subscription backs that tier.
    pub has_active_subscription: bool,
    /// The admin-set `hasMedullaAccess` allowlist flag.
    pub admin_granted: bool,
    /// The account's email address, when `/auth/me` reported one.
    ///
    /// Carried here only so the staff-domain rule can read it — see
    /// [`Entitlement::is_staff`]. It is not otherwise part of the billing
    /// picture.
    pub email: Option<String>,
    /// When the account was created, in milliseconds since the Unix epoch.
    pub registered_at_ms: Option<i64>,
    /// The backend's own clock at the moment it answered.
    ///
    /// Used in place of this machine's clock wherever the backend sent one —
    /// see [`Entitlement::account_age_ms`].
    pub server_time: ServerClock,
}

/// The backend's clock, as extracted from `serverTime` on `/auth/me`.
///
/// A three-way read rather than `Option<i64>` because "the backend sent
/// nothing" and "the backend sent something this binary could not parse" are
/// not the same fact and must not resolve the same way: the first is a safe
/// invitation to fall back to the local clock, the second is a signal that
/// something is wrong with the field this binary asked for and falling back
/// would silently trade the backend's authority for a clock the operator
/// controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerClock {
    /// `/auth/me` had no `serverTime` field at all. Falling back to the local
    /// clock is the deployment's normal, documented behaviour, not a fault.
    Absent,
    /// `/auth/me` sent a `serverTime` that did not parse. Never fall back from
    /// this: the backend attempted to hand over its authoritative clock and
    /// failed, and a skewed local clock could turn that into a wrongly granted
    /// (or wrongly refused) trial.
    Malformed,
    /// The backend's clock, in milliseconds since the Unix epoch.
    Present(i64),
}

impl Entitlement {
    /// Whether this account's address sits on [`STAFF_DOMAIN`](super::STAFF_DOMAIN).
    ///
    /// Matches the domain itself and any subdomain of it, case-insensitively,
    /// and only ever on the part after the *last* `@` — so an address like
    /// `someone@evil.example` cannot smuggle the domain in via its local part,
    /// and `tinyhumans.ai.evil.example` does not match either.
    ///
    /// An account with no address reported is not staff; there is nothing to
    /// check against.
    pub fn is_staff(&self) -> bool {
        let Some(email) = self.email.as_deref() else {
            return false;
        };
        let Some((_, domain)) = email.trim().rsplit_once('@') else {
            return false;
        };
        let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
        domain == super::STAFF_DOMAIN || domain.ends_with(&format!(".{}", super::STAFF_DOMAIN))
    }

    /// How old the account is in milliseconds, measured against the backend's
    /// clock when it sent one and this machine's otherwise.
    ///
    /// Preferring the server's clock is the point of sending it: the age of an
    /// account is the difference between two instants the *backend* owns, and
    /// subtracting one of them from a local clock that is wrong by a month
    /// hands out either an extra month of trial or none at all. A `serverTime` the
    /// backend sent but this binary could not parse is never treated as if it
    /// were absent — see [`ServerClock`] — so it returns `None` here rather
    /// than quietly falling back to `local_now_ms`.
    ///
    /// A negative age — an account registered in this machine's future — is
    /// clamped to zero rather than refused, so a small forward skew reads as
    /// "brand new" instead of failing the whole check.
    pub fn account_age_ms(&self, local_now_ms: i64) -> Option<i64> {
        let now = match self.server_time {
            ServerClock::Present(ms) => ms,
            ServerClock::Absent => local_now_ms,
            ServerClock::Malformed => return None,
        };
        Some((now - self.registered_at_ms?).max(0))
    }
}

/// Why access was granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// The account is on a paid tier.
    Paid(Plan),
    /// Inside the free window every new account gets. Carries the whole
    /// milliseconds left of it, for the countdown the UI shows.
    Trial { remaining_ms: i64 },
    /// The admin-set allowlist flag is on.
    AdminGranted,
    /// The account signs in on [`STAFF_DOMAIN`](super::STAFF_DOMAIN) — our own
    /// people, who are not the audience the paywall is for.
    StaffDomain,
}

/// Why access was refused.
///
/// Two variants because they are not the same news. One is about the operator's
/// account; the other is about us failing to answer, and saying "your free
/// trial is over" to somebody whose trial is not over would be a lie the screen
/// tells on our behalf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    /// Registered, on no paid tier, and past the free window.
    ///
    /// The remedy is a Basic (or Pro) subscription; there is nothing else the
    /// operator can do from the terminal.
    TrialExpired,
    /// The backend did not report enough to decide — no entitlement block, an
    /// unresolvable plan, or no registration instant.
    Undetermined,
}

/// The verdict on whether this account may use Medulla.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// May use the product.
    Granted(Grant),
    /// May not, until they subscribe.
    Denied(Denial),
}

impl Access {
    /// Whether this verdict lets the operator through.
    pub fn is_granted(self) -> bool {
        matches!(self, Access::Granted(_))
    }
}

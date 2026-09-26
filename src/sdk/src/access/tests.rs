//! Unit tests for the access policy and its `/auth/me` extraction.

use serde_json::json;

use super::*;

/// A fixed "now" so no test depends on the wall clock: 2026-08-27T12:00:00Z.
const NOW_MS: i64 = 1_787_832_000_000;

const DAY_MS: i64 = 24 * 60 * 60 * 1_000;

/// A registered, unpaid account created `age_days` ago, with the backend's clock
/// reported as `NOW_MS`.
fn aged(age_days: i64) -> Entitlement {
    Entitlement {
        plan: Plan::Free,
        has_active_subscription: false,
        admin_granted: false,
        email: None,
        registered_at_ms: Some(NOW_MS - age_days * DAY_MS),
        server_time: ServerClock::Present(NOW_MS),
    }
}

// --- the rule ------------------------------------------------------------

/// The free window is 30 days, and the copy that names it reads it from here.
#[test]
fn the_free_window_is_thirty_days() {
    assert_eq!(TRIAL.as_millis() as i64, 30 * DAY_MS);
    assert_eq!(TRIAL_DAYS, 30);
}

#[test]
fn a_paid_plan_is_let_in() {
    for plan in [Plan::Basic, Plan::Pro] {
        let e = Entitlement {
            plan,
            // Old enough that the trial cannot be what admitted them.
            ..aged(400)
        };
        assert_eq!(decide(&e, NOW_MS), Access::Granted(Grant::Paid(plan)));
    }
}

#[test]
fn a_fresh_account_is_let_in_on_the_trial() {
    let e = aged(2);
    match decide(&e, NOW_MS) {
        Access::Granted(Grant::Trial { remaining_ms }) => {
            assert_eq!(remaining_ms, 28 * DAY_MS);
        }
        other => panic!("expected a trial grant, got {other:?}"),
    }
}

#[test]
fn an_unpaid_account_past_the_free_window_is_refused() {
    assert_eq!(
        decide(&aged(31), NOW_MS),
        Access::Denied(Denial::TrialExpired)
    );
}

/// The boundary is exclusive: at exactly 30 days the free window is spent.
#[test]
fn the_trial_ends_the_instant_the_free_window_is_up() {
    let a_moment_short = Entitlement {
        registered_at_ms: Some(NOW_MS - TRIAL.as_millis() as i64 + 1),
        ..aged(0)
    };
    assert!(decide(&a_moment_short, NOW_MS).is_granted());

    let exactly = Entitlement {
        registered_at_ms: Some(NOW_MS - TRIAL.as_millis() as i64),
        ..aged(0)
    };
    assert_eq!(
        decide(&exactly, NOW_MS),
        Access::Denied(Denial::TrialExpired)
    );
}

/// An address on our own domain is in, whatever the plan and however old the
/// account: we are not the audience for our own paywall.
#[test]
fn a_staff_address_outranks_an_expired_trial() {
    let e = Entitlement {
        email: Some("someone@tinyhumans.ai".to_string()),
        ..aged(400)
    };
    assert_eq!(decide(&e, NOW_MS), Access::Granted(Grant::StaffDomain));
}

#[test]
fn a_staff_address_is_matched_case_insensitively_and_on_subdomains() {
    for address in [
        "Someone@TinyHumans.ai",
        "someone@mail.tinyhumans.ai",
        " someone@tinyhumans.ai ",
        // A trailing root dot is the same domain.
        "someone@tinyhumans.ai.",
    ] {
        let e = Entitlement {
            email: Some(address.to_string()),
            ..aged(400)
        };
        assert_eq!(
            decide(&e, NOW_MS),
            Access::Granted(Grant::StaffDomain),
            "{address} should be staff"
        );
    }
}

/// Only the part after the last `@` counts, so a lookalike domain cannot borrow
/// the name.
#[test]
fn a_lookalike_address_is_not_staff() {
    for address in [
        "tinyhumans.ai@evil.example",
        "someone@tinyhumans.ai.evil.example",
        "someone@nottinyhumans.ai",
        "someone@tinyhumans.aim",
        "no-at-sign",
    ] {
        let e = Entitlement {
            email: Some(address.to_string()),
            ..aged(400)
        };
        assert_eq!(
            decide(&e, NOW_MS),
            Access::Denied(Denial::TrialExpired),
            "{address} should not be staff"
        );
    }
}

/// The staff rule reads an address, and an unresolvable plan still has none to
/// read — so it stays refused as undetermined rather than waved through.
#[test]
fn an_unknown_plan_without_an_address_is_still_undetermined() {
    let e = Entitlement {
        plan: Plan::Unknown,
        email: None,
        ..aged(1)
    };
    assert_eq!(decide(&e, NOW_MS), Access::Denied(Denial::Undetermined));
}

/// ...but a staff address is enough on its own, even when the billing lookup
/// failed: the backend still meters what they spend.
#[test]
fn a_staff_address_survives_an_unresolvable_plan() {
    let e = Entitlement {
        plan: Plan::Unknown,
        email: Some("someone@tinyhumans.ai".to_string()),
        ..aged(400)
    };
    assert_eq!(decide(&e, NOW_MS), Access::Granted(Grant::StaffDomain));
}

#[test]
fn the_admin_allowlist_outranks_an_expired_trial() {
    let e = Entitlement {
        admin_granted: true,
        ..aged(400)
    };
    assert_eq!(decide(&e, NOW_MS), Access::Granted(Grant::AdminGranted));
}

// --- the undetermined cases fail closed ----------------------------------

#[test]
fn an_unresolvable_plan_is_refused_as_unconfirmed_not_as_an_expired_trial() {
    // Old enough that an expired trial would also fit. The reason has to be the
    // one that is true, because it is what the screen tells the operator.
    let e = Entitlement {
        plan: Plan::Unknown,
        ..aged(400)
    };
    assert_eq!(decide(&e, NOW_MS), Access::Denied(Denial::Undetermined));
}

#[test]
fn an_account_with_no_known_registration_date_is_refused() {
    // Confirmed unpaid, but the free window can be neither granted nor declared
    // spent without knowing when it started.
    let e = Entitlement {
        registered_at_ms: None,
        ..aged(0)
    };
    assert_eq!(decide(&e, NOW_MS), Access::Denied(Denial::Undetermined));
}

#[test]
fn a_response_with_no_entitlement_block_is_refused() {
    // A gate that opens on its own faults is not a gate.
    let access = decide(&entitlement_from_me(&json!({ "id": "u1" })), NOW_MS);
    assert_eq!(access, Access::Denied(Denial::Undetermined));
}

// --- clock handling ------------------------------------------------------

#[test]
fn the_server_clock_beats_a_skewed_local_one() {
    let e = aged(2);
    // The local clock is a year fast; without the server's own instant this
    // account would look long expired.
    let skewed = NOW_MS + 365 * DAY_MS;
    assert!(decide(&e, skewed).is_granted());
}

#[test]
fn the_local_clock_is_used_when_the_server_sent_none() {
    let e = Entitlement {
        server_time: ServerClock::Absent,
        ..aged(2)
    };
    assert!(decide(&e, NOW_MS).is_granted());
    assert_eq!(
        decide(&e, NOW_MS + 30 * DAY_MS),
        Access::Denied(Denial::TrialExpired)
    );
}

#[test]
fn a_malformed_server_clock_is_never_treated_as_absent() {
    // The backend sent *something* for serverTime, but this binary could not
    // read it. Falling back to the local clock here — the way an absent field
    // legitimately does — would let a skewed operator clock decide an age the
    // backend explicitly tried and failed to hand over.
    let e = Entitlement {
        server_time: ServerClock::Malformed,
        ..aged(2)
    };
    assert_eq!(e.account_age_ms(NOW_MS), None);
    assert_eq!(decide(&e, NOW_MS), Access::Denied(Denial::Undetermined));
    // Even a skewed local clock that would otherwise expire the trial must not
    // change the verdict: Malformed refuses before local_now_ms is consulted.
    assert_eq!(
        decide(&e, NOW_MS + 365 * DAY_MS),
        Access::Denied(Denial::Undetermined)
    );
}

#[test]
fn an_unparseable_server_time_extracts_as_malformed_not_absent() {
    let me = json!({
        "entitlement": {
            "plan": "FREE",
            "registeredAt": "2026-08-01T00:00:00.000Z",
            "serverTime": "not-a-timestamp",
        }
    });
    assert_eq!(entitlement_from_me(&me).server_time, ServerClock::Malformed);
}

#[test]
fn an_account_registered_in_the_future_reads_as_brand_new() {
    let e = Entitlement {
        registered_at_ms: Some(NOW_MS + DAY_MS),
        server_time: ServerClock::Absent,
        ..aged(0)
    };
    assert_eq!(e.account_age_ms(NOW_MS), Some(0));
    assert!(decide(&e, NOW_MS).is_granted());
}

// --- plan parsing --------------------------------------------------------

#[test]
fn plan_names_parse_case_insensitively() {
    assert_eq!(Plan::parse("FREE"), Plan::Free);
    assert_eq!(Plan::parse("basic"), Plan::Basic);
    assert_eq!(Plan::parse(" Pro "), Plan::Pro);
}

#[test]
fn an_unrecognized_plan_is_unknown_not_free() {
    // A tier this binary is too old to know about is more likely a new paid one.
    assert_eq!(Plan::parse("ENTERPRISE"), Plan::Unknown);
    assert!(!Plan::Unknown.is_paid());
    assert!(!Plan::Free.is_paid());
}

// --- extraction ----------------------------------------------------------

#[test]
fn entitlement_is_read_off_the_response() {
    let me = json!({
        "id": "u1",
        "entitlement": {
            "plan": "BASIC",
            "hasActiveSubscription": true,
            "adminGranted": false,
            "registeredAt": "2026-08-01T00:00:00.000Z",
            "serverTime": "2026-08-27T12:00:00.000Z",
        }
    });

    let e = entitlement_from_me(&me);

    assert_eq!(e.plan, Plan::Basic);
    assert!(e.has_active_subscription);
    assert!(!e.admin_granted);
    assert_eq!(e.server_time, ServerClock::Present(NOW_MS));
    assert_eq!(
        e.registered_at_ms,
        Some(NOW_MS - 26 * DAY_MS - 12 * 3_600_000)
    );
}

#[test]
fn the_nested_user_shape_is_accepted() {
    let me = json!({ "user": { "entitlement": { "plan": "PRO" } } });
    assert_eq!(entitlement_from_me(&me).plan, Plan::Pro);
}

#[test]
fn a_null_plan_reads_as_unknown() {
    let me = json!({ "entitlement": { "plan": null } });
    assert_eq!(entitlement_from_me(&me).plan, Plan::Unknown);
}

/// Only the `entitlement` block is read for billing facts (the address is the
/// one exception — see `the_address_is_read_from_the_user_object`). Top-level
/// `createdAt` and `hasMedullaAccess` are the pre-gate response shape and carry
/// no weight.
#[test]
fn facts_outside_the_entitlement_block_are_ignored() {
    let me = json!({
        "id": "u1",
        "hasMedullaAccess": true,
        "createdAt": "2026-08-27T00:00:00.000Z",
    });

    let e = entitlement_from_me(&me);

    assert!(!e.admin_granted);
    assert_eq!(e.registered_at_ms, None);
    assert_eq!(decide(&e, NOW_MS), Access::Denied(Denial::Undetermined));
}

/// The address sits beside the id, not inside the entitlement block, and is
/// read through both response shapes.
#[test]
fn the_address_is_read_from_the_user_object() {
    let flat = json!({ "email": "someone@tinyhumans.ai", "entitlement": { "plan": "FREE" } });
    let e = entitlement_from_me(&flat);
    assert_eq!(e.email.as_deref(), Some("someone@tinyhumans.ai"));
    assert_eq!(decide(&e, NOW_MS), Access::Granted(Grant::StaffDomain));

    let nested = json!({ "user": { "email": "someone@tinyhumans.ai" } });
    assert_eq!(
        entitlement_from_me(&nested).email.as_deref(),
        Some("someone@tinyhumans.ai")
    );

    assert_eq!(entitlement_from_me(&json!({ "id": "u1" })).email, None);
}

#[test]
fn an_unparseable_registration_instant_is_none_not_zero() {
    // Zero would be 1970 — an expired trial, which is a different (and wrong)
    // reason from the unconfirmed one this actually is.
    let me = json!({ "entitlement": { "plan": "FREE", "registeredAt": "not a date" } });
    let e = entitlement_from_me(&me);
    assert_eq!(e.registered_at_ms, None);
    assert_eq!(decide(&e, NOW_MS), Access::Denied(Denial::Undetermined));
}

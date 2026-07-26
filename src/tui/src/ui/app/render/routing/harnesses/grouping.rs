//! Group flat harness declarations into provider accounts and connected hosts.

use std::collections::BTreeMap;

use medulla::runtime::{CapacitySnapshot, HarnessBudget};

use super::types::AccountGroup;

/// Group one harness kind by provider, account family, and opaque account ID.
///
/// Every harness appears below each account it reports. Multiple usage windows
/// for one account remain in one group, and the same account reported by two
/// hosts produces two host rows rather than two account headings.
pub(super) fn account_groups<'a>(
    capacity: &'a CapacitySnapshot,
    kind: &str,
) -> Vec<AccountGroup<'a>> {
    type Key = (String, String, Option<String>);

    let mut groups: BTreeMap<Key, AccountGroup<'a>> = BTreeMap::new();
    for harness in capacity
        .harnesses
        .iter()
        .filter(|harness| harness.kind == kind)
    {
        let mut account_budgets: BTreeMap<Key, Vec<&HarnessBudget>> = BTreeMap::new();
        for budget in &harness.budgets {
            let key = budget_key(budget);
            account_budgets.entry(key).or_default().push(budget);
        }

        // A connected harness with no usage report is still an account/host
        // relationship worth showing. Its provider is the best identity the
        // declaration has; the account remains explicitly unidentified.
        if account_budgets.is_empty() {
            let provider = harness
                .providers
                .first()
                .cloned()
                .unwrap_or_else(|| harness.kind.clone());
            account_budgets.insert((provider, "account".into(), None), Vec::new());
        }

        for (key, budgets) in account_budgets {
            let group = groups.entry(key.clone()).or_insert_with(|| AccountGroup {
                provider: key.0,
                account_type: key.1,
                account_id: key.2,
                harnesses: Vec::new(),
                budgets: Vec::new(),
            });
            if !group
                .harnesses
                .iter()
                .any(|existing| existing.id == harness.id)
            {
                group.harnesses.push(harness);
            }
            group.budgets.extend(budgets);
        }
    }
    groups.into_values().collect()
}

/// Stable grouping key for one usage report.
fn budget_key(budget: &HarnessBudget) -> (String, String, Option<String>) {
    (
        non_blank(&budget.provider).unwrap_or("unknown").to_string(),
        budget
            .account_type
            .as_deref()
            .and_then(non_blank)
            .unwrap_or("account")
            .to_string(),
        budget
            .seat
            .as_deref()
            .and_then(non_blank)
            .map(str::to_string),
    )
}

/// Treat blank provider data as absent rather than as a distinct account.
fn non_blank(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

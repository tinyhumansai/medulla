//! Private row-model types for the Harnesses hierarchy.

use medulla::runtime::{HarnessBudget, HarnessDescriptor};

/// One provider account consumed by one harness kind on one or more hosts.
pub(super) struct AccountGroup<'a> {
    /// Provider that owns the account.
    pub(super) provider: String,
    /// Authentication/account family, such as subscription or API key.
    pub(super) account_type: String,
    /// Opaque non-secret provider account identifier.
    pub(super) account_id: Option<String>,
    /// Distinct installed harness instances consuming the account.
    pub(super) harnesses: Vec<&'a HarnessDescriptor>,
    /// Usage reports for this account across those instances.
    pub(super) budgets: Vec<&'a HarnessBudget>,
}

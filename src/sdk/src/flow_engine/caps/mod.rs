//! Assembling the capability bundle the engine runs against.
//!
//! There are exactly two entry points out of this module —
//! [`build_capabilities`] and [`open_checkpointer`] — and everything downstream
//! goes through them. Keeping the surface that narrow is what makes a re-vendor
//! of the engine a one-file problem: when its `Capabilities` struct gains a
//! field, this is the only place that has to learn about it.
//!
//! Each capability lives in its own submodule beside this one, named for the
//! engine trait it satisfies.

pub mod agent;
pub mod code;
pub mod dispatch;
pub mod http;
pub mod mocks;
pub mod script;
pub mod state;
pub mod tools;

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tinyflows::caps::{Capabilities, WorkflowResolver};
use tinyflows::engine::{Checkpointer, FileCheckpointer};

use crate::flow_engine::settings::CapabilitySettings;

use self::agent::{HarnessAgentRunner, HarnessLlm};
use self::code::{DeniedCodeRunner, ProcessCodeRunner};
use self::dispatch::HarnessDispatch;
use self::http::{AllowlistHttpClient, HttpCredential};
use self::state::FileStateStore;
use self::tools::MedullaToolInvoker;

/// Everything a run needs from the host, other than its settings.
///
/// Grouped into one struct because [`build_capabilities`] would otherwise take
/// six positional arguments of which three are `Arc<dyn …>`, and a caller would
/// eventually pass them in the wrong order.
pub struct HostServices {
    /// Where `agent` nodes send their work.
    pub dispatch: Arc<dyn HarnessDispatch>,
    /// How `sub_workflow` nodes find their child graph.
    pub resolver: Arc<dyn WorkflowResolver>,
    /// HTTP credentials, keyed by the name a `connection_ref` uses.
    pub http_credentials: HashMap<String, HttpCredential>,
}

/// Build the capability bundle for one run.
///
/// `state_namespace` scopes the [`StateStore`](tinyflows::caps::StateStore) —
/// conventionally `workflow:<id>`, so two workflows never collide on a key.
/// `run_id` tags every dispatched task, and is the id an abort matches.
pub fn build_capabilities(
    settings: Arc<CapabilitySettings>,
    services: HostServices,
    state_namespace: &str,
    run_id: &str,
) -> Capabilities {
    let code: Arc<dyn tinyflows::caps::CodeRunner> = if settings.allow_code {
        // The same bound `medulla:shell` uses, so an author who moves a
        // script between the two does not silently change its deadline.
        Arc::new(ProcessCodeRunner::new(settings.script_timeout()))
    } else {
        Arc::new(DeniedCodeRunner)
    };

    Capabilities {
        llm: Arc::new(HarnessLlm::new(
            services.dispatch.clone(),
            settings.clone(),
            run_id,
        )),
        agent: Some(Arc::new(HarnessAgentRunner::new(
            services.dispatch,
            settings.clone(),
            run_id,
        ))),
        tools: Arc::new(MedullaToolInvoker::new(settings.clone())),
        http: Arc::new(AllowlistHttpClient::new(
            settings.clone(),
            services.http_credentials,
        )),
        code,
        state: Arc::new(FileStateStore::new(&settings.state_dir, state_namespace)),
        resolver: services.resolver,
    }
}

/// Build a capability bundle that touches nothing outside the process.
///
/// Used for validating a graph by simulation: expressions resolve against real
/// values, node schemas are satisfied, and no harness session is started. The
/// tool invoker is the engine's mock behind [`tools::PreflightToolInvoker`], so
/// an argument that failed to resolve is still caught.
pub fn build_dry_run_capabilities(resolver: Arc<dyn WorkflowResolver>) -> Capabilities {
    let mut caps = tinyflows::caps::mock::mock_capabilities();
    caps.llm = Arc::new(mocks::SchemaAwareMockLlm);
    caps.agent = Some(Arc::new(mocks::SchemaAwareMockAgentRunner));
    caps.tools = Arc::new(tools::PreflightToolInvoker::new(caps.tools.clone()));
    caps.resolver = resolver;
    caps
}

/// Open the run checkpointer.
///
/// A checkpoint is what makes an approval pause outlive the process: a run that
/// parked yesterday is resumed by handing the engine the same `thread_id`. The
/// engine's own file-backed implementation is reused rather than written afresh
/// — a bespoke one would only be a second thing to get wrong.
pub fn open_checkpointer(settings: &CapabilitySettings) -> Arc<dyn Checkpointer<Value>> {
    Arc::new(FileCheckpointer::<Value>::new(&settings.checkpoint_dir))
}

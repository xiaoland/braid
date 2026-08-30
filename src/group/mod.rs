//! Agent Group: thin forwarder from the Event Queue to an `AgentSession`.

pub(crate) mod issue_agent;
pub(crate) mod pr_agent;
pub(crate) mod provider;
pub mod session_manager;

pub(crate) use issue_agent::issue_agent_worker;
pub(crate) use pr_agent::pr_agent_worker;
pub use session_manager::SessionManager;

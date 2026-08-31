//! Agent Group: workers own provider connection epochs and physical session
//! lifecycle; `dispatch` is the execution half that claims queue decisions and
//! runs them against `AgentSession`s.

pub(crate) mod dispatch;
pub(crate) mod issue_agent;
pub(crate) mod pr_agent;
pub(crate) mod provider;
pub mod session_manager;

pub(crate) use issue_agent::issue_agent_worker;
pub(crate) use pr_agent::pr_agent_worker;
pub use session_manager::SessionManager;

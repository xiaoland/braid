//! Agent Group: logical lifecycle and materialization. Shared workers execute
//! queue decisions through neutral session handles; adapters own physical resources.

pub(crate) mod dispatch;
pub(crate) mod issue_agent;
pub(crate) mod pr_agent;
pub(crate) mod provider;
mod session_manager;

mod worker;
use session_manager::SessionManager;
pub(crate) use worker::{GroupKind, GroupSpec, agent_group_worker};

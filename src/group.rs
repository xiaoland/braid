//! Agent Group: thin forwarder from the Event Queue to an `AgentSession`.
//!
//! The implementation currently lives in `crate::runtime`; this module provides
//! the symmetric top-level boundary so callers use `crate::group::*`.

pub(crate) mod issue_agent {
    pub(crate) use crate::runtime::issue_agent::*;
}
pub(crate) mod pr_agent {
    pub(crate) use crate::runtime::pr_agent::*;
}
pub mod session_manager {
    pub use crate::runtime::session_manager::*;
}

pub(crate) use issue_agent::issue_agent_worker;
pub(crate) use pr_agent::pr_agent_worker;
pub use session_manager::SessionManager;

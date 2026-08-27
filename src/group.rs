//! Agent Group: thin forwarder from the Event Queue to an `AgentSession`.
//!
//! In this release the group role is still implemented by `crate::runtime::issue_agent`
//! and `crate::runtime::pr_agent`; this module is the boundary placeholder.
//!
//! TODO: move per-group session lifecycle and `AgentSession` forwarding into
//! this module.

pub use crate::runtime::session_manager::SessionManager;

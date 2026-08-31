//! Event Queue: per-work-item per-agent-group quiet window, batch emission,
//! and claim decisions. The queue never touches provider sessions or
//! connections.

pub(crate) mod scheduler;

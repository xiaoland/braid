//! Event Queue: per-work-item per-agent-group quiet window and batch emission.

pub(crate) mod outbox;
pub(crate) mod scheduler;

pub(crate) use outbox::drain_one_write;

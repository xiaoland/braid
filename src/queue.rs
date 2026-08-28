#![allow(unused_imports)]
//! Event Queue: per-work-item per-agent-group quiet window and batch emission.
//!
//! The implementation currently lives in `crate::runtime`; this module provides
//! the symmetric top-level boundary so callers use `crate::queue::*`.

pub(crate) mod outbox {
    pub(crate) use crate::runtime::outbox::*;
}
pub(crate) mod scheduler {
    pub(crate) use crate::runtime::scheduler::*;
}

pub(crate) use outbox::drain_one_write;

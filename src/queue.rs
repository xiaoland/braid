//! Event Queue: per-work-item per-agent-group quiet window and batch emission.
//!
//! In this release the queue role is still implemented by `crate::runtime::scheduler`
//! and the `scheduler_batches` tables in `crate::store`; this module is the
//! boundary placeholder.
//!
//! TODO: move quiet-window / threshold / single-flight claim logic into this
//! module.

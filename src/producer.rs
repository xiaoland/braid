//! Event Producer: webhook/GraphQL ingress → canonical diff → classified events.
//!
//! In this release the producer role is still implemented by `crate::webhook`
//! and `crate::runtime::reconcile`; this module is the boundary placeholder.
//!
//! TODO: move classification and explicit routing into this module so the
//! producer owns the Event Queue writes.

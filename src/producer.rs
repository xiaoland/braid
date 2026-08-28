//! Event Producer: webhook/GraphQL ingress -> canonical diff -> classified events.
//!
//! The implementation currently lives in `crate::runtime`; this module provides
//! the symmetric top-level boundary so callers use `crate::producer::*`.

pub(crate) mod ingress {
    pub(crate) use crate::runtime::ingress::*;
}
pub(crate) mod reconcile {
    pub(crate) use crate::runtime::reconcile::*;
}

pub(crate) use ingress::{event_worker, webhook_handler};
pub(crate) use reconcile::{lease_worker, reconciliation_worker};

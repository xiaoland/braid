//! Event Producer: webhook/GraphQL ingress -> canonical diff -> classified events.

pub(crate) mod ingress;
pub(crate) mod reconcile;

pub(crate) use ingress::{IngressState, event_worker, webhook_handler};
pub(crate) use reconcile::{lease_worker, reconciliation_worker};

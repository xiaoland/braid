//! Event Producer: webhook/GraphQL ingress -> canonical diff -> classified events.

pub(crate) mod ingress;
pub(crate) mod reconcile;

pub(crate) use ingress::{IngressState, webhook_handler};
pub(crate) use reconcile::LEASE_TTL_SECONDS;
pub(crate) use reconcile::{lease_worker, reconciliation_worker};

mod mentions;
pub(crate) use mentions::mention_worker;

//! Queue decisions and periodic quiet-window advancement, independent of network I/O.
pub(crate) mod scheduler;

use crate::store::StoreActor;
use std::sync::Arc;
use tokio::{
    sync::watch,
    time::{Duration, MissedTickBehavior},
};

pub(crate) async fn queue_worker(store: Arc<StoreActor>, mut shutdown: watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = tick.tick() => {
                if let Err(error) = store.advance_scheduler() {
                    tracing::error!(%error, "cannot advance scheduler");
                }
            }
        }
    }
}

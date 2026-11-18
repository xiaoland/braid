//! Wrangler and cloudflared share a newly created, Braid-owned Unix group.
use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use std::io;
use tokio::{
    process::Child,
    time::{Duration, timeout},
};

pub(super) struct OwnedGroup {
    id: Pid,
    armed: bool,
}
impl OwnedGroup {
    pub(super) fn new(child: &Child) -> Self {
        let id = i32::try_from(child.id().expect("newly spawned child has a PID"))
            .expect("Unix PID fits pid_t");
        assert!(id > 1, "a spawned tunnel cannot own a system process group");
        Self { id: Pid::from_raw(id), armed: true }
    }

    pub(super) async fn close(&mut self, child: &mut Child) -> io::Result<()> {
        self.signal(Some(Signal::SIGTERM))?;
        if let Ok(result) = timeout(Duration::from_secs(2), self.wait(child)).await {
            result?;
        } else {
            self.signal(Some(Signal::SIGKILL))?;
            timeout(Duration::from_secs(2), self.wait(child)).await.map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "owned Tunnel process group did not stop")
            })??;
        }
        self.armed = false;
        Ok(())
    }

    fn signal(&self, signal: Option<Signal>) -> io::Result<bool> {
        match killpg(self.id, signal) {
            Ok(()) => Ok(true),
            Err(Errno::ESRCH) => Ok(false),
            Err(error) => Err(io::Error::from_raw_os_error(error as i32)),
        }
    }

    async fn wait(&self, child: &mut Child) -> io::Result<()> {
        loop {
            let wrapper_gone = child.try_wait()?.is_some();
            if !self.signal(None)? && wrapper_gone {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
impl Drop for OwnedGroup {
    fn drop(&mut self) {
        if self.armed
            && let Err(error) = self.signal(Some(Signal::SIGKILL))
        {
            tracing::warn!(%error,"cannot signal owned Tunnel group during cancellation cleanup");
        }
    }
}

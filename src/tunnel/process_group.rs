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
        if let Err(error) = self.signal_and_wait(child, Signal::SIGTERM).await {
            if error.kind() != io::ErrorKind::TimedOut
                && error.raw_os_error() != Some(Errno::EPERM as i32)
            {
                return Err(error);
            }
            self.signal_and_wait(child, Signal::SIGKILL).await?;
        }
        self.armed = false;
        Ok(())
    }

    async fn signal_and_wait(&self, child: &mut Child, signal: Signal) -> io::Result<()> {
        let denied = match self.signal(Some(signal)) {
            Ok(_) => None,
            // Darwin also returns EPERM for a group containing only zombies.
            // Reap and wait for ESRCH; EPERM itself never proves termination.
            Err(error) if error.raw_os_error() == Some(Errno::EPERM as i32) => Some(error),
            Err(error) => return Err(error),
        };
        match timeout(Duration::from_secs(2), self.wait(child)).await {
            Ok(result) => result,
            Err(_) => Err(denied.unwrap_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "owned Tunnel process group did not stop")
            })),
        }
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
            let group_gone = match self.signal(None) {
                Ok(exists) => !exists,
                Err(error) if error.raw_os_error() == Some(Errno::EPERM as i32) => false,
                Err(error) => return Err(error),
            };
            if group_gone && wrapper_gone {
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

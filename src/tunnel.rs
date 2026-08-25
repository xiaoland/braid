use std::{path::Path, process::Stdio};

use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    process::{Child, Command},
    sync::mpsc,
    task::JoinHandle,
    time::{Duration, timeout},
};

#[derive(Debug, Error)]
pub enum TunnelError {
    #[error("cannot start Wrangler Quick Tunnel: {0}")]
    Start(std::io::Error),
    #[error(
        "Wrangler Quick Tunnel did not publish and register a trycloudflare.com URL within 45 seconds"
    )]
    MissingUrl,
    #[error("cannot stop Wrangler Quick Tunnel: {0}")]
    Stop(std::io::Error),
}

pub struct QuickTunnel {
    child: Child,
    drains: Vec<JoinHandle<()>>,
    pub url: String,
}

impl QuickTunnel {
    pub async fn start(wrangler: &Path, local_url: &str) -> Result<Self, TunnelError> {
        let mut child = Command::new(wrangler)
            .args(["tunnel", "quick-start", local_url, "--log-level", "info"])
            .env("TUNNEL_TRANSPORT_PROTOCOL", "http2")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(TunnelError::Start)?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let drains = vec![drain(stdout, sender.clone()), drain(stderr, sender)];
        let url = timeout(Duration::from_secs(45), async {
            let mut public_url = None;
            let mut registered = false;
            loop {
                tokio::select! {
                    line = receiver.recv() => {
                        let line = line?;
                        if public_url.is_none() {
                            public_url = find_tunnel_url(&line);
                        }
                        if line.contains("Registered tunnel connection") {
                            registered = true;
                        }
                        if registered && public_url.is_some() {
                            return public_url;
                        }
                    }
                    status = child.wait() => {
                        return status.ok().and_then(|status| {
                            tracing::error!(%status, "Wrangler Quick Tunnel exited during startup");
                            None
                        });
                    }
                }
            }
        })
        .await
        .ok()
        .flatten()
        .ok_or(TunnelError::MissingUrl)?;
        Ok(Self { child, drains, url })
    }

    pub fn has_exited(&mut self) -> Result<bool, TunnelError> {
        self.child.try_wait().map(|status| status.is_some()).map_err(TunnelError::Stop)
    }

    pub async fn stop(mut self) -> Result<(), TunnelError> {
        if self.child.try_wait().map_err(TunnelError::Stop)?.is_none() {
            self.child.kill().await.map_err(TunnelError::Stop)?;
        }
        for drain in self.drains.drain(..) {
            drain.abort();
        }
        Ok(())
    }
}

fn drain<R>(reader: R, sender: mpsc::UnboundedSender<String>) -> JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::info!(target: "braid::tunnel", output = %line, "Wrangler Quick Tunnel");
            let _ = sender.send(line);
        }
    })
}

fn find_tunnel_url(line: &str) -> Option<String> {
    line.split(|character: char| {
        character.is_whitespace() || matches!(character, '|' | '"' | '\'' | '`')
    })
    .map(|value| {
        value.trim_matches(|character: char| {
            !character.is_ascii_alphanumeric() && !matches!(character, ':' | '/' | '.' | '-')
        })
    })
    .find(|value| value.starts_with("https://") && value.ends_with(".trycloudflare.com"))
    .map(str::to_owned)
}

#![allow(clippy::wildcard_imports)]
use super::*;

struct ProcessGuard {
    child: StdMutex<Option<Child>>,
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if let Some(child) =
            self.child.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_mut()
        {
            let _ = child.start_kill();
        }
    }
}

#[derive(Clone)]
pub struct CodexProvider {
    writer: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<PendingRequests>>,
    notifications: broadcast::Sender<ProviderNotification>,
    next_request_id: Arc<AtomicI64>,
    _process: Arc<ProcessGuard>,
}

impl CodexProvider {
    pub async fn connect(config: &CodexConfig) -> Result<Self, ProviderError> {
        let mut child = Command::new(&config.executable)
            .args(["app-server", "--stdio"])
            .env("CODEX_HOME", &config.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let writer = child
            .stdin
            .take()
            .ok_or_else(|| ProviderError::Protocol("app-server stdin was not piped".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::Protocol("app-server stdout was not piped".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ProviderError::Protocol("app-server stderr was not piped".into()))?;
        let pending = Arc::new(Mutex::new(BTreeMap::new()));
        let (notifications, _) = broadcast::channel(512);
        spawn_codex_stdout(stdout, Arc::clone(&pending), notifications.clone());
        spawn_codex_stderr(stderr);
        let client = Self {
            writer: Arc::new(Mutex::new(writer)),
            pending,
            notifications,
            next_request_id: Arc::new(AtomicI64::new(1)),
            _process: Arc::new(ProcessGuard { child: StdMutex::new(Some(child)) }),
        };
        client
            .request(
                "initialize",
                json!({
                    "clientInfo":{"name":"braid","version":env!("CARGO_PKG_VERSION")},
                    "capabilities":{"experimentalApi":true}
                }),
            )
            .await?;
        client.notify("initialized", None).await?;
        Ok(client)
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, ProviderError> {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        if let Err(error) = self.write(&json!({"id":id,"method":method,"params":params})).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        match timeout(REQUEST_TIMEOUT, receiver).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => Err(ProviderError::Disconnected),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(ProviderError::Timeout { method: method.into() })
            }
        }
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), ProviderError> {
        let frame = params.map_or_else(
            || json!({"method":method}),
            |params| json!({"method":method,"params":params}),
        );
        self.write(&frame).await
    }

    async fn write(&self, frame: &Value) -> Result<(), ProviderError> {
        let mut bytes = serde_json::to_vec(frame)
            .map_err(|error| ProviderError::Protocol(error.to_string()))?;
        bytes.push(b'\n');
        let mut writer = self.writer.lock().await;
        writer.write_all(&bytes).await.map_err(|_| ProviderError::Disconnected)?;
        writer.flush().await.map_err(|_| ProviderError::Disconnected)
    }
}

#[async_trait::async_trait]
impl AgentProvider for CodexProvider {
    fn subscribe(&self) -> broadcast::Receiver<ProviderNotification> {
        self.notifications.subscribe()
    }

    async fn start_session(
        &self,
        profile: &Profile,
        developer_instructions: &str,
    ) -> Result<ProviderSession, ProviderError> {
        let result = self
            .request(
                "thread/start",
                json!({
                    "cwd":path_text(&profile.workspace)?,
                    "model":profile.model,
                    "developerInstructions":developer_instructions,
                    "approvalPolicy":"never",
                    "sandbox":"danger-full-access",
                    "ephemeral":false,
                    "serviceName":"braid"
                }),
            )
            .await?;
        let thread = result
            .get("thread")
            .ok_or_else(|| ProviderError::Protocol("thread/start omitted result.thread".into()))?;
        Ok(ProviderSession {
            thread_id: required_string(thread, "id", "thread/start result.thread")?,
        })
    }

    async fn inject_context(&self, thread_id: &str, context: &str) -> Result<(), ProviderError> {
        self.request(
            "thread/inject_items",
            json!({
                "threadId":thread_id,
                "items":[{
                    "type":"message",
                    "role":"user",
                    "content":[{"type":"input_text","text":context}]
                }]
            }),
        )
        .await?;
        Ok(())
    }

    async fn resume_session(
        &self,
        thread_id: &str,
        profile: &Profile,
        developer_instructions: &str,
    ) -> Result<ProviderSession, ProviderError> {
        let result = self
            .request(
                "thread/resume",
                json!({
                    "threadId":thread_id,
                    "cwd":path_text(&profile.workspace)?,
                    "model":profile.model,
                    "developerInstructions":developer_instructions,
                    "approvalPolicy":"never",
                    "sandbox":"danger-full-access"
                }),
            )
            .await?;
        let thread = result
            .get("thread")
            .ok_or_else(|| ProviderError::Protocol("thread/resume omitted result.thread".into()))?;
        let resumed_id = required_string(thread, "id", "thread/resume result.thread")?;
        if resumed_id != thread_id {
            return Err(ProviderError::Protocol(format!(
                "thread/resume returned {resumed_id} for requested thread {thread_id}"
            )));
        }
        Ok(ProviderSession { thread_id: resumed_id })
    }

    async fn start_turn(
        &self,
        thread_id: &str,
        profile: &Profile,
        event_references: &str,
    ) -> Result<ProviderTurn, ProviderError> {
        let result = self
            .request(
                "turn/start",
                json!({
                    "threadId":thread_id,
                    "input":[{"type":"text","text":event_references}],
                    "model":profile.model,
                    "effort":profile.reasoning
                }),
            )
            .await?;
        let turn = result
            .get("turn")
            .ok_or_else(|| ProviderError::Protocol("turn/start omitted result.turn".into()))?;
        Ok(ProviderTurn { turn_id: required_string(turn, "id", "turn/start result.turn")? })
    }

    async fn steer(
        &self,
        thread_id: &str,
        expected_turn_id: &str,
        event_references: &str,
    ) -> Result<(), ProviderError> {
        self.request(
            "turn/steer",
            json!({
                "threadId":thread_id,
                "expectedTurnId":expected_turn_id,
                "input":[{"type":"text","text":event_references}]
            }),
        )
        .await?;
        Ok(())
    }

    async fn interrupt(&self, thread_id: &str, turn_id: &str) -> Result<(), ProviderError> {
        self.request("turn/interrupt", json!({"threadId":thread_id,"turnId":turn_id})).await?;
        Ok(())
    }
}

fn spawn_codex_stdout(
    stdout: tokio::process::ChildStdout,
    pending: Arc<Mutex<PendingRequests>>,
    notifications: broadcast::Sender<ProviderNotification>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            telemetry::emit_payload_event(&PayloadEvidence {
                github_body: "",
                github_summary: "",
                credential: "",
                provider_transcript: &line,
                webhook_payload: "",
                local_path: "",
            });
            let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                tracing::warn!("Codex app-server emitted non-JSON stdout");
                continue;
            };
            if let Some(id) = frame.get("id").and_then(Value::as_i64) {
                if let Some(sender) = pending.lock().await.remove(&id) {
                    let response = if let Some(error) = frame.get("error") {
                        Err(ProviderError::Protocol(
                            error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown app-server error")
                                .to_owned(),
                        ))
                    } else {
                        Ok(frame.get("result").cloned().unwrap_or(Value::Null))
                    };
                    let _ = sender.send(response);
                }
                continue;
            }
            if let Some(notification) = parse_codex_notification(&frame) {
                let _ = notifications.send(notification);
            }
        }
        let drained = std::mem::take(&mut *pending.lock().await);
        for sender in drained.into_values() {
            let _ = sender.send(Err(ProviderError::Disconnected));
        }
        let _ = notifications.send(ProviderNotification::Disconnected);
    });
}

fn spawn_codex_stderr(stderr: tokio::process::ChildStderr) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            telemetry::emit_payload_event(&PayloadEvidence {
                github_body: "",
                github_summary: "",
                credential: "",
                provider_transcript: &line,
                webhook_payload: "",
                local_path: "",
            });
            tracing::debug!(provider = "codex", output = %line, "provider diagnostic");
        }
    });
}

fn parse_codex_notification(frame: &Value) -> Option<ProviderNotification> {
    let method = frame.get("method")?.as_str()?.to_owned();
    let params = frame.get("params").unwrap_or(&Value::Null);
    let thread_id = params.get("threadId").and_then(Value::as_str).map(str::to_owned);
    let turn = params.get("turn");
    let turn_id = turn
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .or_else(|| params.get("turnId").and_then(Value::as_str))
        .map(str::to_owned);
    match method.as_str() {
        "turn/started" => {
            Some(ProviderNotification::TurnStarted { thread_id: thread_id?, turn_id: turn_id? })
        }
        "turn/completed" => Some(ProviderNotification::TurnCompleted {
            thread_id: thread_id?,
            turn_id: turn_id?,
            status: turn?.get("status")?.as_str()?.to_owned(),
            error: turn
                .and_then(|turn| turn.get("error"))
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        _ => Some(ProviderNotification::Activity { method, thread_id, turn_id }),
    }
}

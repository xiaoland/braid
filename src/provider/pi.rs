#![allow(clippy::wildcard_imports)]
use super::*;
use std::path::PathBuf;

struct PiRequest {
    response: oneshot::Sender<Result<Value, ProviderError>>,
    turn_id: Option<String>,
}

type PiPendingRequests = BTreeMap<i64, PiRequest>;

struct PiProcess {
    writer: ChildStdin,
    child: Child,
    #[allow(dead_code)]
    stdout_handle: tokio::task::JoinHandle<()>,
    #[allow(dead_code)]
    stderr_handle: tokio::task::JoinHandle<()>,
}

struct PiState {
    config: PiConfig,
    pending: Arc<Mutex<PiPendingRequests>>,
    next_request_id: Arc<AtomicI64>,
    process: Option<PiProcess>,
    session: Option<String>,
    developer_instructions: String,
    context: String,
    native_session_id: Option<String>,
    close_result: Option<Result<(), String>>,
}

#[derive(Clone)]
pub struct PiProvider {
    state: Arc<Mutex<PiState>>,
    notifications: broadcast::Sender<ProviderNotification>,
    closed: watch::Sender<bool>,
    thread_id: Arc<Mutex<String>>,
    turn_id: Arc<Mutex<String>>,
}

impl PiProvider {
    pub fn connect(config: &PiConfig) -> Self {
        let (notifications, _) = broadcast::channel(512);
        let (closed, _) = watch::channel(false);
        let state = PiState {
            config: config.clone(),
            pending: Arc::new(Mutex::new(BTreeMap::new())),
            next_request_id: Arc::new(AtomicI64::new(1)),
            process: None,
            session: None,
            developer_instructions: String::new(),
            context: String::new(),
            native_session_id: None,
            close_result: None,
        };
        Self {
            state: Arc::new(Mutex::new(state)),
            notifications,
            closed,
            thread_id: Arc::new(Mutex::new(String::new())),
            turn_id: Arc::new(Mutex::new(String::new())),
        }
    }

    fn spawn(
        &self,
        state: &mut PiState,
        workspace: &Path,
        session: Option<&str>,
    ) -> Result<(), ProviderError> {
        let mut cmd = Command::new(&state.config.executable);
        cmd.args(["--mode", "rpc"]);
        let session_dir = workspace.join(".braid").join("pi-sessions");
        std::fs::create_dir_all(&session_dir).map_err(|error| {
            ProviderError::Protocol(format!(
                "cannot create pi session dir {}: {error}",
                session_dir.display()
            ))
        })?;
        cmd.args(["--session-dir", &session_dir.to_string_lossy()]);
        if let Some(provider) = &state.config.provider {
            cmd.args(["--provider", provider]);
        }
        if let Some(model) = &state.config.model {
            cmd.args(["--model", model]);
        }
        if let Some(thinking) = &state.config.thinking {
            cmd.args(["--thinking", thinking]);
        }
        if let Some(home) = &state.config.home {
            cmd.env("PI_CODING_AGENT_DIR", home);
        }
        if let Ok(api_key) = state.config.api_key()
            && let Some(api_key_env) = &state.config.api_key_environment
        {
            cmd.env(api_key_env, api_key);
        }
        // Native shell commands must reach the same Braid binary as this runtime.
        if let Ok(current_exe) = std::env::current_exe()
            && let Some(exe_dir) = current_exe.parent()
        {
            let mut path_parts: Vec<PathBuf> =
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
            if !path_parts.iter().any(|p| p == exe_dir) {
                path_parts.insert(0, exe_dir.to_path_buf());
            }
            cmd.env(
                "PATH",
                std::env::join_paths(path_parts).map_err(|error| {
                    ProviderError::Protocol(format!("cannot build native Pi PATH: {error}"))
                })?,
            );
        }
        if let Some(braid_config) = &state.config.braid_config_path {
            let braid_config = if braid_config.is_absolute() {
                braid_config.clone()
            } else {
                std::env::current_dir()?.join(braid_config)
            };
            cmd.env("BRAID_CONFIG", braid_config).env("BRAID_AGENT", "1");
        } else if let Ok(braid_config) = std::env::var("BRAID_CONFIG") {
            cmd.env("BRAID_CONFIG", braid_config);
        }
        if let Some(session_path) = session {
            cmd.args(["--session", session_path]);
        }
        if !state.developer_instructions.is_empty() {
            cmd.args(["--append-system-prompt", &state.developer_instructions]);
        }
        cmd.current_dir(workspace);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);

        let mut child = cmd.spawn()?;
        let writer = child
            .stdin
            .take()
            .ok_or_else(|| ProviderError::Protocol("pi stdin was not piped".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::Protocol("pi stdout was not piped".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ProviderError::Protocol("pi stderr was not piped".into()))?;
        let stdout_handle = spawn_pi_stdout(
            stdout,
            Arc::clone(&state.pending),
            self.notifications.clone(),
            self.closed.clone(),
            Arc::clone(&self.thread_id),
            Arc::clone(&self.turn_id),
        );
        let stderr_handle =
            spawn_pi_stderr(stderr, Arc::clone(&self.thread_id), Arc::clone(&self.turn_id));
        state.process = Some(PiProcess { writer, child, stdout_handle, stderr_handle });
        Ok(())
    }

    async fn request(state: &mut PiState, frame: Value) -> Result<Value, ProviderError> {
        Self::request_with_turn(state, frame, None).await
    }

    async fn request_with_turn(
        state: &mut PiState,
        frame: Value,
        turn_id: Option<String>,
    ) -> Result<Value, ProviderError> {
        let id = state.next_request_id.fetch_add(1, Ordering::Relaxed);
        let method = frame.get("type").and_then(Value::as_str).unwrap_or("unknown").to_owned();
        let mut frame = frame;
        if let Some(obj) = frame.as_object_mut() {
            obj.insert("id".to_string(), json!(id));
        }

        let (sender, receiver) = oneshot::channel();
        if let (Some(thread), Some(turn)) = (state.session.as_deref(), turn_id.as_deref()) {
            telemetry::bind_provider_turn(thread, turn);
        }
        telemetry::emit_provider_payload(
            "pi",
            state.session.as_deref().unwrap_or(""),
            turn_id.as_deref(),
            "request",
            &frame.to_string(),
        );
        state.pending.lock().await.insert(id, PiRequest { response: sender, turn_id });

        let process = state.process.as_mut().ok_or(ProviderError::Disconnected)?;
        let bytes = {
            let mut bytes = serde_json::to_vec(&frame)
                .map_err(|error| ProviderError::Protocol(error.to_string()))?;
            bytes.push(b'\n');
            bytes
        };
        if let Err(error) = process.writer.write_all(&bytes).await {
            state.pending.lock().await.remove(&id);
            return Err(ProviderError::Unknown(format!(
                "Pi {method} request {id} write failed: {error}"
            )));
        }
        if let Err(error) = process.writer.flush().await {
            state.pending.lock().await.remove(&id);
            return Err(ProviderError::Unknown(format!(
                "Pi {method} request {id} flush failed: {error}"
            )));
        }

        match timeout(REQUEST_TIMEOUT, receiver).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => Err(ProviderError::Disconnected),
            Err(_) => {
                state.pending.lock().await.remove(&id);
                Err(ProviderError::Timeout { method: format!("Pi {method} request {id}") })
            }
        }
    }

    fn success_or_protocol(result: &Value, context: &str) -> Result<(), ProviderError> {
        if result.get("success").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(());
        }
        let message = if let Some(error) = result.get("error").and_then(Value::as_str) {
            error.to_owned()
        } else if let Some(message) =
            result.get("data").and_then(|data| data.get("message")).and_then(Value::as_str)
        {
            message.to_owned()
        } else {
            format!("{context} failed")
        };
        Err(ProviderError::Protocol(message))
    }
}

#[async_trait::async_trait]
impl AgentProvider for PiProvider {
    fn subscribe(&self) -> broadcast::Receiver<ProviderNotification> {
        self.notifications.subscribe()
    }

    async fn closed(&self) {
        let mut closed = self.closed.subscribe();
        while !*closed.borrow() && closed.changed().await.is_ok() {}
    }

    async fn start_session(
        &self,
        profile: &Profile,
        developer_instructions: &str,
    ) -> Result<ProviderSession, ProviderError> {
        let mut state = self.state.lock().await;
        developer_instructions.clone_into(&mut state.developer_instructions);
        state.context.clear();
        state.session = None;
        state.close_result = None;
        state.process = None;

        self.spawn(&mut state, profile.workspace(), None)?;

        let result =
            Self::request(&mut state, json!({"type": "new_session", "name": "braid-session"}))
                .await?;
        Self::success_or_protocol(&result, "new_session")?;

        let state_resp = Self::request(&mut state, json!({"type": "get_state"})).await?;
        let session_file = state_resp
            .get("data")
            .and_then(|data| data.get("sessionFile"))
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Protocol("get_state missing sessionFile".into()))?;
        state.session = Some(session_file.to_owned());
        state.native_session_id =
            Some(required_string(&state_resp["data"], "sessionId", "Pi get_state")?);
        *self.thread_id.lock().await = session_file.to_owned();

        Ok(ProviderSession { thread_id: session_file.to_owned() })
    }

    async fn inject_context(&self, thread_id: &str, context: &str) -> Result<(), ProviderError> {
        let mut state = self.state.lock().await;
        if state.session.as_deref() != Some(thread_id) {
            return Err(ProviderError::Protocol("Context session identity mismatch".into()));
        }
        super::pi_context::save(
            thread_id,
            &super::pi_context::PendingContext {
                native_session_id: state.native_session_id.clone().ok_or_else(|| {
                    ProviderError::Protocol("Pi omitted native session identity".into())
                })?,
                context: context.to_owned(),
            },
        )?;
        context.clone_into(&mut state.context);
        Ok(())
    }

    async fn resume_session(
        &self,
        thread_id: &str,
        profile: &Profile,
        developer_instructions: &str,
    ) -> Result<ProviderSession, ProviderError> {
        let mut pending = super::pi_context::load(thread_id)?;
        let has_history = Path::new(thread_id).is_file();
        if has_history {
            super::pi_context::validate_history(thread_id)?;
        }
        if !has_history && pending.is_none() {
            return Err(ProviderError::Protocol(
                "Pi native history and startup Context are both missing".into(),
            ));
        }
        let mut state = self.state.lock().await;
        developer_instructions.clone_into(&mut state.developer_instructions);
        state.context.clear();
        state.session = Some(thread_id.to_owned());
        *self.thread_id.lock().await = thread_id.to_owned();
        state.close_result = None;
        state.process = None;
        self.spawn(&mut state, profile.workspace(), Some(thread_id))?;
        let native = Self::request(&mut state, json!({"type":"get_state"})).await?;
        let resumed_path = required_string(&native["data"], "sessionFile", "Pi get_state")?;
        if Path::new(&resumed_path) != Path::new(thread_id) {
            return Err(ProviderError::Protocol("Pi resumed a different session path".into()));
        }
        let native_id = required_string(&native["data"], "sessionId", "Pi get_state")?;
        if let Some(initial) = pending.as_mut() {
            if has_history && initial.native_session_id != native_id {
                return Err(ProviderError::Protocol(
                    "Pi startup Context belongs to a different native session".into(),
                ));
            }
            let history = Self::request(&mut state, json!({"type":"get_entries"})).await?;
            if !super::pi_context::consumed(initial, &history)? {
                initial.context.clone_into(&mut state.context);
            }
            if !has_history {
                // No prior user history exists: Pi recreated only the empty
                // native header behind the same Braid execution identity.
                native_id.clone_into(&mut initial.native_session_id);
                super::pi_context::save(thread_id, initial)?;
            }
        }
        state.native_session_id = Some(native_id);
        *self.thread_id.lock().await = thread_id.to_owned();
        Ok(ProviderSession { thread_id: thread_id.to_owned() })
    }

    async fn start_turn(
        &self,
        _thread_id: &str,
        profile: &Profile,
        event_references: &str,
    ) -> Result<ProviderTurn, ProviderError> {
        let mut state = self.state.lock().await;
        if state.process.is_none() {
            let session = state.session.clone();
            self.spawn(&mut state, profile.workspace(), session.as_deref())?;
        }

        let native = Self::request(&mut state, json!({"type":"get_state"})).await?;
        if native_is_busy(&native)? {
            return Err(ProviderError::Deferred(
                "Pi is streaming, compacting or has queued input".into(),
            ));
        }
        let mut message = String::new();
        if !state.context.is_empty() {
            message.push_str(&state.context);
            message.push_str("\n\n");
        }
        message.push_str(event_references);
        let turn_id = uuid::Uuid::now_v7().to_string();
        let result = Self::request_with_turn(
            &mut state,
            json!({"type":"prompt","message":message}),
            Some(turn_id.clone()),
        )
        .await;
        match result {
            Ok(result) => Self::success_or_protocol(&result, "prompt")?,
            Err(ProviderError::Protocol(reason)) => {
                let native = Self::request(&mut state, json!({"type":"get_state"})).await?;
                if native_is_busy(&native)? {
                    return Err(ProviderError::Deferred(reason));
                }
                return Err(ProviderError::Protocol(reason));
            }
            Err(error) => return Err(error),
        }
        // ACK clears only this process's staging buffer. The immutable startup
        // artifact survives until native history can establish consumption.
        state.context.clear();
        Ok(ProviderTurn { turn_id })
    }

    async fn steer(
        &self,
        _thread_id: &str,
        expected_turn_id: &str,
        event_references: &str,
    ) -> Result<(), ProviderError> {
        let mut state = self.state.lock().await;
        if self.turn_id.lock().await.as_str() != expected_turn_id {
            return Err(ProviderError::Deferred("Pi active turn changed before steering".into()));
        }
        let native = Self::request(&mut state, json!({"type":"get_state"})).await?;
        if !required_native_bool(&native, "isStreaming")? {
            return Err(ProviderError::Deferred("Pi is no longer streaming".into()));
        }
        let result =
            Self::request(&mut state, json!({"type":"steer","message":event_references})).await?;
        Self::success_or_protocol(&result, "steer")?;
        if self.turn_id.lock().await.as_str() != expected_turn_id {
            return Err(ProviderError::Unknown(
                "Pi accepted steering at a changing turn boundary".into(),
            ));
        }
        Ok(())
    }

    async fn interrupt(&self, _thread_id: &str, turn_id: &str) -> Result<(), ProviderError> {
        let mut state = self.state.lock().await;
        if self.turn_id.lock().await.as_str() != turn_id {
            return Ok(());
        }
        let result = Self::request(&mut state, json!({"type":"abort"})).await?;
        Self::success_or_protocol(&result, "abort")
    }

    async fn close_session(
        &self,
        _thread_id: &str,
        confirmed_idle: bool,
    ) -> Result<(), ProviderError> {
        let mut state = self.state.lock().await;
        if let Some(result) = &state.close_result {
            return result.clone().map_err(ProviderError::Protocol);
        }
        let result = async {
            if !confirmed_idle {
                let cleared = Self::request(&mut state, json!({"type":"clear_queue"})).await?;
                Self::success_or_protocol(&cleared, "clear_queue")?;
                let aborted = Self::request(&mut state, json!({"type":"abort"})).await?;
                Self::success_or_protocol(&aborted, "abort")?;
                let native = Self::request(&mut state, json!({"type":"get_state"})).await?;
                if native_is_busy(&native)? {
                    return Err(ProviderError::Protocol(
                        "Pi remains busy after abort; replacement is blocked".into(),
                    ));
                }
            }
            if let Some(process) = state.process.as_mut() {
                process.writer.shutdown().await?;
                timeout(REQUEST_TIMEOUT, process.child.wait()).await.map_err(|_| {
                    ProviderError::Timeout { method: "Pi child exit after stop".into() }
                })??;
            }
            Ok(())
        }
        .await;
        if result.is_err()
            && let Some(process) = state.process.as_mut()
        {
            // Reclaim the parent, but never promote a forced kill into a
            // claim that detached tools have stopped.
            let _ = process.child.start_kill();
            let _ = timeout(REQUEST_TIMEOUT, process.child.wait()).await;
        }
        state.close_result = Some(result.as_ref().copied().map_err(ToString::to_string));
        if result.is_ok() {
            state.process = None;
        }
        result
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_pi_stdout(
    stdout: tokio::process::ChildStdout,
    pending: Arc<Mutex<PiPendingRequests>>,
    notifications: broadcast::Sender<ProviderNotification>,
    closed: watch::Sender<bool>,
    thread_id: Arc<Mutex<String>>,
    turn_id: Arc<Mutex<String>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        let mut last_stop = None;
        let mut last_error = None;
        while let Ok(Some(line)) = lines.next_line().await {
            let current_thread = thread_id.lock().await.clone();
            let current_turn = turn_id.lock().await.clone();
            let parsed = serde_json::from_str::<Value>(&line);
            let response_turn = if let Ok(frame) = &parsed {
                if let Some(id) = frame.get("id").and_then(Value::as_i64) {
                    pending.lock().await.get(&id).and_then(|request| request.turn_id.clone())
                } else {
                    None
                }
            } else {
                None
            };
            let evidence_turn = response_turn
                .as_deref()
                .or_else(|| (!current_turn.is_empty()).then_some(current_turn.as_str()));
            telemetry::emit_provider_payload("pi", &current_thread, evidence_turn, "stdout", &line);
            let Ok(frame) = parsed else {
                tracing::warn!("pi emitted non-JSON stdout");
                continue;
            };
            if let Some(id) = frame.get("id").and_then(Value::as_i64) {
                if let Some(request) = pending.lock().await.remove(&id) {
                    let response = if frame.get("success").and_then(Value::as_bool) == Some(false) {
                        let message = frame
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("pi rpc returned success=false")
                            .to_owned();
                        Err(ProviderError::Protocol(message))
                    } else {
                        Ok(frame)
                    };
                    let response = match (response, request.turn_id) {
                        (Ok(frame), Some(candidate)) => {
                            if frame["data"]["disposition"]
                                .as_str()
                                .is_some_and(|value| value != "started")
                            {
                                Err(ProviderError::Unknown(format!(
                                    "Pi prompt accepted without immediate execution: {}",
                                    frame["data"]
                                )))
                            } else {
                                *turn_id.lock().await = candidate;
                                Ok(frame)
                            }
                        }
                        (response, _) => response,
                    };
                    let _ = request.response.send(response);
                }
                continue;
            }
            if let Some(notification) = parse_pi_event(
                &frame,
                &current_thread,
                &current_turn,
                &mut last_stop,
                &mut last_error,
            ) {
                let settled = matches!(notification, ProviderNotification::TurnCompleted { .. });
                let _ = notifications.send(notification);
                if settled {
                    turn_id.lock().await.clear();
                }
            }
        }
        let drained = std::mem::take(&mut *pending.lock().await);
        for request in drained.into_values() {
            let _ = request.response.send(Err(ProviderError::Disconnected));
        }
        let _ = notifications.send(ProviderNotification::Disconnected);
        closed.send_replace(true);
    })
}

fn spawn_pi_stderr(
    stderr: tokio::process::ChildStderr,
    thread_id: Arc<Mutex<String>>,
    turn_id: Arc<Mutex<String>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let thread = thread_id.lock().await.clone();
            let turn = turn_id.lock().await.clone();
            telemetry::emit_provider_payload(
                "pi",
                &thread,
                (!turn.is_empty()).then_some(turn.as_str()),
                "stderr",
                &line,
            );
            tracing::debug!(provider = "pi", output = %line, "provider diagnostic");
        }
    })
}

fn parse_pi_event(
    frame: &Value,
    thread_id: &str,
    turn_id: &str,
    last_stop: &mut Option<String>,
    last_error: &mut Option<String>,
) -> Option<ProviderNotification> {
    let event_type = frame.get("type")?.as_str()?;
    if event_type == "agent_start" {
        *last_stop = None;
        *last_error = None;
    }
    if event_type == "message_end" {
        let message = &frame["message"];
        if message["role"] == "assistant" {
            *last_stop = message["stopReason"].as_str().map(str::to_owned);
            *last_error = message["errorMessage"].as_str().map(str::to_owned);
        }
    }
    match event_type {
        "turn_start" => Some(ProviderNotification::TurnStarted {
            thread_id: thread_id.to_owned(),
            turn_id: turn_id.to_owned(),
        }),
        // Pi may persist, compact, or retry after message_end. Only settled
        // closes this physical run; a transient error must not abort recovery.
        "agent_settled" => Some(ProviderNotification::TurnCompleted {
            thread_id: thread_id.to_owned(),
            turn_id: turn_id.to_owned(),
            status: match last_stop.as_deref() {
                Some("stop") => "completed",
                Some("aborted") => "interrupted",
                _ => "failed",
            }
            .into(),
            error: if matches!(last_stop.as_deref(), Some("stop" | "aborted")) {
                None
            } else {
                Some(last_error.clone().unwrap_or_else(|| format!("Pi settled with {last_stop:?}")))
            },
        }),
        _ => None,
    }
}

fn required_native_bool(response: &Value, field: &str) -> Result<bool, ProviderError> {
    response["data"][field]
        .as_bool()
        .ok_or_else(|| ProviderError::Protocol(format!("Pi get_state missing boolean {field}")))
}

fn native_is_busy(response: &Value) -> Result<bool, ProviderError> {
    let streaming = required_native_bool(response, "isStreaming")?;
    let compacting = required_native_bool(response, "isCompacting")?;
    let pending = response["data"]["pendingMessageCount"].as_u64().ok_or_else(|| {
        ProviderError::Protocol("Pi get_state missing pendingMessageCount".into())
    })?;
    Ok(streaming || compacting || pending != 0)
}

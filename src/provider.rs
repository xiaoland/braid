use std::{
    collections::BTreeMap,
    path::Path,
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicI64, Ordering},
    },
};

use serde_json::{Value, json};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{Mutex, broadcast, oneshot},
    time::{Duration, timeout},
};

use crate::{
    config::{CodexConfig, PiConfig, Profile},
    telemetry::{self, PayloadEvidence},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
type PendingRequests = BTreeMap<i64, oneshot::Sender<Result<Value, ProviderError>>>;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("cannot start provider process: {0}")]
    Start(#[from] std::io::Error),
    #[error("provider request {method} timed out")]
    Timeout { method: String },
    #[error("provider disconnected")]
    Disconnected,
    #[error("provider protocol error: {0}")]
    Protocol(String),
}

#[derive(Debug, Clone)]
pub enum ProviderNotification {
    TurnStarted { thread_id: String, turn_id: String },
    TurnCompleted { thread_id: String, turn_id: String, status: String, error: Option<String> },
    Activity { method: String, thread_id: Option<String>, turn_id: Option<String> },
    Disconnected,
}

#[derive(Debug, Clone)]
pub struct ProviderSession {
    pub thread_id: String,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct ProviderTurn {
    pub turn_id: String,
}

/// Trait abstracting over Codex and Pi providers.
#[async_trait::async_trait]
pub trait AgentProvider: Send + Sync {
    fn subscribe(&self) -> broadcast::Receiver<ProviderNotification>;

    async fn start_session(
        &self,
        profile: &Profile,
        developer_instructions: &str,
    ) -> Result<ProviderSession, ProviderError>;

    async fn inject_context(&self, thread_id: &str, context: &str) -> Result<(), ProviderError>;

    async fn resume_session(
        &self,
        thread_id: &str,
        profile: &Profile,
        developer_instructions: &str,
    ) -> Result<ProviderSession, ProviderError>;

    async fn start_turn(
        &self,
        thread_id: &str,
        profile: &Profile,
        event_references: &str,
    ) -> Result<ProviderTurn, ProviderError>;

    async fn steer(
        &self,
        thread_id: &str,
        expected_turn_id: &str,
        event_references: &str,
    ) -> Result<(), ProviderError>;

    async fn interrupt(&self, thread_id: &str, turn_id: &str) -> Result<(), ProviderError>;
}

// ---------------------------------------------------------------------------
// CodexProvider
// ---------------------------------------------------------------------------

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
            model: result
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("provider-default")
                .to_owned(),
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
        Ok(ProviderSession {
            thread_id: resumed_id,
            model: result
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("provider-default")
                .to_owned(),
        })
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

// ---------------------------------------------------------------------------
// PiProvider
// ---------------------------------------------------------------------------

struct PiProcess {
    writer: ChildStdin,
    _child: Child,
    #[allow(dead_code)]
    stdout_handle: tokio::task::JoinHandle<()>,
    #[allow(dead_code)]
    stderr_handle: tokio::task::JoinHandle<()>,
}

struct PiState {
    config: PiConfig,
    pending: Arc<Mutex<PendingRequests>>,
    next_request_id: Arc<AtomicI64>,
    process: Option<PiProcess>,
    session: Option<String>,
    developer_instructions: String,
    context: String,
    current_turn_id: Option<String>,
}

#[derive(Clone)]
pub struct PiProvider {
    state: Arc<Mutex<PiState>>,
    notifications: broadcast::Sender<ProviderNotification>,
    thread_id: Arc<Mutex<String>>,
    turn_id: Arc<Mutex<String>>,
}

impl PiProvider {
    pub fn connect(config: &PiConfig) -> Self {
        let (notifications, _) = broadcast::channel(512);
        let state = PiState {
            config: config.clone(),
            pending: Arc::new(Mutex::new(BTreeMap::new())),
            next_request_id: Arc::new(AtomicI64::new(1)),
            process: None,
            session: None,
            developer_instructions: String::new(),
            context: String::new(),
            current_turn_id: None,
        };
        Self {
            state: Arc::new(Mutex::new(state)),
            notifications,
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
        if let Some(api_key_env) = &state.config.api_key_environment
            && let Ok(api_key) = std::env::var(api_key_env)
        {
            cmd.env(api_key_env, api_key);
        }
        // Ensure the spawned Pi process can find the same `braid` binary and config
        // so it can execute `braid gh` and `gh` shell commands.
        if let Ok(current_exe) = std::env::current_exe() {
            if let Some(exe_dir) = current_exe.parent() {
                let mut path_parts: Vec<String> =
                    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect();
                let exe_dir_str = exe_dir.to_string_lossy().into_owned();
                if !path_parts.iter().any(|p| p == &exe_dir_str) {
                    path_parts.insert(0, exe_dir_str);
                }
                cmd.env("PATH", std::env::join_paths(path_parts).unwrap_or_default());
            }
        }
        if let Ok(braid_config) = std::env::var("BRAID_CONFIG") {
            cmd.env("BRAID_CONFIG", braid_config);
        }
        if let Some(session_path) = session {
            cmd.args(["--session", session_path]);
        }
        if !state.developer_instructions.is_empty() {
            cmd.args(["--system-prompt", &state.developer_instructions]);
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
            Arc::clone(&self.thread_id),
            Arc::clone(&self.turn_id),
        );
        let stderr_handle = spawn_pi_stderr(stderr);
        state.process = Some(PiProcess { writer, _child: child, stdout_handle, stderr_handle });
        Ok(())
    }

    async fn request(state: &mut PiState, frame: Value) -> Result<Value, ProviderError> {
        let id = state.next_request_id.fetch_add(1, Ordering::Relaxed);
        let mut frame = frame;
        if let Some(obj) = frame.as_object_mut() {
            obj.insert("id".to_string(), json!(id));
        }

        let (sender, receiver) = oneshot::channel();
        state.pending.lock().await.insert(id, sender);

        let process = state.process.as_mut().ok_or(ProviderError::Disconnected)?;
        let bytes = {
            let mut bytes = serde_json::to_vec(&frame)
                .map_err(|error| ProviderError::Protocol(error.to_string()))?;
            bytes.push(b'\n');
            bytes
        };
        if let Err(error) = process.writer.write_all(&bytes).await {
            state.pending.lock().await.remove(&id);
            return Err(ProviderError::Start(error));
        }
        if let Err(error) = process.writer.flush().await {
            state.pending.lock().await.remove(&id);
            return Err(ProviderError::Start(error));
        }

        match timeout(REQUEST_TIMEOUT, receiver).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => Err(ProviderError::Disconnected),
            Err(_) => {
                state.pending.lock().await.remove(&id);
                Err(ProviderError::Timeout { method: "pi_rpc".into() })
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

    async fn start_session(
        &self,
        profile: &Profile,
        developer_instructions: &str,
    ) -> Result<ProviderSession, ProviderError> {
        let mut state = self.state.lock().await;
        developer_instructions.clone_into(&mut state.developer_instructions);
        state.context.clear();
        state.session = None;
        state.current_turn_id = None;
        state.process = None;

        self.spawn(&mut state, &profile.workspace, None)?;

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

        Ok(ProviderSession {
            thread_id: session_file.to_owned(),
            model: profile.model.clone().unwrap_or_else(|| "pi-default".into()),
        })
    }

    async fn inject_context(&self, _thread_id: &str, context: &str) -> Result<(), ProviderError> {
        let mut state = self.state.lock().await;
        context.clone_into(&mut state.context);
        Ok(())
    }

    async fn resume_session(
        &self,
        thread_id: &str,
        profile: &Profile,
        developer_instructions: &str,
    ) -> Result<ProviderSession, ProviderError> {
        let mut state = self.state.lock().await;
        developer_instructions.clone_into(&mut state.developer_instructions);
        state.context.clear();
        state.session = Some(thread_id.to_owned());
        state.current_turn_id = None;
        state.process = None;

        self.spawn(&mut state, &profile.workspace, Some(thread_id))?;

        Ok(ProviderSession {
            thread_id: thread_id.to_owned(),
            model: profile.model.clone().unwrap_or_else(|| "pi-default".into()),
        })
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
            self.spawn(&mut state, &profile.workspace, session.as_deref())?;
        }

        let mut message = String::new();
        if !state.developer_instructions.is_empty() {
            message.push_str(&state.developer_instructions);
            message.push_str("\n\n");
        }
        if !state.context.is_empty() {
            message.push_str(&state.context);
            message.push_str("\n\n");
        }
        message.push_str(event_references);

        let turn_id = uuid::Uuid::now_v7().to_string();
        state.current_turn_id = Some(turn_id.clone());
        *self.turn_id.lock().await = turn_id.clone();
        *self.thread_id.lock().await = state.session.clone().unwrap_or_default();

        let result =
            Self::request(&mut state, json!({"type": "prompt", "message": message})).await?;
        Self::success_or_protocol(&result, "prompt")?;

        Ok(ProviderTurn { turn_id })
    }

    async fn steer(
        &self,
        _thread_id: &str,
        _expected_turn_id: &str,
        event_references: &str,
    ) -> Result<(), ProviderError> {
        let mut state = self.state.lock().await;
        let result =
            Self::request(&mut state, json!({"type": "steer", "message": event_references}))
                .await?;
        Self::success_or_protocol(&result, "steer")?;
        Ok(())
    }

    async fn interrupt(&self, _thread_id: &str, _turn_id: &str) -> Result<(), ProviderError> {
        let mut state = self.state.lock().await;
        let result = Self::request(&mut state, json!({"type": "abort"})).await?;
        Self::success_or_protocol(&result, "abort")?;
        Ok(())
    }
}

fn spawn_pi_stdout(
    stdout: tokio::process::ChildStdout,
    pending: Arc<Mutex<PendingRequests>>,
    notifications: broadcast::Sender<ProviderNotification>,
    thread_id: Arc<Mutex<String>>,
    turn_id: Arc<Mutex<String>>,
) -> tokio::task::JoinHandle<()> {
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
                tracing::warn!("pi emitted non-JSON stdout");
                continue;
            };
            if let Some(id) = frame.get("id").and_then(Value::as_i64) {
                if let Some(sender) = pending.lock().await.remove(&id) {
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
                    let _ = sender.send(response);
                }
                continue;
            }
            let current_thread = thread_id.lock().await.clone();
            let current_turn = turn_id.lock().await.clone();
            if let Some(notification) = parse_pi_event(&frame, &current_thread, &current_turn) {
                let _ = notifications.send(notification);
            }
        }
        let drained = std::mem::take(&mut *pending.lock().await);
        for sender in drained.into_values() {
            let _ = sender.send(Err(ProviderError::Disconnected));
        }
        let _ = notifications.send(ProviderNotification::Disconnected);
    })
}

fn spawn_pi_stderr(stderr: tokio::process::ChildStderr) -> tokio::task::JoinHandle<()> {
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
            tracing::debug!(provider = "pi", output = %line, "provider diagnostic");
        }
    })
}

fn parse_pi_event(frame: &Value, thread_id: &str, turn_id: &str) -> Option<ProviderNotification> {
    let event_type = frame.get("type")?.as_str()?;
    match event_type {
        "turn_start" => Some(ProviderNotification::TurnStarted {
            thread_id: thread_id.to_owned(),
            turn_id: turn_id.to_owned(),
        }),
        "turn_end" | "agent_settled" => {
            let has_error = if event_type == "turn_end" {
                frame
                    .get("message")
                    .and_then(|message| message.get("stopReason"))
                    .and_then(Value::as_str)
                    .is_some_and(|reason| reason == "error" || reason == "aborted")
            } else {
                false
            };
            let error =
                if has_error { Some("pi turn ended with error or abort".into()) } else { None };
            Some(ProviderNotification::TurnCompleted {
                thread_id: thread_id.to_owned(),
                turn_id: turn_id.to_owned(),
                status: if has_error { "failed".into() } else { "completed".into() },
                error,
            })
        }
        "message_end" => {
            // Fallback for Pi sessions that emit the final assistant message without a
            // following turn_end (for example, when the model stops cleanly but the
            // session remains open for follow-ups). Only treat a stopReason of "stop"
            // as terminal; toolUse means more tool calls are expected.
            let message = frame.get("message")?;
            if message.get("role").and_then(Value::as_str) != Some("assistant") {
                return Some(ProviderNotification::Activity {
                    method: event_type.into(),
                    thread_id: None,
                    turn_id: None,
                });
            }
            let stop_reason = message.get("stopReason").and_then(Value::as_str)?;
            if stop_reason == "stop" {
                Some(ProviderNotification::TurnCompleted {
                    thread_id: thread_id.to_owned(),
                    turn_id: turn_id.to_owned(),
                    status: "completed".into(),
                    error: None,
                })
            } else if stop_reason == "error" || stop_reason == "aborted" {
                Some(ProviderNotification::TurnCompleted {
                    thread_id: thread_id.to_owned(),
                    turn_id: turn_id.to_owned(),
                    status: "failed".into(),
                    error: Some(format!("pi assistant message ended with {stop_reason}")),
                })
            } else {
                Some(ProviderNotification::Activity {
                    method: event_type.into(),
                    thread_id: None,
                    turn_id: None,
                })
            }
        }
        _ => Some(ProviderNotification::Activity {
            method: event_type.into(),
            thread_id: None,
            turn_id: None,
        }),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn required_string(value: &Value, field: &str, context: &str) -> Result<String, ProviderError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ProviderError::Protocol(format!("{context} omitted {field}")))
}

fn path_text(path: &Path) -> Result<&str, ProviderError> {
    path.to_str().ok_or_else(|| {
        ProviderError::Protocol(format!("provider workspace is not UTF-8: {}", path.display()))
    })
}

#[async_trait::async_trait]
impl AgentProvider for Box<dyn AgentProvider> {
    fn subscribe(&self) -> broadcast::Receiver<ProviderNotification> {
        self.as_ref().subscribe()
    }

    async fn start_session(
        &self,
        profile: &Profile,
        developer_instructions: &str,
    ) -> Result<ProviderSession, ProviderError> {
        self.as_ref().start_session(profile, developer_instructions).await
    }

    async fn inject_context(&self, thread_id: &str, context: &str) -> Result<(), ProviderError> {
        self.as_ref().inject_context(thread_id, context).await
    }

    async fn resume_session(
        &self,
        thread_id: &str,
        profile: &Profile,
        developer_instructions: &str,
    ) -> Result<ProviderSession, ProviderError> {
        self.as_ref().resume_session(thread_id, profile, developer_instructions).await
    }

    async fn start_turn(
        &self,
        thread_id: &str,
        profile: &Profile,
        event_references: &str,
    ) -> Result<ProviderTurn, ProviderError> {
        self.as_ref().start_turn(thread_id, profile, event_references).await
    }

    async fn steer(
        &self,
        thread_id: &str,
        expected_turn_id: &str,
        event_references: &str,
    ) -> Result<(), ProviderError> {
        self.as_ref().steer(thread_id, expected_turn_id, event_references).await
    }

    async fn interrupt(&self, thread_id: &str, turn_id: &str) -> Result<(), ProviderError> {
        self.as_ref().interrupt(thread_id, turn_id).await
    }
}

pub async fn connect_provider(
    config: &crate::config::ProviderConfig,
) -> Result<Box<dyn crate::provider::AgentProvider>, crate::provider::ProviderError> {
    if let Some(codex) = &config.codex {
        let provider = CodexProvider::connect(codex).await?;
        Ok(Box::new(provider))
    } else if let Some(pi) = &config.pi {
        let provider = PiProvider::connect(pi);
        Ok(Box::new(provider))
    } else {
        Err(crate::provider::ProviderError::Protocol("no provider configured".into()))
    }
}

use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, backup::Backup, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::{Duration as TimeDuration, OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

pub const DATABASE_SCHEMA_VERSION: u32 = 6;

const INITIAL_SQL: &str = include_str!("../migrations/0001_initial.sql");
const CONTEXT_LEDGER_SQL: &str = include_str!("../migrations/0002_context_ledger.sql");
const TRANSPORT_RUNTIME_SQL: &str = include_str!("../migrations/0003_transport_runtime.sql");
const PROVIDER_RUNTIME_SQL: &str = include_str!("../migrations/0004_provider_runtime.sql");
const OPERATIONAL_STATUS_SQL: &str = include_str!("../migrations/0005_operational_status.sql");
const CONTEXT_RESETS_SQL: &str = include_str!("../migrations/0006_context_resets.sql");
const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, name: "initial", sql: INITIAL_SQL },
    Migration { version: 2, name: "context-ledger", sql: CONTEXT_LEDGER_SQL },
    Migration { version: 3, name: "transport-runtime", sql: TRANSPORT_RUNTIME_SQL },
    Migration { version: 4, name: "provider-runtime", sql: PROVIDER_RUNTIME_SQL },
    Migration { version: 5, name: "operational-status", sql: OPERATIONAL_STATUS_SQL },
    Migration { version: 6, name: "context-resets", sql: CONTEXT_RESETS_SQL },
];

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database actor is unavailable")]
    ActorUnavailable,
    #[error("database actor stopped unexpectedly")]
    ActorStopped,
    #[error("database I/O failed for {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database {path} is not an empty or Braid Rust database; move it aside explicitly")]
    ForeignDatabase { path: PathBuf },
    #[error("database schema {found} is newer than this binary's supported schema {supported}")]
    NewerSchema { found: u32, supported: u32 },
    #[error("migration {version} checksum differs from the embedded immutable migration")]
    ChecksumMismatch { version: u32 },
    #[error("migration ledger is not contiguous at version {version}")]
    NonContiguous { version: u32 },
    #[error("migration {version} failed: {message}")]
    Migration { version: u32, message: String },
    #[error("backup target {0} already exists")]
    BackupExists(PathBuf),
    #[error("another process holds the migration lease {0}")]
    MigrationBusy(PathBuf),
    #[error("database schema {found} is not ready; apply migrations through schema {required}")]
    SchemaNotReady { found: u32, required: u32 },
    #[error("invalid durable state: {0}")]
    InvalidData(String),
    #[error("another Braid runtime owns {scope} until {expires_at}")]
    RuntimeBusy { scope: String, expires_at: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct MigrationPlan {
    pub database: PathBuf,
    pub current_schema: u32,
    pub supported_schema: u32,
    pub pending: Vec<PendingMigration>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingMigration {
    pub version: u32,
    pub name: &'static str,
    pub checksum: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MigrationResult {
    pub previous_schema: u32,
    pub current_schema: u32,
    pub applied: Vec<u32>,
    pub backup: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoreStatus {
    pub database: PathBuf,
    pub exists: bool,
    pub bytes: u64,
    pub schema_version: u32,
    pub supported_schema: u32,
    pub pending_migrations: usize,
    pub journal_mode: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IngressEvent {
    pub delivery_guid: String,
    pub event_name: String,
    pub action: Option<String>,
    pub repository_node_id: String,
    pub repository: String,
    pub work_item_node_id: Option<String>,
    pub work_item_kind: Option<&'static str>,
    pub work_item_number: Option<u64>,
    pub work_item_state: Option<String>,
    pub object_node_id: Option<String>,
    pub object_version: Option<String>,
    pub object_digest: Option<String>,
    pub actor_node_id: Option<String>,
    pub actor_login: Option<String>,
    pub classification: &'static str,
    pub origin: &'static str,
    pub reference: String,
    pub mention_candidate: bool,
    pub reaction_target: Option<ReactionTarget>,
    pub known: bool,
    pub raw_payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ReactionTarget {
    pub kind: &'static str,
    pub database_id: String,
}

#[derive(Debug, Clone, Copy)]
pub struct SchedulerPolicy {
    pub quiet_seconds: u64,
    pub event_threshold: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct IngestResult {
    pub duplicate: bool,
    pub delivery_guid: String,
    pub event_id: Option<String>,
    pub event_lifecycle: Option<String>,
    pub batch_id: Option<String>,
    pub batch_lifecycle: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MentionCandidate {
    pub event_id: String,
    pub actor_login: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeStoreStatus {
    pub owner: Option<LeaseStatus>,
    pub deliveries: u64,
    pub duplicate_deliveries: u64,
    pub unknown_deliveries: u64,
    pub pending_batches: u64,
    pub runnable_batches: u64,
    pub pending_mentions: u64,
    pub pending_writes: u64,
    pub uncertain_writes: u64,
    pub last_reconciliation: Option<String>,
    pub batches: Vec<WakeBatchSummary>,
    pub agent_groups: Vec<AgentGroupSummary>,
    pub context_resets: Vec<ContextResetSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WakeBatchSummary {
    pub batch_id: String,
    pub repository: String,
    pub work_item_kind: String,
    pub work_item_number: u64,
    pub work_item_node_id: String,
    pub event_count: u64,
    pub quiet_deadline: String,
    pub urgent: bool,
    pub lifecycle: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LeaseStatus {
    pub scope: String,
    pub generation: u64,
    pub owner_id: String,
    pub expires_at: String,
}

#[derive(Debug, Clone)]
pub struct RuntimeLease {
    pub scope: String,
    pub generation: u64,
    pub owner_id: String,
}

#[derive(Debug, Clone)]
pub struct PendingGitHubWrite {
    pub intent_id: String,
    pub repository: String,
    pub target_kind: String,
    pub target_database_id: String,
    pub operation: String,
    pub content: String,
    pub remote_database_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TrackedWorkItem {
    pub node_id: String,
    pub repository: String,
    pub kind: String,
    pub number: u64,
}

#[derive(Debug, Clone)]
pub struct CanonicalObjectState {
    pub node_id: String,
    pub database_id: Option<String>,
    pub object_kind: String,
    pub version: String,
    pub digest: String,
    pub lifecycle: String,
    pub author_node_id: Option<String>,
    pub author_login: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProfileRecord {
    pub profile_id: String,
    pub revision: u64,
    pub effective_digest: String,
    pub provider_kind: String,
    pub tags: String,
}

#[derive(Debug, Clone)]
pub struct AssignmentCandidate {
    pub event_id: String,
    pub action: String,
    pub repository: String,
    pub number: u64,
}

#[derive(Debug, Clone)]
pub struct IssueLifecycleCandidate {
    pub event_id: String,
    pub action: String,
    pub repository: String,
    pub number: u64,
}

#[derive(Debug, Clone)]
pub struct AgentMaterialization {
    pub assignment_id: String,
    pub agent_id: String,
    pub work_item_node_id: String,
    pub generation: u64,
    pub profile_id: String,
    pub profile_revision: u64,
}

#[derive(Debug, Clone)]
pub struct TurnClaim {
    pub turn_id: String,
    pub batch_id: String,
    pub provider_session_id: String,
    pub repository: String,
    pub number: u64,
    pub context_revision: String,
    pub profile_id: String,
    pub references: Vec<String>,
    pub trusted_mention: bool,
    pub trigger_kind: String,
}

#[derive(Debug, Clone)]
pub struct ContextResetClaim {
    pub reset_id: String,
    pub repository: String,
    pub number: u64,
    pub profile_id: String,
    pub old_provider_session_id: String,
    pub active_turn_id: Option<String>,
    pub provider_turn_id: Option<String>,
    pub references: Vec<String>,
    pub continuation: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentGroupSummary {
    pub work_item_kind: String,
    pub work_item_number: u64,
    pub profile_id: String,
    pub assignment_generation: u64,
    pub assignment_lifecycle: String,
    pub provider_session_id: Option<String>,
    pub session_lifecycle: Option<String>,
    pub active_turn_id: Option<String>,
    pub turn_lifecycle: Option<String>,
    pub finalization_turns: u64,
    pub last_finalization_lifecycle: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextResetSummary {
    pub reset_id: String,
    pub repository: String,
    pub work_item_kind: String,
    pub work_item_number: u64,
    pub profile_id: String,
    pub lifecycle: String,
    pub continuation: bool,
    pub old_provider_session_id: String,
    pub new_provider_session_id: Option<String>,
    pub context_revision_before: String,
    pub context_revision_after: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReconciliationRun {
    pub run_id: String,
    pub repository_node_id: String,
}

#[derive(Debug, Clone)]
pub struct CanonicalComment {
    pub node_id: String,
    pub database_id: String,
    pub object_kind: &'static str,
    pub version: String,
    pub digest: String,
    pub lifecycle: &'static str,
    pub author_node_id: Option<String>,
    pub author_login: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub pinned: bool,
}

#[derive(Debug, Clone)]
pub struct CanonicalCommentSet {
    pub repository_node_id: String,
    pub repository: String,
    pub work_item_node_id: String,
    pub work_item_kind: &'static str,
    pub work_item_number: u64,
    pub work_item_state: String,
    pub work_item_version: String,
    pub work_item_digest: String,
    pub object_kind: &'static str,
    pub comments: Vec<CanonicalComment>,
}

#[derive(Debug, Clone)]
pub struct AssociatedWorkItem {
    pub node_id: String,
    pub repository_node_id: String,
    pub repository: String,
    pub kind: &'static str,
    pub number: u64,
    pub state: String,
}

#[derive(Debug, Clone)]
pub struct AssociationSet {
    pub anchor_node_id: String,
    pub anchor_kind: &'static str,
    pub observed_version: String,
    pub related: Vec<AssociatedWorkItem>,
}

#[derive(Debug, Clone)]
pub struct DeletedComment {
    pub node_id: String,
    pub database_id: String,
    pub author_node_id: Option<String>,
    pub author_login: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub pinned: bool,
}

pub struct StoreActor {
    sender: Sender<Command>,
    join: Option<thread::JoinHandle<()>>,
}

impl StoreActor {
    pub fn start(database: PathBuf, backups: PathBuf) -> Result<Self, StoreError> {
        let (sender, receiver) = mpsc::channel();
        let join = thread::Builder::new()
            .name("braid-sqlite".into())
            .spawn(move || actor_loop(&database, &backups, receiver))
            .map_err(|source| StoreError::Io { path: PathBuf::from("<database-actor>"), source })?;
        Ok(Self { sender, join: Some(join) })
    }

    pub fn plan(&self) -> Result<MigrationPlan, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender.send(Command::Plan(reply)).map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn apply(&self) -> Result<MigrationResult, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender.send(Command::Apply(reply)).map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn status(&self) -> Result<StoreStatus, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender.send(Command::Status(reply)).map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn reconcile_comments(
        &self,
        update: CanonicalCommentSet,
    ) -> Result<Vec<DeletedComment>, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::ReconcileComments(update, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn set_context_revision(
        &self,
        work_item_node_id: String,
        revision: String,
    ) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::SetContextRevision(work_item_node_id, revision, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn operational_status_comment_ids(
        &self,
        work_item_node_id: String,
    ) -> Result<BTreeSet<String>, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::OperationalStatusCommentIds(work_item_node_id, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn reconcile_associations(&self, update: AssociationSet) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::ReconcileAssociations(update, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn ingest_event(
        &self,
        event: IngressEvent,
        policy: SchedulerPolicy,
    ) -> Result<IngestResult, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::IngestEvent(Box::new(event), policy, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn mention_candidates(&self, limit: usize) -> Result<Vec<MentionCandidate>, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::MentionCandidates(limit, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn resolve_mention(
        &self,
        event_id: String,
        trusted: bool,
        policy: SchedulerPolicy,
    ) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::ResolveMention(event_id, trusted, policy, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn advance_scheduler(&self) -> Result<u64, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::AdvanceScheduler(reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn runtime_status(&self) -> Result<RuntimeStoreStatus, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::RuntimeStatus(reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn acquire_runtime_lease(
        &self,
        scope: String,
        owner_id: String,
        ttl_seconds: u64,
    ) -> Result<RuntimeLease, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::AcquireRuntimeLease(scope, owner_id, ttl_seconds, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn renew_runtime_lease(
        &self,
        lease: RuntimeLease,
        ttl_seconds: u64,
    ) -> Result<RuntimeLease, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::RenewRuntimeLease(lease, ttl_seconds, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn release_runtime_lease(&self, lease: RuntimeLease) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::ReleaseRuntimeLease(lease, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn recover_writes(&self) -> Result<u64, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::RecoverWrites(reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn claim_github_write(&self) -> Result<Option<PendingGitHubWrite>, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::ClaimGitHubWrite(reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn finish_github_write(
        &self,
        intent_id: String,
        lifecycle: &'static str,
        remote_database_id: Option<String>,
        remote_node_id: Option<String>,
        error: Option<String>,
    ) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::FinishGitHubWrite(
                intent_id,
                lifecycle,
                remote_database_id,
                remote_node_id,
                error,
                reply,
            ))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn tracked_work_items(&self) -> Result<Vec<TrackedWorkItem>, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::TrackedWorkItems(reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn canonical_objects(
        &self,
        work_item_node_id: String,
    ) -> Result<Vec<CanonicalObjectState>, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::CanonicalObjects(work_item_node_id, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn begin_reconciliation(
        &self,
        repository_node_id: String,
    ) -> Result<ReconciliationRun, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::BeginReconciliation(repository_node_id, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn finish_reconciliation(
        &self,
        run: ReconciliationRun,
        lifecycle: &'static str,
        work_item_count: usize,
        change_count: usize,
        error: Option<String>,
    ) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::FinishReconciliation(
                run,
                lifecycle,
                work_item_count,
                change_count,
                error,
                reply,
            ))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn register_profile(&self, profile: ProfileRecord) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::RegisterProfile(profile, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn assignment_candidates(
        &self,
        limit: usize,
    ) -> Result<Vec<AssignmentCandidate>, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::AssignmentCandidates(limit, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn issue_lifecycle_candidates(
        &self,
        limit: usize,
    ) -> Result<Vec<IssueLifecycleCandidate>, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::IssueLifecycleCandidates(limit, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn prepare_issue_finalization(&self, event_id: String) -> Result<bool, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::PrepareIssueFinalization(event_id, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn begin_issue_reactivation(
        &self,
        event_id: String,
    ) -> Result<Option<AgentMaterialization>, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::BeginIssueReactivation(event_id, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    #[allow(clippy::too_many_arguments)]
    pub fn complete_issue_reactivation(
        &self,
        event_id: String,
        materialization: AgentMaterialization,
        provider_session_id: String,
        context_revision: String,
        instruction_revision: String,
        policy: SchedulerPolicy,
    ) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::CompleteIssueReactivation(
                event_id,
                materialization,
                provider_session_id,
                context_revision,
                instruction_revision,
                policy,
                reply,
            ))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn fail_issue_reactivation(
        &self,
        event_id: String,
        assignment_id: String,
        error: String,
    ) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::FailIssueReactivation(event_id, assignment_id, error, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn has_lifecycle_observation(
        &self,
        work_item_node_id: String,
        action: String,
        object_version: String,
    ) -> Result<bool, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::HasLifecycleObservation(
                work_item_node_id,
                action,
                object_version,
                reply,
            ))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn begin_issue_assignment(
        &self,
        event_id: String,
        profile: ProfileRecord,
        context_revision: String,
        preserve_wake_batch: bool,
    ) -> Result<Option<AgentMaterialization>, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::BeginIssueAssignment(
                event_id,
                profile,
                context_revision,
                preserve_wake_batch,
                reply,
            ))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn ignore_assignment_event(&self, event_id: String) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::IgnoreAssignmentEvent(event_id, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn complete_issue_assignment(
        &self,
        materialization: AgentMaterialization,
        provider_session_id: String,
        context_revision: String,
        instruction_revision: String,
    ) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::CompleteIssueAssignment(
                materialization,
                provider_session_id,
                context_revision,
                instruction_revision,
                reply,
            ))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn fail_issue_assignment(
        &self,
        assignment_id: String,
        error: String,
    ) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::FailIssueAssignment(assignment_id, error, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn claim_runnable_turn(&self) -> Result<Option<TurnClaim>, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::ClaimRunnableTurn(reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn mark_turn_started(
        &self,
        turn_id: String,
        provider_turn_id: String,
    ) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::MarkTurnStarted(turn_id, provider_turn_id, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn mark_turn_terminal(&self, turn_id: String, lifecycle: String) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::MarkTurnTerminal(turn_id, lifecycle, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn claim_urgent_steer(&self, turn_id: String) -> Result<Option<TurnClaim>, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::ClaimUrgentSteer(turn_id, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn consume_steer_batch(&self, batch_id: String) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::ConsumeSteerBatch(batch_id, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn enqueue_turn_reaction(
        &self,
        turn_id: String,
        content: String,
    ) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::EnqueueTurnReaction(turn_id, content, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn enqueue_operational_status(
        &self,
        turn_id: String,
        body: String,
    ) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::EnqueueOperationalStatus(turn_id, body, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn begin_context_reset(
        &self,
        active_turn_id: Option<String>,
    ) -> Result<Option<ContextResetClaim>, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::BeginContextReset(active_turn_id, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn ready_context_reset(&self) -> Result<Option<ContextResetClaim>, StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::ReadyContextReset(reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn mark_context_reset_turn_terminal(
        &self,
        reset_id: String,
        turn_id: String,
        lifecycle: String,
    ) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::MarkContextResetTurnTerminal(reset_id, turn_id, lifecycle, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn complete_context_reset(
        &self,
        reset_id: String,
        provider_session_id: String,
        context_revision: String,
        instruction_revision: String,
    ) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::CompleteContextReset(
                reset_id,
                provider_session_id,
                context_revision,
                instruction_revision,
                reply,
            ))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }

    pub fn fail_context_reset(&self, reset_id: String, error: String) -> Result<(), StoreError> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(Command::FailContextReset(reset_id, error, reply))
            .map_err(|_| StoreError::ActorUnavailable)?;
        receiver.recv().map_err(|_| StoreError::ActorStopped)?
    }
}

impl Drop for StoreActor {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

enum Command {
    Plan(Sender<Result<MigrationPlan, StoreError>>),
    Apply(Sender<Result<MigrationResult, StoreError>>),
    Status(Sender<Result<StoreStatus, StoreError>>),
    ReconcileComments(CanonicalCommentSet, Sender<Result<Vec<DeletedComment>, StoreError>>),
    SetContextRevision(String, String, Sender<Result<(), StoreError>>),
    OperationalStatusCommentIds(String, Sender<Result<BTreeSet<String>, StoreError>>),
    ReconcileAssociations(AssociationSet, Sender<Result<(), StoreError>>),
    IngestEvent(Box<IngressEvent>, SchedulerPolicy, Sender<Result<IngestResult, StoreError>>),
    MentionCandidates(usize, Sender<Result<Vec<MentionCandidate>, StoreError>>),
    ResolveMention(String, bool, SchedulerPolicy, Sender<Result<(), StoreError>>),
    AdvanceScheduler(Sender<Result<u64, StoreError>>),
    RuntimeStatus(Sender<Result<RuntimeStoreStatus, StoreError>>),
    AcquireRuntimeLease(String, String, u64, Sender<Result<RuntimeLease, StoreError>>),
    RenewRuntimeLease(RuntimeLease, u64, Sender<Result<RuntimeLease, StoreError>>),
    ReleaseRuntimeLease(RuntimeLease, Sender<Result<(), StoreError>>),
    RecoverWrites(Sender<Result<u64, StoreError>>),
    ClaimGitHubWrite(Sender<Result<Option<PendingGitHubWrite>, StoreError>>),
    FinishGitHubWrite(
        String,
        &'static str,
        Option<String>,
        Option<String>,
        Option<String>,
        Sender<Result<(), StoreError>>,
    ),
    TrackedWorkItems(Sender<Result<Vec<TrackedWorkItem>, StoreError>>),
    CanonicalObjects(String, Sender<Result<Vec<CanonicalObjectState>, StoreError>>),
    BeginReconciliation(String, Sender<Result<ReconciliationRun, StoreError>>),
    FinishReconciliation(
        ReconciliationRun,
        &'static str,
        usize,
        usize,
        Option<String>,
        Sender<Result<(), StoreError>>,
    ),
    RegisterProfile(ProfileRecord, Sender<Result<(), StoreError>>),
    AssignmentCandidates(usize, Sender<Result<Vec<AssignmentCandidate>, StoreError>>),
    IssueLifecycleCandidates(usize, Sender<Result<Vec<IssueLifecycleCandidate>, StoreError>>),
    PrepareIssueFinalization(String, Sender<Result<bool, StoreError>>),
    BeginIssueReactivation(String, Sender<Result<Option<AgentMaterialization>, StoreError>>),
    CompleteIssueReactivation(
        String,
        AgentMaterialization,
        String,
        String,
        String,
        SchedulerPolicy,
        Sender<Result<(), StoreError>>,
    ),
    FailIssueReactivation(String, String, String, Sender<Result<(), StoreError>>),
    HasLifecycleObservation(String, String, String, Sender<Result<bool, StoreError>>),
    BeginIssueAssignment(
        String,
        ProfileRecord,
        String,
        bool,
        Sender<Result<Option<AgentMaterialization>, StoreError>>,
    ),
    IgnoreAssignmentEvent(String, Sender<Result<(), StoreError>>),
    CompleteIssueAssignment(
        AgentMaterialization,
        String,
        String,
        String,
        Sender<Result<(), StoreError>>,
    ),
    FailIssueAssignment(String, String, Sender<Result<(), StoreError>>),
    ClaimRunnableTurn(Sender<Result<Option<TurnClaim>, StoreError>>),
    MarkTurnStarted(String, String, Sender<Result<(), StoreError>>),
    MarkTurnTerminal(String, String, Sender<Result<(), StoreError>>),
    ClaimUrgentSteer(String, Sender<Result<Option<TurnClaim>, StoreError>>),
    ConsumeSteerBatch(String, Sender<Result<(), StoreError>>),
    EnqueueTurnReaction(String, String, Sender<Result<(), StoreError>>),
    EnqueueOperationalStatus(String, String, Sender<Result<(), StoreError>>),
    BeginContextReset(Option<String>, Sender<Result<Option<ContextResetClaim>, StoreError>>),
    ReadyContextReset(Sender<Result<Option<ContextResetClaim>, StoreError>>),
    MarkContextResetTurnTerminal(String, String, String, Sender<Result<(), StoreError>>),
    CompleteContextReset(String, String, String, String, Sender<Result<(), StoreError>>),
    FailContextReset(String, String, Sender<Result<(), StoreError>>),
    Shutdown,
}

#[derive(Debug)]
struct Migration {
    version: u32,
    name: &'static str,
    sql: &'static str,
}

#[derive(Debug)]
struct LedgerEntry {
    version: u32,
    checksum: String,
}

#[allow(clippy::too_many_lines)]
fn actor_loop(database: &Path, backups: &Path, receiver: Receiver<Command>) {
    for command in receiver {
        match command {
            Command::Plan(reply) => {
                let _ = reply.send(plan(database));
            }
            Command::Apply(reply) => {
                let _ = reply.send(apply(database, backups));
            }
            Command::Status(reply) => {
                let _ = reply.send(status(database));
            }
            Command::ReconcileComments(update, reply) => {
                let _ = reply.send(reconcile_comments(database, &update));
            }
            Command::SetContextRevision(work_item_node_id, revision, reply) => {
                let _ = reply.send(set_context_revision(database, &work_item_node_id, &revision));
            }
            Command::OperationalStatusCommentIds(work_item_node_id, reply) => {
                let _ = reply.send(operational_status_comment_ids(database, &work_item_node_id));
            }
            Command::ReconcileAssociations(update, reply) => {
                let _ = reply.send(reconcile_associations(database, &update));
            }
            Command::IngestEvent(event, policy, reply) => {
                let _ = reply.send(ingest_event(database, &event, policy));
            }
            Command::MentionCandidates(limit, reply) => {
                let _ = reply.send(mention_candidates(database, limit));
            }
            Command::ResolveMention(event_id, trusted, policy, reply) => {
                let _ = reply.send(resolve_mention(database, &event_id, trusted, policy));
            }
            Command::AdvanceScheduler(reply) => {
                let _ = reply.send(advance_scheduler(database));
            }
            Command::RuntimeStatus(reply) => {
                let _ = reply.send(runtime_status(database));
            }
            Command::AcquireRuntimeLease(scope, owner_id, ttl_seconds, reply) => {
                let _ = reply.send(acquire_runtime_lease(database, &scope, &owner_id, ttl_seconds));
            }
            Command::RenewRuntimeLease(lease, ttl_seconds, reply) => {
                let _ = reply.send(renew_runtime_lease(database, &lease, ttl_seconds));
            }
            Command::ReleaseRuntimeLease(lease, reply) => {
                let _ = reply.send(release_runtime_lease(database, &lease));
            }
            Command::RecoverWrites(reply) => {
                let _ = reply.send(recover_writes(database));
            }
            Command::ClaimGitHubWrite(reply) => {
                let _ = reply.send(claim_github_write(database));
            }
            Command::FinishGitHubWrite(
                intent_id,
                lifecycle,
                remote_database_id,
                remote_node_id,
                error,
                reply,
            ) => {
                let _ = reply.send(finish_github_write(
                    database,
                    &intent_id,
                    lifecycle,
                    remote_database_id.as_deref(),
                    remote_node_id.as_deref(),
                    error.as_deref(),
                ));
            }
            Command::TrackedWorkItems(reply) => {
                let _ = reply.send(tracked_work_items(database));
            }
            Command::CanonicalObjects(work_item_node_id, reply) => {
                let _ = reply.send(canonical_objects(database, &work_item_node_id));
            }
            Command::BeginReconciliation(repository_node_id, reply) => {
                let _ = reply.send(begin_reconciliation(database, &repository_node_id));
            }
            Command::FinishReconciliation(
                run,
                lifecycle,
                work_item_count,
                change_count,
                error,
                reply,
            ) => {
                let _ = reply.send(finish_reconciliation(
                    database,
                    &run,
                    lifecycle,
                    work_item_count,
                    change_count,
                    error.as_deref(),
                ));
            }
            Command::RegisterProfile(profile, reply) => {
                let _ = reply.send(register_profile(database, &profile));
            }
            Command::AssignmentCandidates(limit, reply) => {
                let _ = reply.send(assignment_candidates(database, limit));
            }
            Command::IssueLifecycleCandidates(limit, reply) => {
                let _ = reply.send(issue_lifecycle_candidates(database, limit));
            }
            Command::PrepareIssueFinalization(event_id, reply) => {
                let _ = reply.send(prepare_issue_finalization(database, &event_id));
            }
            Command::BeginIssueReactivation(event_id, reply) => {
                let _ = reply.send(begin_issue_reactivation(database, &event_id));
            }
            Command::CompleteIssueReactivation(
                event_id,
                materialization,
                provider_session_id,
                context_revision,
                instruction_revision,
                policy,
                reply,
            ) => {
                let _ = reply.send(complete_issue_reactivation(
                    database,
                    &event_id,
                    &materialization,
                    &provider_session_id,
                    &context_revision,
                    &instruction_revision,
                    policy,
                ));
            }
            Command::FailIssueReactivation(event_id, assignment_id, error, reply) => {
                let _ = reply.send(fail_issue_reactivation(
                    database,
                    &event_id,
                    &assignment_id,
                    &error,
                ));
            }
            Command::HasLifecycleObservation(work_item_node_id, action, object_version, reply) => {
                let _ = reply.send(has_lifecycle_observation(
                    database,
                    &work_item_node_id,
                    &action,
                    &object_version,
                ));
            }
            Command::BeginIssueAssignment(
                event_id,
                profile,
                context_revision,
                preserve_wake_batch,
                reply,
            ) => {
                let _ = reply.send(begin_issue_assignment(
                    database,
                    &event_id,
                    &profile,
                    &context_revision,
                    preserve_wake_batch,
                ));
            }
            Command::IgnoreAssignmentEvent(event_id, reply) => {
                let _ = reply.send(ignore_assignment_event(database, &event_id));
            }
            Command::CompleteIssueAssignment(
                materialization,
                provider_session_id,
                context_revision,
                instruction_revision,
                reply,
            ) => {
                let _ = reply.send(complete_issue_assignment(
                    database,
                    &materialization,
                    &provider_session_id,
                    &context_revision,
                    &instruction_revision,
                ));
            }
            Command::FailIssueAssignment(assignment_id, error, reply) => {
                let _ = reply.send(fail_issue_assignment(database, &assignment_id, &error));
            }
            Command::ClaimRunnableTurn(reply) => {
                let _ = reply.send(claim_runnable_turn(database));
            }
            Command::MarkTurnStarted(turn_id, provider_turn_id, reply) => {
                let _ = reply.send(mark_turn_started(database, &turn_id, &provider_turn_id));
            }
            Command::MarkTurnTerminal(turn_id, lifecycle, reply) => {
                let _ = reply.send(mark_turn_terminal(database, &turn_id, &lifecycle));
            }
            Command::ClaimUrgentSteer(turn_id, reply) => {
                let _ = reply.send(claim_urgent_steer(database, &turn_id));
            }
            Command::ConsumeSteerBatch(batch_id, reply) => {
                let _ = reply.send(consume_steer_batch(database, &batch_id));
            }
            Command::EnqueueTurnReaction(turn_id, content, reply) => {
                let _ = reply.send(enqueue_turn_reaction(database, &turn_id, &content));
            }
            Command::EnqueueOperationalStatus(turn_id, body, reply) => {
                let _ = reply.send(enqueue_operational_status(database, &turn_id, &body));
            }
            Command::BeginContextReset(active_turn_id, reply) => {
                let _ = reply.send(begin_context_reset(database, active_turn_id.as_deref()));
            }
            Command::ReadyContextReset(reply) => {
                let _ = reply.send(ready_context_reset(database));
            }
            Command::MarkContextResetTurnTerminal(reset_id, turn_id, lifecycle, reply) => {
                let _ = reply.send(mark_context_reset_turn_terminal(
                    database, &reset_id, &turn_id, &lifecycle,
                ));
            }
            Command::CompleteContextReset(
                reset_id,
                provider_session_id,
                context_revision,
                instruction_revision,
                reply,
            ) => {
                let _ = reply.send(complete_context_reset(
                    database,
                    &reset_id,
                    &provider_session_id,
                    &context_revision,
                    &instruction_revision,
                ));
            }
            Command::FailContextReset(reset_id, error, reply) => {
                let _ = reply.send(fail_context_reset(database, &reset_id, &error));
            }
            Command::Shutdown => break,
        }
    }
}

fn plan(database: &Path) -> Result<MigrationPlan, StoreError> {
    let ledger = read_ledger(database)?;
    validate_ledger(&ledger)?;
    let current_schema = ledger.last().map_or(0, |entry| entry.version);
    Ok(MigrationPlan {
        database: database.to_path_buf(),
        current_schema,
        supported_schema: DATABASE_SCHEMA_VERSION,
        pending: MIGRATIONS
            .iter()
            .filter(|migration| migration.version > current_schema)
            .map(|migration| PendingMigration {
                version: migration.version,
                name: migration.name,
                checksum: migration_checksum(migration),
            })
            .collect(),
    })
}

fn apply(database: &Path, backups: &Path) -> Result<MigrationResult, StoreError> {
    if let Some(parent) = database.parent() {
        create_dir_all(parent)?;
    }
    create_dir_all(backups)?;
    let _lease = MigrationLease::acquire(database)?;

    let before = plan(database)?;
    if before.pending.is_empty() {
        return Ok(MigrationResult {
            previous_schema: before.current_schema,
            current_schema: before.current_schema,
            applied: Vec::new(),
            backup: None,
        });
    }

    let existed_with_bytes = fs::metadata(database).is_ok_and(|metadata| metadata.len() > 0);
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let backup = if existed_with_bytes {
        Some(create_backup(&connection, backups, before.pending[0].version)?)
    } else {
        None
    };

    let mut applied = Vec::new();
    for migration in MIGRATIONS.iter().filter(|migration| migration.version > before.current_schema)
    {
        apply_one(&mut connection, migration)?;
        applied.push(migration.version);
    }
    let after = plan(database)?;
    Ok(MigrationResult {
        previous_schema: before.current_schema,
        current_schema: after.current_schema,
        applied,
        backup,
    })
}

fn status(database: &Path) -> Result<StoreStatus, StoreError> {
    let exists = database.is_file();
    let bytes = fs::metadata(database).map_or(0, |metadata| metadata.len());
    let migration_plan = plan(database)?;
    let journal_mode = if exists && bytes > 0 {
        let connection = open_read_only(database)?;
        connection.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0)).optional()?
    } else {
        None
    };
    Ok(StoreStatus {
        database: database.to_path_buf(),
        exists,
        bytes,
        schema_version: migration_plan.current_schema,
        supported_schema: DATABASE_SCHEMA_VERSION,
        pending_migrations: migration_plan.pending.len(),
        journal_mode,
    })
}

#[derive(Debug)]
struct ExistingComment {
    node_id: String,
}

#[allow(clippy::too_many_lines)]
fn reconcile_comments(
    database: &Path,
    update: &CanonicalCommentSet,
) -> Result<Vec<DeletedComment>, StoreError> {
    require_current_schema(database)?;
    let number = i64::try_from(update.work_item_number).map_err(|_| {
        StoreError::InvalidData("GitHub work-item number exceeds SQLite INTEGER".into())
    })?;
    let observed_at = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO repositories(node_id, name_with_owner, observed_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(node_id) DO UPDATE SET
           name_with_owner=excluded.name_with_owner,
           observed_at=excluded.observed_at",
        params![update.repository_node_id, update.repository, observed_at],
    )?;
    transaction.execute(
        "INSERT INTO work_items(node_id, repository_node_id, kind, number, state, observed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(node_id) DO UPDATE SET
           repository_node_id=excluded.repository_node_id,
           kind=excluded.kind,
           number=excluded.number,
           state=excluded.state,
           observed_at=excluded.observed_at",
        params![
            update.work_item_node_id,
            update.repository_node_id,
            update.work_item_kind,
            number,
            update.work_item_state,
            observed_at,
        ],
    )?;
    transaction.execute(
        "INSERT INTO canonical_objects(
           node_id, work_item_node_id, object_kind, version, digest, lifecycle,
           observed_at, reference_repository, reference_number
         ) VALUES (?1,?1,?2,?3,?4,'active',?5,?6,?7)
         ON CONFLICT(node_id) DO UPDATE SET
           work_item_node_id=excluded.work_item_node_id,
           object_kind=excluded.object_kind,
           version=excluded.version,
           digest=excluded.digest,
           lifecycle='active',
           observed_at=excluded.observed_at,
           reference_repository=excluded.reference_repository,
           reference_number=excluded.reference_number",
        params![
            update.work_item_node_id,
            update.work_item_kind,
            update.work_item_version,
            update.work_item_digest,
            observed_at,
            update.repository,
            number,
        ],
    )?;

    let previous = {
        let mut statement = transaction.prepare(
            "SELECT node_id
             FROM canonical_objects
             WHERE work_item_node_id=?1 AND object_kind=?2 AND lifecycle != 'deleted'",
        )?;
        let rows = statement
            .query_map(params![update.work_item_node_id, update.object_kind], |row| {
                Ok(ExistingComment { node_id: row.get(0)? })
            })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let current_ids =
        update.comments.iter().map(|comment| comment.node_id.as_str()).collect::<BTreeSet<_>>();
    for comment in &update.comments {
        transaction.execute(
            "INSERT INTO canonical_objects(
               node_id, work_item_node_id, object_kind, version, digest, lifecycle,
               author_node_id, created_at, updated_at, observed_at,
               database_id, author_login, reference_repository, reference_number, pinned
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
             ON CONFLICT(node_id) DO UPDATE SET
               work_item_node_id=excluded.work_item_node_id,
               object_kind=excluded.object_kind,
               version=excluded.version,
               digest=excluded.digest,
               lifecycle=excluded.lifecycle,
               author_node_id=excluded.author_node_id,
               created_at=excluded.created_at,
               updated_at=excluded.updated_at,
               observed_at=excluded.observed_at,
               database_id=excluded.database_id,
               author_login=excluded.author_login,
               reference_repository=excluded.reference_repository,
               reference_number=excluded.reference_number,
               pinned=excluded.pinned",
            params![
                comment.node_id,
                update.work_item_node_id,
                comment.object_kind,
                comment.version,
                comment.digest,
                comment.lifecycle,
                comment.author_node_id,
                comment.created_at,
                comment.updated_at,
                observed_at,
                comment.database_id,
                comment.author_login,
                update.repository,
                number,
                i64::from(comment.pinned),
            ],
        )?;
    }
    for comment in previous {
        if !current_ids.contains(comment.node_id.as_str()) {
            transaction.execute(
                "UPDATE canonical_objects
                 SET lifecycle='deleted', version=?2, observed_at=?2
                 WHERE node_id=?1",
                params![comment.node_id, observed_at],
            )?;
        }
    }
    let deleted = {
        let mut statement = transaction.prepare(
            "SELECT node_id, database_id, author_node_id, author_login, created_at, updated_at, pinned
             FROM canonical_objects
             WHERE work_item_node_id=?1 AND object_kind=?2 AND lifecycle='deleted'
             ORDER BY created_at, node_id",
        )?;
        let rows =
            statement.query_map(params![update.work_item_node_id, update.object_kind], |row| {
                Ok(DeletedComment {
                    node_id: row.get(0)?,
                    database_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    author_node_id: row.get(2)?,
                    author_login: row.get(3)?,
                    created_at: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    updated_at: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                    pinned: row.get::<_, i64>(6)? != 0,
                })
            })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    transaction.commit()?;
    Ok(deleted)
}

fn set_context_revision(
    database: &Path,
    work_item_node_id: &str,
    revision: &str,
) -> Result<(), StoreError> {
    require_current_schema(database)?;
    let connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let updated = connection.execute(
        "UPDATE work_items SET context_revision=?2, observed_at=?3 WHERE node_id=?1",
        params![work_item_node_id, revision, now_rfc3339()],
    )?;
    if updated == 1 {
        Ok(())
    } else {
        Err(StoreError::InvalidData(format!(
            "cannot record Context Revision for unknown Work Item {work_item_node_id}"
        )))
    }
}

fn operational_status_comment_ids(
    database: &Path,
    work_item_node_id: &str,
) -> Result<BTreeSet<String>, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_only(database)?;
    let mut statement = connection.prepare(
        "SELECT remote_comment_node_id
         FROM status_comments
         WHERE work_item_node_id=?1 AND remote_comment_node_id IS NOT NULL AND lifecycle != 'deleted'",
    )?;
    let rows = statement.query_map([work_item_node_id], |row| row.get::<_, String>(0))?;
    rows.collect::<Result<BTreeSet<_>, _>>().map_err(StoreError::from)
}

#[allow(clippy::too_many_lines)]
fn reconcile_associations(database: &Path, update: &AssociationSet) -> Result<(), StoreError> {
    require_current_schema(database)?;
    let observed_at = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let mut active_pairs = BTreeSet::new();
    for related in &update.related {
        let number = i64::try_from(related.number).map_err(|_| {
            StoreError::InvalidData(
                "GitHub associated Work Item number exceeds SQLite INTEGER".into(),
            )
        })?;
        transaction.execute(
            "INSERT INTO repositories(node_id, name_with_owner, observed_at)
             VALUES (?1,?2,?3)
             ON CONFLICT(node_id) DO UPDATE SET
               name_with_owner=excluded.name_with_owner,
               observed_at=excluded.observed_at",
            params![related.repository_node_id, related.repository, observed_at],
        )?;
        transaction.execute(
            "INSERT INTO work_items(node_id, repository_node_id, kind, number, state, observed_at)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(node_id) DO UPDATE SET
               repository_node_id=excluded.repository_node_id,
               kind=excluded.kind,
               number=excluded.number,
               state=excluded.state,
               observed_at=excluded.observed_at",
            params![
                related.node_id,
                related.repository_node_id,
                related.kind,
                number,
                related.state,
                observed_at,
            ],
        )?;
        let (issue_node_id, pr_node_id) = if update.anchor_kind == "issue" {
            (update.anchor_node_id.as_str(), related.node_id.as_str())
        } else {
            (related.node_id.as_str(), update.anchor_node_id.as_str())
        };
        active_pairs.insert((issue_node_id.to_owned(), pr_node_id.to_owned()));
        transaction.execute(
            "INSERT INTO associations(issue_node_id, pr_node_id, source, observed_version, active)
             VALUES (?1,?2,'native',?3,1)
             ON CONFLICT(issue_node_id,pr_node_id) DO UPDATE SET
               source='native',
               observed_version=excluded.observed_version,
               active=1",
            params![issue_node_id, pr_node_id, update.observed_version],
        )?;
    }
    let anchor_column = if update.anchor_kind == "issue" { "issue_node_id" } else { "pr_node_id" };
    let query = format!(
        "SELECT issue_node_id, pr_node_id FROM associations WHERE {anchor_column}=?1 AND active=1"
    );
    let prior = {
        let mut statement = transaction.prepare(&query)?;
        let rows = statement.query_map([&update.anchor_node_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for pair in prior {
        if !active_pairs.contains(&pair) {
            transaction.execute(
                "UPDATE associations
                 SET active=0, observed_version=?3
                 WHERE issue_node_id=?1 AND pr_node_id=?2",
                params![pair.0, pair.1, update.observed_version],
            )?;
        }
    }
    transaction.commit()?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn ingest_event(
    database: &Path,
    event: &IngressEvent,
    policy: SchedulerPolicy,
) -> Result<IngestResult, StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let duplicate = transaction
        .query_row(
            "SELECT 1 FROM deliveries WHERE delivery_guid=?1",
            [&event.delivery_guid],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if duplicate {
        transaction.execute(
            "UPDATE deliveries SET duplicate_count=duplicate_count+1 WHERE delivery_guid=?1",
            [&event.delivery_guid],
        )?;
        transaction.commit()?;
        return Ok(IngestResult {
            duplicate: true,
            delivery_guid: event.delivery_guid.clone(),
            event_id: None,
            event_lifecycle: None,
            batch_id: None,
            batch_lifecycle: None,
        });
    }

    transaction.execute(
        "INSERT INTO repositories(node_id,name_with_owner,observed_at)
         VALUES (?1,?2,?3)
         ON CONFLICT(node_id) DO UPDATE SET
           name_with_owner=excluded.name_with_owner,
           observed_at=excluded.observed_at",
        params![event.repository_node_id, event.repository, now],
    )?;
    if let (Some(node_id), Some(kind), Some(number), Some(state)) = (
        event.work_item_node_id.as_deref(),
        event.work_item_kind,
        event.work_item_number,
        event.work_item_state.as_deref(),
    ) {
        let number = sqlite_u64(number, "GitHub work-item number")?;
        transaction.execute(
            "INSERT INTO work_items(node_id,repository_node_id,kind,number,state,observed_at)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(node_id) DO UPDATE SET
               repository_node_id=excluded.repository_node_id,
               kind=excluded.kind,
               number=excluded.number,
               state=excluded.state,
               observed_at=excluded.observed_at",
            params![node_id, event.repository_node_id, kind, number, state, now],
        )?;
    }
    transaction.execute(
        "INSERT INTO deliveries(
           delivery_guid,repository_node_id,event_name,action,received_at,admitted_at,
           repository_name,object_node_id,actor_node_id,actor_login,raw_payload,known
         ) VALUES (?1,?2,?3,?4,?5,?5,?6,?7,?8,?9,?10,?11)",
        params![
            event.delivery_guid,
            event.repository_node_id,
            event.event_name,
            event.action,
            now,
            event.repository,
            event.object_node_id,
            event.actor_node_id,
            event.actor_login,
            event.raw_payload,
            i64::from(event.known),
        ],
    )?;

    let event_id = Uuid::now_v7().to_string();
    let dedupe_key = event_dedupe_key(event);
    let object_kind = event_object_kind(event);
    let stale = match (
        object_kind,
        event.object_node_id.as_deref(),
        event.object_version.as_deref(),
        event.object_digest.as_deref(),
    ) {
        (Some(_), Some(node_id), Some(version), Some(digest)) => transaction
            .query_row(
                "SELECT version,digest FROM canonical_objects WHERE node_id=?1",
                [node_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .is_some_and(|(canonical_version, canonical_digest)| {
                canonical_version.as_str() > version
                    || (canonical_version == version && canonical_digest == digest)
            }),
        _ => false,
    };
    let lifecycle = if stale { "superseded" } else { "pending" };
    let inserted = transaction.execute(
        "INSERT OR IGNORE INTO events(
           event_id,delivery_guid,work_item_node_id,object_node_id,object_version,
           classification,origin,reference,lifecycle,observed_at,dedupe_key,
           mention_candidate,trusted_mention,body_digest
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,NULL,?13)",
        params![
            event_id,
            event.delivery_guid,
            event.work_item_node_id,
            event.object_node_id,
            event.object_version,
            event.classification,
            event.origin,
            event.reference,
            lifecycle,
            now,
            dedupe_key,
            i64::from(event.mention_candidate),
            event.object_digest,
        ],
    )?;
    if inserted == 0 {
        transaction.commit()?;
        return Ok(IngestResult {
            duplicate: false,
            delivery_guid: event.delivery_guid.clone(),
            event_id: None,
            event_lifecycle: Some("superseded".into()),
            batch_id: None,
            batch_lifecycle: None,
        });
    }

    if !stale
        && let (
            Some(object_kind),
            Some(node_id),
            Some(work_item_node_id),
            Some(version),
            Some(digest),
        ) = (
            object_kind,
            event.object_node_id.as_deref(),
            event.work_item_node_id.as_deref(),
            event.object_version.as_deref(),
            event.object_digest.as_deref(),
        )
    {
        let lifecycle = match event.action.as_deref() {
            Some("deleted") => "deleted",
            Some("minimized") => "minimized",
            Some("dismissed") => "dismissed",
            Some("resolved") => "resolved",
            _ => "active",
        };
        transaction.execute(
            "INSERT INTO canonical_objects(
               node_id,work_item_node_id,object_kind,version,digest,lifecycle,observed_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(node_id) DO UPDATE SET
               work_item_node_id=excluded.work_item_node_id,
               object_kind=excluded.object_kind,
               version=excluded.version,
               digest=CASE
                 WHEN excluded.object_kind IN ('issue','pr') AND ?8=0
                   THEN canonical_objects.digest
                 ELSE excluded.digest
               END,
               lifecycle=excluded.lifecycle,
               observed_at=excluded.observed_at",
            params![
                node_id,
                work_item_node_id,
                object_kind,
                version,
                digest,
                lifecycle,
                now,
                i64::from(event.delivery_guid.starts_with("reconcile-")),
            ],
        )?;
    }

    let mut batch = None;
    if lifecycle == "pending"
        && event.classification == "wake"
        && let Some(work_item_node_id) = event.work_item_node_id.as_deref()
    {
        batch =
            Some(schedule_event(&transaction, work_item_node_id, &event_id, policy, false, &now)?);
    }
    if lifecycle == "pending"
        && let Some(target) = &event.reaction_target
    {
        enqueue_reaction(&transaction, event, &event_id, target, &now)?;
    }
    transaction.commit()?;
    Ok(IngestResult {
        duplicate: false,
        delivery_guid: event.delivery_guid.clone(),
        event_id: Some(event_id),
        event_lifecycle: Some(lifecycle.into()),
        batch_id: batch.as_ref().map(|value| value.0.clone()),
        batch_lifecycle: batch.map(|value| value.1),
    })
}

fn event_object_kind(event: &IngressEvent) -> Option<&'static str> {
    match event.event_name.as_str() {
        "issues" => Some("issue"),
        "pull_request" => Some("pr"),
        "issue_comment" if event.work_item_kind == Some("pr") => Some("pr_comment"),
        "issue_comment" => Some("issue_comment"),
        "pull_request_review" => Some("review"),
        "pull_request_review_comment" => Some("review_comment"),
        "pull_request_review_thread" => Some("review_thread"),
        _ => None,
    }
}

fn event_dedupe_key(event: &IngressEvent) -> String {
    let mut digest = Sha256::new();
    for value in [
        event.repository_node_id.as_str(),
        event.object_node_id.as_deref().unwrap_or(""),
        event.object_version.as_deref().unwrap_or(""),
        event.object_digest.as_deref().unwrap_or(""),
        event.action.as_deref().unwrap_or(""),
        event.classification,
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    hex::encode(digest.finalize())
}

fn enqueue_reaction(
    transaction: &rusqlite::Transaction<'_>,
    event: &IngressEvent,
    event_id: &str,
    target: &ReactionTarget,
    now: &str,
) -> Result<(), StoreError> {
    let operation = "reaction_add";
    let content = "eyes";
    let request_digest = hex::encode(Sha256::digest(
        format!("{}\0{}\0{}\0{content}", event.repository, target.kind, target.database_id)
            .as_bytes(),
    ));
    transaction.execute(
        "INSERT OR IGNORE INTO github_write_outbox(
           intent_id,event_id,repository,target_kind,target_database_id,operation,content,
           request_digest,lifecycle,next_attempt_at,created_at,updated_at
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'pending',?9,?9,?9)",
        params![
            Uuid::now_v7().to_string(),
            event_id,
            event.repository,
            target.kind,
            target.database_id,
            operation,
            content,
            request_digest,
            now,
        ],
    )?;
    Ok(())
}

fn schedule_event(
    transaction: &rusqlite::Transaction<'_>,
    work_item_node_id: &str,
    event_id: &str,
    policy: SchedulerPolicy,
    urgent: bool,
    now: &str,
) -> Result<(String, String), StoreError> {
    let open = transaction
        .query_row(
            "SELECT batch_id,event_count,lifecycle
             FROM wake_batches
             WHERE work_item_node_id=?1 AND lifecycle IN ('pending','runnable')",
            [work_item_node_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?)),
        )
        .optional()?;
    let deadline = deadline_rfc3339(policy.quiet_seconds)?;
    let (batch_id, old_count, old_lifecycle) = if let Some(open) = open {
        open
    } else {
        let batch_id = Uuid::now_v7().to_string();
        transaction.execute(
            "INSERT INTO wake_batches(
               batch_id,work_item_node_id,event_count,quiet_deadline,urgent,lifecycle,created_at,updated_at
             ) VALUES (?1,?2,0,?3,0,'pending',?4,?4)",
            params![batch_id, work_item_node_id, deadline, now],
        )?;
        (batch_id, 0, "pending".into())
    };
    let inserted = transaction.execute(
        "INSERT OR IGNORE INTO wake_batch_events(batch_id,event_id,ordinal)
         VALUES (?1,?2,(SELECT COUNT(*) FROM wake_batch_events WHERE batch_id=?1))",
        params![batch_id, event_id],
    )?;
    let count = old_count + i64::try_from(inserted).expect("SQLite change count fits i64");
    let threshold = i64::from(policy.event_threshold);
    let lifecycle = if old_lifecycle == "runnable" || urgent || count >= threshold {
        "runnable"
    } else {
        "pending"
    };
    transaction.execute(
        "UPDATE wake_batches SET
           event_count=?2,
           quiet_deadline=CASE WHEN lifecycle='runnable' THEN quiet_deadline ELSE ?3 END,
           urgent=CASE WHEN ?4=1 THEN 1 ELSE urgent END,
           lifecycle=?5,
           updated_at=?6
         WHERE batch_id=?1",
        params![batch_id, count, deadline, i64::from(urgent), lifecycle, now],
    )?;
    Ok((batch_id, lifecycle.into()))
}

fn mention_candidates(database: &Path, limit: usize) -> Result<Vec<MentionCandidate>, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_only(database)?;
    let limit =
        i64::try_from(limit).map_err(|_| StoreError::InvalidData("limit exceeds i64".into()))?;
    let mut statement = connection.prepare(
        "SELECT e.event_id,d.actor_login
         FROM events e JOIN deliveries d ON d.delivery_guid=e.delivery_guid
         WHERE e.mention_candidate=1 AND e.trusted_mention IS NULL AND e.lifecycle='pending'
           AND d.repository_name IS NOT NULL AND d.actor_login IS NOT NULL
         ORDER BY e.observed_at,e.event_id LIMIT ?1",
    )?;
    let rows = statement.query_map([limit], |row| {
        Ok(MentionCandidate { event_id: row.get(0)?, actor_login: row.get(1)? })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
}

fn resolve_mention(
    database: &Path,
    event_id: &str,
    trusted: bool,
    policy: SchedulerPolicy,
) -> Result<(), StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let event = transaction
        .query_row(
            "SELECT work_item_node_id,trusted_mention FROM events WHERE event_id=?1",
            [event_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .optional()?;
    let Some((work_item_node_id, prior)) = event else {
        return Err(StoreError::InvalidData(format!("unknown mention event {event_id}")));
    };
    if prior.is_some() {
        transaction.commit()?;
        return Ok(());
    }
    transaction.execute(
        "UPDATE events SET trusted_mention=?2 WHERE event_id=?1",
        params![event_id, i64::from(trusted)],
    )?;
    if trusted && let Some(work_item_node_id) = work_item_node_id {
        schedule_event(&transaction, &work_item_node_id, event_id, policy, true, &now)?;
    }
    transaction.commit()?;
    Ok(())
}

fn advance_scheduler(database: &Path) -> Result<u64, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let changed = connection.execute(
        "UPDATE wake_batches SET lifecycle='runnable',updated_at=?1
         WHERE lifecycle='pending' AND quiet_deadline<=?1",
        [now_rfc3339()],
    )?;
    Ok(u64::try_from(changed).expect("SQLite change count fits u64"))
}

fn runtime_status(database: &Path) -> Result<RuntimeStoreStatus, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_only(database)?;
    let owner = connection
        .query_row(
            "SELECT scope,generation,owner_id,expires_at FROM owner_leases WHERE scope='runtime'",
            [],
            |row| {
                Ok(LeaseStatus {
                    scope: row.get(0)?,
                    generation: sqlite_i64_to_u64(row.get(1)?, "owner lease generation")?,
                    owner_id: row.get(2)?,
                    expires_at: row.get(3)?,
                })
            },
        )
        .optional()?;
    let batches = load_wake_batches(&connection)?;
    let agent_groups = load_agent_groups(&connection)?;
    let context_resets = load_context_resets(&connection)?;
    Ok(RuntimeStoreStatus {
        owner,
        deliveries: scalar_u64(&connection, "SELECT COUNT(*) FROM deliveries")?,
        duplicate_deliveries: scalar_u64(
            &connection,
            "SELECT COALESCE(SUM(duplicate_count),0) FROM deliveries",
        )?,
        unknown_deliveries: scalar_u64(
            &connection,
            "SELECT COUNT(*) FROM deliveries WHERE known=0",
        )?,
        pending_batches: scalar_u64(
            &connection,
            "SELECT COUNT(*) FROM wake_batches WHERE lifecycle='pending'",
        )?,
        runnable_batches: scalar_u64(
            &connection,
            "SELECT COUNT(*) FROM wake_batches WHERE lifecycle='runnable'",
        )?,
        pending_mentions: scalar_u64(
            &connection,
            "SELECT COUNT(*) FROM events WHERE mention_candidate=1 AND trusted_mention IS NULL AND lifecycle='pending'",
        )?,
        pending_writes: scalar_u64(
            &connection,
            "SELECT COUNT(*) FROM github_write_outbox WHERE lifecycle IN ('pending','sending')",
        )?,
        uncertain_writes: scalar_u64(
            &connection,
            "SELECT COUNT(*) FROM github_write_outbox WHERE lifecycle='uncertain'",
        )?,
        last_reconciliation: connection
            .query_row(
                "SELECT completed_at FROM reconciliation_runs
                 WHERE lifecycle='completed' ORDER BY completed_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?,
        batches,
        agent_groups,
        context_resets,
    })
}

fn load_wake_batches(
    connection: &rusqlite::Connection,
) -> Result<Vec<WakeBatchSummary>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT b.batch_id,r.name_with_owner,w.kind,w.number,w.node_id,b.event_count,
                b.quiet_deadline,b.urgent,b.lifecycle
         FROM wake_batches b
         JOIN work_items w ON w.node_id=b.work_item_node_id
         JOIN repositories r ON r.node_id=w.repository_node_id
         WHERE b.lifecycle IN ('pending','runnable')
         ORDER BY b.created_at,b.batch_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(WakeBatchSummary {
            batch_id: row.get(0)?,
            repository: row.get(1)?,
            work_item_kind: row.get(2)?,
            work_item_number: sqlite_i64_to_u64(row.get(3)?, "work-item number")?,
            work_item_node_id: row.get(4)?,
            event_count: sqlite_i64_to_u64(row.get(5)?, "batch event count")?,
            quiet_deadline: row.get(6)?,
            urgent: row.get::<_, i64>(7)? != 0,
            lifecycle: row.get(8)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
}

fn load_agent_groups(
    connection: &rusqlite::Connection,
) -> Result<Vec<AgentGroupSummary>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT w.kind,w.number,ai.profile_id,a.generation,a.lifecycle,
                ps.provider_session_id,ps.lifecycle,t.provider_turn_id,t.lifecycle,
                (SELECT COUNT(*) FROM turns ft
                 JOIN provider_sessions fps ON fps.session_id=ft.session_id
                 WHERE fps.agent_id=ai.agent_id AND ft.trigger_kind='finalization'),
                (SELECT ft.lifecycle FROM turns ft
                 JOIN provider_sessions fps ON fps.session_id=ft.session_id
                 WHERE fps.agent_id=ai.agent_id AND ft.trigger_kind='finalization'
                 ORDER BY ft.rowid DESC LIMIT 1)
         FROM assignments a
         JOIN work_items w ON w.node_id=a.work_item_node_id
         JOIN agent_instances ai ON ai.assignment_id=a.assignment_id
         LEFT JOIN provider_sessions ps ON ps.agent_id=ai.agent_id
         LEFT JOIN turns t ON t.session_id=ps.session_id
           AND t.lifecycle IN ('starting','running','unknown')
         ORDER BY a.assigned_at,a.assignment_id,ps.started_at",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(AgentGroupSummary {
            work_item_kind: row.get(0)?,
            work_item_number: sqlite_i64_to_u64(row.get(1)?, "agent Work Item number")?,
            profile_id: row.get(2)?,
            assignment_generation: sqlite_i64_to_u64(row.get(3)?, "assignment generation")?,
            assignment_lifecycle: row.get(4)?,
            provider_session_id: row.get(5)?,
            session_lifecycle: row.get(6)?,
            active_turn_id: row.get(7)?,
            turn_lifecycle: row.get(8)?,
            finalization_turns: sqlite_i64_to_u64(row.get(9)?, "finalization turn count")?,
            last_finalization_lifecycle: row.get(10)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
}

fn load_context_resets(connection: &Connection) -> Result<Vec<ContextResetSummary>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT cr.reset_id,r.name_with_owner,w.kind,w.number,ai.profile_id,cr.lifecycle,
                cr.continuation,old.provider_session_id,new.provider_session_id,
                cr.context_revision_before,cr.context_revision_after
         FROM context_resets cr
         JOIN agent_instances ai ON ai.agent_id=cr.agent_id
         JOIN assignments a ON a.assignment_id=ai.assignment_id
         JOIN work_items w ON w.node_id=a.work_item_node_id
         JOIN repositories r ON r.node_id=w.repository_node_id
         JOIN provider_sessions old ON old.session_id=cr.old_session_id
         LEFT JOIN provider_sessions new ON new.session_id=cr.new_session_id
         ORDER BY cr.created_at,cr.reset_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(ContextResetSummary {
            reset_id: row.get(0)?,
            repository: row.get(1)?,
            work_item_kind: row.get(2)?,
            work_item_number: sqlite_i64_to_u64(row.get(3)?, "context reset Work Item number")?,
            profile_id: row.get(4)?,
            lifecycle: row.get(5)?,
            continuation: row.get::<_, i64>(6)? != 0,
            old_provider_session_id: row.get(7)?,
            new_provider_session_id: row.get(8)?,
            context_revision_before: row.get(9)?,
            context_revision_after: row.get(10)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
}

fn acquire_runtime_lease(
    database: &Path,
    scope: &str,
    owner_id: &str,
    ttl_seconds: u64,
) -> Result<RuntimeLease, StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let expires_at = deadline_rfc3339(ttl_seconds)?;
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let current = transaction
        .query_row(
            "SELECT generation,owner_id,expires_at FROM owner_leases WHERE scope=?1",
            [scope],
            |row| {
                Ok((
                    sqlite_i64_to_u64(row.get(0)?, "owner lease generation")?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    if let Some((_, current_owner, current_expiry)) = &current
        && current_owner != owner_id
        && current_expiry > &now
    {
        return Err(StoreError::RuntimeBusy {
            scope: scope.into(),
            expires_at: current_expiry.clone(),
        });
    }
    let generation = current.map_or(1, |(generation, _, _)| generation.saturating_add(1));
    transaction.execute(
        "INSERT INTO owner_leases(scope,generation,owner_id,expires_at) VALUES (?1,?2,?3,?4)
         ON CONFLICT(scope) DO UPDATE SET generation=excluded.generation,
           owner_id=excluded.owner_id,expires_at=excluded.expires_at",
        params![scope, sqlite_u64(generation, "owner lease generation")?, owner_id, expires_at],
    )?;
    transaction.commit()?;
    Ok(RuntimeLease { scope: scope.into(), generation, owner_id: owner_id.into() })
}

fn renew_runtime_lease(
    database: &Path,
    lease: &RuntimeLease,
    ttl_seconds: u64,
) -> Result<RuntimeLease, StoreError> {
    require_current_schema(database)?;
    let expires_at = deadline_rfc3339(ttl_seconds)?;
    let connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let updated = connection.execute(
        "UPDATE owner_leases SET expires_at=?4
         WHERE scope=?1 AND generation=?2 AND owner_id=?3",
        params![
            lease.scope,
            sqlite_u64(lease.generation, "owner lease generation")?,
            lease.owner_id,
            expires_at
        ],
    )?;
    if updated != 1 {
        return Err(StoreError::InvalidData("runtime lease ownership was lost".into()));
    }
    Ok(lease.clone())
}

fn release_runtime_lease(database: &Path, lease: &RuntimeLease) -> Result<(), StoreError> {
    require_current_schema(database)?;
    let connection = open_read_write(database)?;
    configure_connection(&connection)?;
    connection.execute(
        "DELETE FROM owner_leases WHERE scope=?1 AND generation=?2 AND owner_id=?3",
        params![
            lease.scope,
            sqlite_u64(lease.generation, "owner lease generation")?,
            lease.owner_id
        ],
    )?;
    Ok(())
}

fn recover_writes(database: &Path) -> Result<u64, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let changed = connection.execute(
        "UPDATE github_write_outbox SET lifecycle='uncertain',next_attempt_at=?1,updated_at=?1
         WHERE lifecycle='sending'",
        [now_rfc3339()],
    )?;
    Ok(u64::try_from(changed).expect("SQLite change count fits u64"))
}

fn claim_github_write(database: &Path) -> Result<Option<PendingGitHubWrite>, StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let write = transaction
        .query_row(
            "SELECT intent_id,repository,target_kind,target_database_id,operation,content,
                    remote_database_id
             FROM github_write_outbox
             WHERE lifecycle IN ('pending','uncertain') AND next_attempt_at<=?1
             ORDER BY created_at,intent_id LIMIT 1",
            [&now],
            |row| {
                Ok(PendingGitHubWrite {
                    intent_id: row.get(0)?,
                    repository: row.get(1)?,
                    target_kind: row.get(2)?,
                    target_database_id: row.get(3)?,
                    operation: row.get(4)?,
                    content: row.get(5)?,
                    remote_database_id: row.get(6)?,
                })
            },
        )
        .optional()?;
    if let Some(write) = &write {
        transaction.execute(
            "UPDATE github_write_outbox SET lifecycle='sending',attempts=attempts+1,
               last_error=NULL,updated_at=?2 WHERE intent_id=?1",
            params![write.intent_id, now],
        )?;
    }
    transaction.commit()?;
    Ok(write)
}

fn finish_github_write(
    database: &Path,
    intent_id: &str,
    lifecycle: &str,
    remote_database_id: Option<&str>,
    remote_node_id: Option<&str>,
    error: Option<&str>,
) -> Result<(), StoreError> {
    require_current_schema(database)?;
    if !matches!(lifecycle, "applied" | "uncertain" | "rejected" | "conflict" | "ambiguous") {
        return Err(StoreError::InvalidData(format!("invalid write terminal {lifecycle}")));
    }
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let (attempts, operation, repository, target_kind, target_database_id, content) = transaction
        .query_row(
            "SELECT attempts,operation,repository,target_kind,target_database_id,content
             FROM github_write_outbox WHERE intent_id=?1 AND lifecycle='sending'",
            [intent_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            StoreError::InvalidData(format!("write intent {intent_id} is not sending"))
        })?;
    let retry_exponent = u32::try_from(attempts).unwrap_or(6).min(6);
    let retry_seconds = 2_u64.pow(retry_exponent).min(60);
    let next_attempt_at =
        if lifecycle == "uncertain" { deadline_rfc3339(retry_seconds)? } else { now_rfc3339() };
    let updated = transaction.execute(
        "UPDATE github_write_outbox SET lifecycle=?2,remote_database_id=?3,last_error=?4,
           next_attempt_at=?5,updated_at=?6 WHERE intent_id=?1 AND lifecycle='sending'",
        params![intent_id, lifecycle, remote_database_id, error, next_attempt_at, now_rfc3339()],
    )?;
    if updated != 1 {
        return Err(StoreError::InvalidData(format!("write intent {intent_id} is not sending")));
    }
    if lifecycle == "applied" && operation == "reaction_add" && content == "rocket" {
        let terminal_exists = transaction
            .query_row(
                "SELECT 1 FROM github_write_outbox
                 WHERE repository=?1 AND target_kind=?2 AND target_database_id=?3
                   AND operation='reaction_add' AND content IN ('+1','confused')
                   AND lifecycle NOT IN ('rejected','conflict','ambiguous','superseded') LIMIT 1",
                params![repository, target_kind, target_database_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if terminal_exists {
            enqueue_rocket_removal(
                &transaction,
                &repository,
                &target_kind,
                &target_database_id,
                &now_rfc3339(),
            )?;
        }
    }
    if matches!(operation.as_str(), "comment_create" | "comment_update") {
        settle_operational_status(
            &transaction,
            &repository,
            &target_kind,
            &target_database_id,
            &content,
            lifecycle,
            remote_database_id,
            remote_node_id,
            &now_rfc3339(),
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn tracked_work_items(database: &Path) -> Result<Vec<TrackedWorkItem>, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_only(database)?;
    let mut statement = connection.prepare(
        "SELECT w.node_id,r.name_with_owner,w.kind,w.number
         FROM work_items w JOIN repositories r ON r.node_id=w.repository_node_id
         ORDER BY r.name_with_owner,w.kind,w.number",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(TrackedWorkItem {
            node_id: row.get(0)?,
            repository: row.get(1)?,
            kind: row.get(2)?,
            number: sqlite_i64_to_u64(row.get(3)?, "work-item number")?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
}

fn canonical_objects(
    database: &Path,
    work_item_node_id: &str,
) -> Result<Vec<CanonicalObjectState>, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_only(database)?;
    let mut statement = connection.prepare(
        "SELECT node_id,database_id,object_kind,version,digest,lifecycle,
                author_node_id,author_login
         FROM canonical_objects WHERE work_item_node_id=?1
         ORDER BY object_kind,node_id",
    )?;
    let rows = statement.query_map([work_item_node_id], |row| {
        Ok(CanonicalObjectState {
            node_id: row.get(0)?,
            database_id: row.get(1)?,
            object_kind: row.get(2)?,
            version: row.get(3)?,
            digest: row.get(4)?,
            lifecycle: row.get(5)?,
            author_node_id: row.get(6)?,
            author_login: row.get(7)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
}

fn begin_reconciliation(
    database: &Path,
    repository_node_id: &str,
) -> Result<ReconciliationRun, StoreError> {
    require_current_schema(database)?;
    let run = ReconciliationRun {
        run_id: Uuid::now_v7().to_string(),
        repository_node_id: repository_node_id.into(),
    };
    let connection = open_read_write(database)?;
    configure_connection(&connection)?;
    connection.execute(
        "INSERT INTO reconciliation_runs(run_id,repository_node_id,lifecycle,started_at)
         VALUES (?1,?2,'running',?3)",
        params![run.run_id, run.repository_node_id, now_rfc3339()],
    )?;
    Ok(run)
}

fn finish_reconciliation(
    database: &Path,
    run: &ReconciliationRun,
    lifecycle: &str,
    work_item_count: usize,
    change_count: usize,
    error: Option<&str>,
) -> Result<(), StoreError> {
    require_current_schema(database)?;
    if !matches!(lifecycle, "completed" | "failed") {
        return Err(StoreError::InvalidData(format!(
            "invalid reconciliation terminal {lifecycle}"
        )));
    }
    let connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let updated = connection.execute(
        "UPDATE reconciliation_runs SET lifecycle=?2,completed_at=?3,work_item_count=?4,
           change_count=?5,error=?6 WHERE run_id=?1 AND lifecycle='running'",
        params![
            run.run_id,
            lifecycle,
            now_rfc3339(),
            sqlite_usize(work_item_count, "reconciliation work-item count")?,
            sqlite_usize(change_count, "reconciliation change count")?,
            error,
        ],
    )?;
    if updated == 1 {
        Ok(())
    } else {
        Err(StoreError::InvalidData(format!("reconciliation run {} is not active", run.run_id)))
    }
}

fn register_profile(database: &Path, profile: &ProfileRecord) -> Result<(), StoreError> {
    require_current_schema(database)?;
    if profile.effective_digest.len() != 64 {
        return Err(StoreError::InvalidData("Profile digest is not SHA-256".into()));
    }
    let connection = open_read_write(database)?;
    configure_connection(&connection)?;
    connection.execute(
        "INSERT INTO profiles(profile_id,revision,effective_digest,provider_kind,tags)
         VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(profile_id,revision) DO UPDATE SET
           effective_digest=excluded.effective_digest,
           provider_kind=excluded.provider_kind,
           tags=excluded.tags",
        params![
            profile.profile_id,
            sqlite_u64(profile.revision, "Profile revision")?,
            profile.effective_digest,
            profile.provider_kind,
            profile.tags,
        ],
    )?;
    Ok(())
}

fn assignment_candidates(
    database: &Path,
    limit: usize,
) -> Result<Vec<AssignmentCandidate>, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_only(database)?;
    let limit = i64::try_from(limit)
        .map_err(|_| StoreError::InvalidData("assignment candidate limit exceeds i64".into()))?;
    let mut statement = connection.prepare(
        "SELECT e.event_id,
                CASE WHEN e.trusted_mention=1 THEN 'trusted_mention' ELSE d.action END,
                r.name_with_owner,w.number
         FROM events e
         JOIN deliveries d ON d.delivery_guid=e.delivery_guid
         JOIN work_items w ON w.node_id=e.work_item_node_id
         JOIN repositories r ON r.node_id=w.repository_node_id
         WHERE e.lifecycle='pending' AND w.kind='issue'
           AND ((e.classification='lifecycle' AND d.event_name='issues'
                 AND d.action IN ('assigned','unassigned'))
                OR e.trusted_mention=1)
         ORDER BY e.observed_at,e.event_id LIMIT ?1",
    )?;
    let rows = statement.query_map([limit], |row| {
        Ok(AssignmentCandidate {
            event_id: row.get(0)?,
            action: row.get(1)?,
            repository: row.get(2)?,
            number: sqlite_i64_to_u64(row.get(3)?, "assignment Issue number")?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
}

fn issue_lifecycle_candidates(
    database: &Path,
    limit: usize,
) -> Result<Vec<IssueLifecycleCandidate>, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_only(database)?;
    let limit = i64::try_from(limit)
        .map_err(|_| StoreError::InvalidData("Issue lifecycle limit exceeds i64".into()))?;
    let mut statement = connection.prepare(
        "SELECT e.event_id,d.action,r.name_with_owner,w.number
         FROM events e
         JOIN deliveries d ON d.delivery_guid=e.delivery_guid
         JOIN work_items w ON w.node_id=e.work_item_node_id
         JOIN repositories r ON r.node_id=w.repository_node_id
         WHERE e.lifecycle='pending' AND e.classification='lifecycle'
           AND d.event_name='issues' AND d.action IN ('closed','reopened')
         ORDER BY e.observed_at,e.event_id LIMIT ?1",
    )?;
    let rows = statement.query_map([limit], |row| {
        Ok(IssueLifecycleCandidate {
            event_id: row.get(0)?,
            action: row.get(1)?,
            repository: row.get(2)?,
            number: sqlite_i64_to_u64(row.get(3)?, "lifecycle Issue number")?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
}

fn prepare_issue_finalization(database: &Path, event_id: &str) -> Result<bool, StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let candidate = transaction
        .query_row(
            "SELECT e.work_item_node_id,w.state,d.action
             FROM events e
             JOIN work_items w ON w.node_id=e.work_item_node_id
             JOIN deliveries d ON d.delivery_guid=e.delivery_guid
             WHERE e.event_id=?1 AND e.lifecycle='pending' AND w.kind='issue'",
            [event_id],
            |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            },
        )
        .optional()?;
    let Some((work_item_node_id, state, action)) = candidate else {
        transaction.commit()?;
        return Ok(false);
    };
    if action != "closed" || !state.eq_ignore_ascii_case("closed") {
        transaction.execute(
            "UPDATE events SET lifecycle='superseded' WHERE event_id=?1 AND lifecycle='pending'",
            [event_id],
        )?;
        transaction.commit()?;
        return Ok(false);
    }
    let selected = transaction
        .query_row(
            "SELECT a.assignment_id,ai.agent_id,ps.session_id
             FROM assignments a
             JOIN agent_instances ai ON ai.assignment_id=a.assignment_id AND ai.lifecycle='idle'
             JOIN provider_sessions ps ON ps.agent_id=ai.agent_id AND ps.lifecycle='idle'
             WHERE a.work_item_node_id=?1 AND a.lifecycle='active'
             ORDER BY ps.started_at DESC LIMIT 1",
            [&work_item_node_id],
            |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            },
        )
        .optional()?;
    let Some((assignment_id, agent_id, _session_id)) = selected else {
        let has_group = transaction
            .query_row(
                "SELECT 1 FROM assignments
                 WHERE work_item_node_id=?1
                   AND lifecycle IN ('materializing','active','finalizing','sleeping') LIMIT 1",
                [&work_item_node_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !has_group {
            transaction.execute(
                "UPDATE events SET lifecycle='consumed' WHERE event_id=?1 AND lifecycle='pending'",
                [event_id],
            )?;
        }
        transaction.commit()?;
        return Ok(false);
    };
    transaction.execute(
        "UPDATE wake_batches SET lifecycle='consumed',updated_at=?2
         WHERE work_item_node_id=?1 AND lifecycle IN ('pending','runnable')",
        params![work_item_node_id, now],
    )?;
    schedule_event(
        &transaction,
        &work_item_node_id,
        event_id,
        SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 },
        false,
        &now,
    )?;
    let assignment = transaction.execute(
        "UPDATE assignments SET lifecycle='finalizing'
         WHERE assignment_id=?1 AND lifecycle='active'",
        [&assignment_id],
    )?;
    let agent = transaction.execute(
        "UPDATE agent_instances SET lifecycle='finalizing'
         WHERE agent_id=?1 AND lifecycle='idle'",
        [&agent_id],
    )?;
    if assignment != 1 || agent != 1 {
        return Err(StoreError::InvalidData(format!(
            "Issue lifecycle event {event_id} lost its idle Agent Group"
        )));
    }
    transaction.commit()?;
    Ok(true)
}

#[allow(clippy::too_many_lines)]
fn begin_issue_reactivation(
    database: &Path,
    event_id: &str,
) -> Result<Option<AgentMaterialization>, StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let candidate = transaction
        .query_row(
            "SELECT e.work_item_node_id,w.state,d.action
             FROM events e
             JOIN work_items w ON w.node_id=e.work_item_node_id
             JOIN deliveries d ON d.delivery_guid=e.delivery_guid
             WHERE e.event_id=?1 AND e.lifecycle='pending' AND w.kind='issue'",
            [event_id],
            |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            },
        )
        .optional()?;
    let Some((work_item_node_id, state, action)) = candidate else {
        transaction.commit()?;
        return Ok(None);
    };
    if action != "reopened" || !state.eq_ignore_ascii_case("open") {
        transaction.execute(
            "UPDATE events SET lifecycle='superseded' WHERE event_id=?1 AND lifecycle='pending'",
            [event_id],
        )?;
        transaction.commit()?;
        return Ok(None);
    }
    let selected = transaction
        .query_row(
            "SELECT a.assignment_id,ai.agent_id,a.generation,ai.profile_id,ai.profile_revision
             FROM assignments a
             JOIN agent_instances ai ON ai.assignment_id=a.assignment_id
             WHERE a.work_item_node_id=?1 AND a.lifecycle='sleeping' AND ai.lifecycle='sleeping'
             ORDER BY a.generation DESC LIMIT 1",
            [&work_item_node_id],
            |row| {
                Ok(AgentMaterialization {
                    assignment_id: row.get(0)?,
                    agent_id: row.get(1)?,
                    work_item_node_id: work_item_node_id.clone(),
                    generation: sqlite_i64_to_u64(row.get(2)?, "reactivation generation")?,
                    profile_id: row.get(3)?,
                    profile_revision: sqlite_i64_to_u64(
                        row.get(4)?,
                        "reactivation Profile revision",
                    )?,
                })
            },
        )
        .optional()?;
    let Some(materialization) = selected else {
        let has_group = transaction
            .query_row(
                "SELECT 1 FROM assignments WHERE work_item_node_id=?1 LIMIT 1",
                [&work_item_node_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !has_group
            || transaction
                .query_row(
                    "SELECT 1 FROM assignments
                     WHERE work_item_node_id=?1 AND lifecycle='active' LIMIT 1",
                    [&work_item_node_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some()
        {
            transaction.execute(
                "UPDATE events SET lifecycle='consumed' WHERE event_id=?1 AND lifecycle='pending'",
                [event_id],
            )?;
        }
        transaction.commit()?;
        return Ok(None);
    };
    transaction.execute(
        "UPDATE wake_batches SET lifecycle='consumed',updated_at=?2
         WHERE work_item_node_id=?1 AND lifecycle IN ('pending','runnable')",
        params![work_item_node_id, now],
    )?;
    transaction.execute(
        "UPDATE events SET lifecycle='consumed'
         WHERE work_item_node_id=?1 AND lifecycle='pending'
           AND classification='hard_invalidation' AND origin!='agent'",
        [&work_item_node_id],
    )?;
    transaction.execute(
        "UPDATE events SET lifecycle='materializing' WHERE event_id=?1 AND lifecycle='pending'",
        [event_id],
    )?;
    transaction.execute(
        "UPDATE assignments SET lifecycle='materializing' WHERE assignment_id=?1",
        [&materialization.assignment_id],
    )?;
    transaction.execute(
        "UPDATE agent_instances SET lifecycle='materializing' WHERE agent_id=?1",
        [&materialization.agent_id],
    )?;
    transaction.commit()?;
    Ok(Some(materialization))
}

#[allow(clippy::too_many_arguments)]
fn complete_issue_reactivation(
    database: &Path,
    event_id: &str,
    materialization: &AgentMaterialization,
    provider_session_id: &str,
    context_revision: &str,
    instruction_revision: &str,
    policy: SchedulerPolicy,
) -> Result<(), StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let event_state = transaction
        .query_row("SELECT lifecycle FROM events WHERE event_id=?1", [event_id], |row| {
            row.get::<_, String>(0)
        })
        .optional()?;
    if event_state.as_deref() != Some("materializing") {
        return Err(StoreError::InvalidData(format!(
            "Issue reopen event {event_id} is not materializing"
        )));
    }
    let session_id = Uuid::now_v7().to_string();
    transaction.execute(
        "UPDATE provider_sessions SET lifecycle='replaced'
         WHERE agent_id=?1 AND lifecycle='sleeping'",
        [&materialization.agent_id],
    )?;
    transaction.execute(
        "INSERT INTO provider_sessions(
           session_id,agent_id,provider_kind,provider_session_id,context_revision,
           instruction_revision,lifecycle,started_at
         ) VALUES (?1,?2,'codex',?3,?4,?5,'idle',?6)",
        params![
            session_id,
            materialization.agent_id,
            provider_session_id,
            context_revision,
            instruction_revision,
            now,
        ],
    )?;
    transaction.execute(
        "UPDATE assignments SET lifecycle='active',retired_at=NULL
         WHERE assignment_id=?1 AND lifecycle='materializing'",
        [&materialization.assignment_id],
    )?;
    transaction.execute(
        "UPDATE agent_instances SET lifecycle='idle'
         WHERE agent_id=?1 AND lifecycle='materializing'",
        [&materialization.agent_id],
    )?;
    transaction.execute(
        "UPDATE work_items SET context_revision=?2,observed_at=?3 WHERE node_id=?1",
        params![materialization.work_item_node_id, context_revision, now],
    )?;
    transaction.execute(
        "UPDATE events SET lifecycle='pending' WHERE event_id=?1 AND lifecycle='materializing'",
        [event_id],
    )?;
    schedule_event(
        &transaction,
        &materialization.work_item_node_id,
        event_id,
        policy,
        false,
        &now,
    )?;
    transaction.commit()?;
    Ok(())
}

fn fail_issue_reactivation(
    database: &Path,
    event_id: &str,
    assignment_id: &str,
    _error: &str,
) -> Result<(), StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    transaction.execute(
        "UPDATE events SET lifecycle='blocked' WHERE event_id=?1 AND lifecycle='materializing'",
        [event_id],
    )?;
    transaction.execute(
        "UPDATE assignments SET lifecycle='blocked',retired_at=?2
         WHERE assignment_id=?1 AND lifecycle='materializing'",
        params![assignment_id, now],
    )?;
    transaction.execute(
        "UPDATE agent_instances SET lifecycle='blocked'
         WHERE assignment_id=?1 AND lifecycle='materializing'",
        [assignment_id],
    )?;
    transaction.commit()?;
    Ok(())
}

fn has_lifecycle_observation(
    database: &Path,
    work_item_node_id: &str,
    action: &str,
    object_version: &str,
) -> Result<bool, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_only(database)?;
    Ok(connection
        .query_row(
            "SELECT 1
             FROM events e JOIN deliveries d ON d.delivery_guid=e.delivery_guid
             WHERE e.work_item_node_id=?1 AND e.object_version=?2
               AND e.classification='lifecycle' AND d.action=?3 LIMIT 1",
            params![work_item_node_id, object_version, action],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn begin_issue_assignment(
    database: &Path,
    event_id: &str,
    profile: &ProfileRecord,
    context_revision: &str,
    preserve_wake_batch: bool,
) -> Result<Option<AgentMaterialization>, StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let work_item_node_id = transaction
        .query_row(
            "SELECT work_item_node_id FROM events WHERE event_id=?1 AND lifecycle='pending'",
            [event_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| {
            StoreError::InvalidData(format!("assignment event {event_id} is not pending"))
        })?;
    transaction.execute(
        "INSERT INTO profiles(profile_id,revision,effective_digest,provider_kind,tags)
         VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(profile_id,revision) DO UPDATE SET
           effective_digest=excluded.effective_digest,
           provider_kind=excluded.provider_kind,
           tags=excluded.tags",
        params![
            profile.profile_id,
            sqlite_u64(profile.revision, "Profile revision")?,
            profile.effective_digest,
            profile.provider_kind,
            profile.tags,
        ],
    )?;
    if !preserve_wake_batch {
        transaction.execute(
            "UPDATE wake_batches SET lifecycle='consumed',updated_at=?2
             WHERE work_item_node_id=?1 AND lifecycle IN ('pending','runnable')",
            params![work_item_node_id, now],
        )?;
    }
    transaction.execute("UPDATE events SET lifecycle='consumed' WHERE event_id=?1", [event_id])?;
    let active = transaction
        .query_row(
            "SELECT 1 FROM assignments
             WHERE work_item_node_id=?1 AND lifecycle IN ('materializing','active')",
            [&work_item_node_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if active {
        transaction.commit()?;
        return Ok(None);
    }
    let generation = transaction.query_row(
        "SELECT COALESCE(MAX(generation),0)+1 FROM assignments WHERE work_item_node_id=?1",
        [&work_item_node_id],
        |row| sqlite_i64_to_u64(row.get(0)?, "assignment generation"),
    )?;
    let materialization = AgentMaterialization {
        assignment_id: Uuid::now_v7().to_string(),
        agent_id: Uuid::now_v7().to_string(),
        work_item_node_id: work_item_node_id.clone(),
        generation,
        profile_id: profile.profile_id.clone(),
        profile_revision: profile.revision,
    };
    transaction.execute(
        "INSERT INTO assignments(
           assignment_id,work_item_node_id,generation,lifecycle,assigned_at
         ) VALUES (?1,?2,?3,'materializing',?4)",
        params![
            materialization.assignment_id,
            materialization.work_item_node_id,
            sqlite_u64(materialization.generation, "assignment generation")?,
            now,
        ],
    )?;
    transaction.execute(
        "INSERT INTO agent_instances(
           agent_id,assignment_id,profile_id,profile_revision,role,lifecycle
         ) VALUES (?1,?2,?3,?4,'issue_agent','materializing')",
        params![
            materialization.agent_id,
            materialization.assignment_id,
            materialization.profile_id,
            sqlite_u64(materialization.profile_revision, "Profile revision")?,
        ],
    )?;
    transaction.execute(
        "UPDATE work_items SET context_revision=?2,observed_at=?3 WHERE node_id=?1",
        params![work_item_node_id, context_revision, now],
    )?;
    transaction.commit()?;
    Ok(Some(materialization))
}

fn ignore_assignment_event(database: &Path, event_id: &str) -> Result<(), StoreError> {
    require_current_schema(database)?;
    let connection = open_read_write(database)?;
    configure_connection(&connection)?;
    connection.execute(
        "UPDATE events SET lifecycle='consumed' WHERE event_id=?1 AND lifecycle='pending'",
        [event_id],
    )?;
    Ok(())
}

fn complete_issue_assignment(
    database: &Path,
    materialization: &AgentMaterialization,
    provider_session_id: &str,
    context_revision: &str,
    instruction_revision: &str,
) -> Result<(), StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let session_id = Uuid::now_v7().to_string();
    transaction.execute(
        "INSERT INTO provider_sessions(
           session_id,agent_id,provider_kind,provider_session_id,context_revision,
           instruction_revision,lifecycle,started_at
         ) VALUES (?1,?2,'codex',?3,?4,?5,'idle',?6)",
        params![
            session_id,
            materialization.agent_id,
            provider_session_id,
            context_revision,
            instruction_revision,
            now,
        ],
    )?;
    let assignment = transaction.execute(
        "UPDATE assignments SET lifecycle='active'
         WHERE assignment_id=?1 AND lifecycle='materializing'",
        [&materialization.assignment_id],
    )?;
    let agent = transaction.execute(
        "UPDATE agent_instances SET lifecycle='idle'
         WHERE agent_id=?1 AND lifecycle='materializing'",
        [&materialization.agent_id],
    )?;
    if assignment != 1 || agent != 1 {
        return Err(StoreError::InvalidData(format!(
            "assignment {} is no longer materializing",
            materialization.assignment_id
        )));
    }
    transaction.commit()?;
    Ok(())
}

fn fail_issue_assignment(
    database: &Path,
    assignment_id: &str,
    _error: &str,
) -> Result<(), StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    transaction.execute(
        "UPDATE assignments SET lifecycle='blocked',retired_at=?2
         WHERE assignment_id=?1 AND lifecycle='materializing'",
        params![assignment_id, now],
    )?;
    transaction.execute(
        "UPDATE agent_instances SET lifecycle='blocked'
         WHERE assignment_id=?1 AND lifecycle='materializing'",
        [assignment_id],
    )?;
    transaction.commit()?;
    Ok(())
}

fn begin_context_reset(
    database: &Path,
    active_turn_id: Option<&str>,
) -> Result<Option<ContextResetClaim>, StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let work_item_node_id = context_reset_work_item(&transaction, active_turn_id)?;
    let Some(work_item_node_id) = work_item_node_id else {
        transaction.commit()?;
        return Ok(None);
    };
    let (agent_id, old_session_id, prior_revision, selected_turn_id) = transaction.query_row(
        "SELECT ai.agent_id,ps.session_id,ps.context_revision,t.turn_id
         FROM assignments a
         JOIN agent_instances ai ON ai.assignment_id=a.assignment_id
         JOIN provider_sessions ps ON ps.agent_id=ai.agent_id
         LEFT JOIN turns t ON t.session_id=ps.session_id AND t.lifecycle='running'
         WHERE a.work_item_node_id=?1 AND a.lifecycle='active'
           AND ps.lifecycle IN ('idle','running')
         ORDER BY ps.started_at DESC LIMIT 1",
        [&work_item_node_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        },
    )?;
    if active_turn_id != selected_turn_id.as_deref() {
        return Err(StoreError::InvalidData(
            "context reset active-turn precondition changed".into(),
        ));
    }
    let events = context_reset_events(&transaction, &work_item_node_id)?;
    if events.is_empty() {
        transaction.commit()?;
        return Ok(None);
    }
    let reset_id = Uuid::now_v7().to_string();
    let continuation = selected_turn_id.is_some();
    let reset_lifecycle = if continuation { "interrupting" } else { "materializing" };
    transaction.execute(
        "INSERT INTO context_resets(
           reset_id,agent_id,old_session_id,active_turn_id,context_revision_before,
           continuation,lifecycle,created_at,updated_at
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)",
        params![
            reset_id,
            agent_id,
            old_session_id,
            selected_turn_id,
            prior_revision,
            i64::from(continuation),
            reset_lifecycle,
            now,
        ],
    )?;
    for (ordinal, event_id) in events.iter().enumerate() {
        transaction.execute(
            "INSERT INTO context_reset_events(reset_id,event_id,ordinal) VALUES (?1,?2,?3)",
            params![reset_id, event_id, sqlite_usize(ordinal, "context reset event ordinal")?],
        )?;
        transaction.execute(
            "UPDATE events SET lifecycle='resetting' WHERE event_id=?1 AND lifecycle='pending'",
            [event_id],
        )?;
    }
    transaction.execute(
        "UPDATE agent_instances SET lifecycle='reset_pending' WHERE agent_id=?1",
        [&agent_id],
    )?;
    transaction.execute(
        "UPDATE provider_sessions SET lifecycle='reset_pending' WHERE session_id=?1",
        [&old_session_id],
    )?;
    let claim = load_context_reset_claim(&transaction, &reset_id)?;
    transaction.commit()?;
    Ok(Some(claim))
}

fn context_reset_work_item(
    transaction: &rusqlite::Transaction<'_>,
    active_turn_id: Option<&str>,
) -> Result<Option<String>, StoreError> {
    if let Some(turn_id) = active_turn_id {
        return transaction
            .query_row(
                "SELECT w.node_id
                 FROM turns t
                 JOIN provider_sessions ps ON ps.session_id=t.session_id
                 JOIN agent_instances ai ON ai.agent_id=ps.agent_id
                 JOIN assignments a ON a.assignment_id=ai.assignment_id
                 JOIN work_items w ON w.node_id=a.work_item_node_id
                 WHERE t.turn_id=?1 AND t.lifecycle='running' AND ps.lifecycle='running'
                   AND a.lifecycle='active' AND w.kind='issue'
                   AND NOT EXISTS (
                     SELECT 1 FROM context_resets cr
                     WHERE cr.agent_id=ai.agent_id
                       AND cr.lifecycle IN ('interrupting','materializing')
                   )
                   AND EXISTS (
                     SELECT 1 FROM events e
                     WHERE e.work_item_node_id=w.node_id AND e.lifecycle='pending'
                       AND e.classification='hard_invalidation' AND e.origin!='agent'
                       AND (e.mention_candidate=0 OR e.trusted_mention=0)
                   )",
                [turn_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from);
    }
    transaction
        .query_row(
            "SELECT w.node_id
             FROM events e
             JOIN work_items w ON w.node_id=e.work_item_node_id
             JOIN assignments a ON a.work_item_node_id=w.node_id AND a.lifecycle='active'
             JOIN agent_instances ai ON ai.assignment_id=a.assignment_id
             JOIN provider_sessions ps ON ps.agent_id=ai.agent_id AND ps.lifecycle='idle'
             WHERE e.lifecycle='pending' AND e.classification='hard_invalidation'
               AND e.origin!='agent' AND w.kind='issue'
               AND (e.mention_candidate=0 OR e.trusted_mention=0)
               AND NOT EXISTS (
                 SELECT 1 FROM context_resets cr
                 WHERE cr.agent_id=ai.agent_id
                   AND cr.lifecycle IN ('interrupting','materializing')
               )
             ORDER BY e.observed_at,e.event_id LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(StoreError::from)
}

fn context_reset_events(
    transaction: &rusqlite::Transaction<'_>,
    work_item_node_id: &str,
) -> Result<Vec<String>, StoreError> {
    let mut statement = transaction.prepare(
        "SELECT event_id FROM events
         WHERE work_item_node_id=?1 AND lifecycle='pending'
           AND classification='hard_invalidation' AND origin!='agent'
           AND (mention_candidate=0 OR trusted_mention=0)
         ORDER BY observed_at,event_id",
    )?;
    let rows = statement.query_map([work_item_node_id], |row| row.get::<_, String>(0))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
}

fn ready_context_reset(database: &Path) -> Result<Option<ContextResetClaim>, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_only(database)?;
    let reset_id = connection
        .query_row(
            "SELECT reset_id FROM context_resets
             WHERE lifecycle='materializing' ORDER BY created_at,reset_id LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    reset_id.map(|reset_id| load_context_reset_claim(&connection, &reset_id)).transpose()
}

fn load_context_reset_claim(
    connection: &Connection,
    reset_id: &str,
) -> Result<ContextResetClaim, StoreError> {
    let mut claim = connection.query_row(
        "SELECT cr.reset_id,r.name_with_owner,w.number,ai.profile_id,
                ps.provider_session_id,cr.active_turn_id,t.provider_turn_id,cr.continuation
         FROM context_resets cr
         JOIN agent_instances ai ON ai.agent_id=cr.agent_id
         JOIN assignments a ON a.assignment_id=ai.assignment_id
         JOIN work_items w ON w.node_id=a.work_item_node_id
         JOIN repositories r ON r.node_id=w.repository_node_id
         JOIN provider_sessions ps ON ps.session_id=cr.old_session_id
         LEFT JOIN turns t ON t.turn_id=cr.active_turn_id
         WHERE cr.reset_id=?1",
        [reset_id],
        |row| {
            Ok(ContextResetClaim {
                reset_id: row.get(0)?,
                repository: row.get(1)?,
                number: sqlite_i64_to_u64(row.get(2)?, "context reset Issue number")?,
                profile_id: row.get(3)?,
                old_provider_session_id: row.get(4)?,
                active_turn_id: row.get(5)?,
                provider_turn_id: row.get(6)?,
                references: Vec::new(),
                continuation: row.get::<_, i64>(7)? != 0,
            })
        },
    )?;
    let mut statement = connection.prepare(
        "SELECT e.reference FROM context_reset_events cre
         JOIN events e ON e.event_id=cre.event_id
         WHERE cre.reset_id=?1 ORDER BY cre.ordinal",
    )?;
    claim.references = statement
        .query_map([reset_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(claim)
}

fn mark_context_reset_turn_terminal(
    database: &Path,
    reset_id: &str,
    turn_id: &str,
    lifecycle: &str,
) -> Result<(), StoreError> {
    require_current_schema(database)?;
    if !matches!(lifecycle, "completed" | "interrupted" | "failed") {
        return Err(StoreError::InvalidData(format!("invalid reset turn terminal {lifecycle}")));
    }
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let reset_matches = transaction
        .query_row(
            "SELECT 1 FROM context_resets
             WHERE reset_id=?1 AND active_turn_id=?2 AND lifecycle='interrupting'",
            params![reset_id, turn_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !reset_matches {
        return Err(StoreError::InvalidData(format!(
            "context reset {reset_id} is not interrupting turn {turn_id}"
        )));
    }
    let updated = transaction.execute(
        "UPDATE turns SET lifecycle=?2,ended_at=?3
         WHERE turn_id=?1 AND lifecycle IN ('starting','running')",
        params![turn_id, lifecycle, now],
    )?;
    if updated != 1 {
        return Err(StoreError::InvalidData(format!("turn {turn_id} is not active")));
    }
    remove_turn_rocket(&transaction, turn_id, &now)?;
    transaction.execute(
        "UPDATE context_resets SET lifecycle='materializing',updated_at=?2
         WHERE reset_id=?1 AND lifecycle='interrupting'",
        params![reset_id, now],
    )?;
    transaction.commit()?;
    Ok(())
}

fn complete_context_reset(
    database: &Path,
    reset_id: &str,
    provider_session_id: &str,
    context_revision: &str,
    instruction_revision: &str,
) -> Result<(), StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let (agent_id, old_session_id, work_item_node_id, continuation) = transaction
        .query_row(
            "SELECT cr.agent_id,cr.old_session_id,a.work_item_node_id,cr.continuation
             FROM context_resets cr
             JOIN agent_instances ai ON ai.agent_id=cr.agent_id
             JOIN assignments a ON a.assignment_id=ai.assignment_id
             WHERE cr.reset_id=?1 AND cr.lifecycle='materializing'",
            [reset_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)? != 0,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            StoreError::InvalidData(format!("context reset {reset_id} is not materializing"))
        })?;
    let new_session_id = Uuid::now_v7().to_string();
    transaction.execute(
        "INSERT INTO provider_sessions(
           session_id,agent_id,provider_kind,provider_session_id,context_revision,
           instruction_revision,lifecycle,started_at
         ) VALUES (?1,?2,'codex',?3,?4,?5,'idle',?6)",
        params![
            new_session_id,
            agent_id,
            provider_session_id,
            context_revision,
            instruction_revision,
            now,
        ],
    )?;
    transaction.execute(
        "UPDATE provider_sessions SET lifecycle='replaced' WHERE session_id=?1",
        [&old_session_id],
    )?;
    transaction
        .execute("UPDATE agent_instances SET lifecycle='idle' WHERE agent_id=?1", [&agent_id])?;
    transaction.execute(
        "UPDATE work_items SET context_revision=?2,observed_at=?3 WHERE node_id=?1",
        params![work_item_node_id, context_revision, now],
    )?;
    transaction.execute(
        "UPDATE context_resets SET new_session_id=?2,context_revision_after=?3,
           lifecycle='applied',updated_at=?4 WHERE reset_id=?1",
        params![reset_id, new_session_id, context_revision, now],
    )?;
    let event_ids = {
        let mut statement = transaction.prepare(
            "SELECT event_id FROM context_reset_events WHERE reset_id=?1 ORDER BY ordinal",
        )?;
        statement
            .query_map([reset_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    if continuation {
        let policy = SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 };
        for event_id in &event_ids {
            schedule_event(&transaction, &work_item_node_id, event_id, policy, true, &now)?;
        }
    }
    for event_id in event_ids {
        transaction.execute(
            "UPDATE events SET lifecycle='consumed' WHERE event_id=?1 AND lifecycle='resetting'",
            [event_id],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn fail_context_reset(database: &Path, reset_id: &str, error: &str) -> Result<(), StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let agent_id = transaction
        .query_row(
            "SELECT agent_id FROM context_resets
             WHERE reset_id=?1 AND lifecycle IN ('interrupting','materializing')",
            [reset_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| {
            StoreError::InvalidData(format!("context reset {reset_id} is not active"))
        })?;
    transaction.execute(
        "UPDATE context_resets SET lifecycle='blocked',error=?2,updated_at=?3 WHERE reset_id=?1",
        params![reset_id, error, now],
    )?;
    transaction
        .execute("UPDATE agent_instances SET lifecycle='blocked' WHERE agent_id=?1", [&agent_id])?;
    transaction.execute(
        "UPDATE provider_sessions SET lifecycle='blocked'
         WHERE agent_id=?1 AND lifecycle='reset_pending'",
        [&agent_id],
    )?;
    transaction.execute(
        "UPDATE events SET lifecycle='blocked' WHERE event_id IN (
           SELECT event_id FROM context_reset_events WHERE reset_id=?1
         ) AND lifecycle='resetting'",
        [reset_id],
    )?;
    transaction.commit()?;
    Ok(())
}

fn remove_turn_rocket(
    transaction: &rusqlite::Transaction<'_>,
    turn_id: &str,
    now: &str,
) -> Result<(), StoreError> {
    let target = transaction
        .query_row(
            "SELECT o.repository,o.target_kind,o.target_database_id
             FROM turns t
             JOIN wake_batch_events be ON be.batch_id=t.batch_id
             JOIN events e ON e.event_id=be.event_id AND e.trusted_mention=1
             JOIN github_write_outbox o ON o.event_id=e.event_id
             WHERE t.turn_id=?1 LIMIT 1",
            [turn_id],
            |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            },
        )
        .optional()?;
    if let Some((repository, target_kind, target_database_id)) = target {
        enqueue_rocket_removal(transaction, &repository, &target_kind, &target_database_id, now)?;
    }
    Ok(())
}

fn claim_runnable_turn(database: &Path) -> Result<Option<TurnClaim>, StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let selected = transaction
        .query_row(
            "SELECT b.batch_id,ps.session_id,ps.provider_session_id,
                    r.name_with_owner,w.number,COALESCE(w.context_revision,ps.context_revision),
                    ai.profile_id,a.lifecycle
             FROM wake_batches b
             JOIN work_items w ON w.node_id=b.work_item_node_id
             JOIN repositories r ON r.node_id=w.repository_node_id
             JOIN assignments a ON a.work_item_node_id=w.node_id
               AND a.lifecycle IN ('active','finalizing')
             JOIN agent_instances ai ON ai.assignment_id=a.assignment_id
               AND ((a.lifecycle='active' AND ai.lifecycle='idle')
                    OR (a.lifecycle='finalizing' AND ai.lifecycle='finalizing'))
             JOIN provider_sessions ps ON ps.agent_id=ai.agent_id AND ps.lifecycle='idle'
             WHERE b.lifecycle='runnable' AND w.kind='issue'
             ORDER BY b.created_at,b.batch_id LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    sqlite_i64_to_u64(row.get(4)?, "turn Issue number")?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?;
    let Some((
        batch_id,
        session_id,
        provider_session_id,
        repository,
        number,
        context_revision,
        profile_id,
        assignment_lifecycle,
    )) = selected
    else {
        transaction.commit()?;
        return Ok(None);
    };
    let (references, trusted_mention) = batch_references(&transaction, &batch_id)?;
    let trigger_kind = if assignment_lifecycle == "finalizing" {
        "finalization"
    } else if trusted_mention {
        "trusted_mention"
    } else {
        "wake_batch"
    };
    let turn_id = Uuid::now_v7().to_string();
    transaction.execute(
        "UPDATE wake_batches SET lifecycle='consumed',updated_at=?2
         WHERE batch_id=?1 AND lifecycle='runnable'",
        params![batch_id, now],
    )?;
    transaction.execute(
        "UPDATE provider_sessions SET lifecycle='running'
         WHERE session_id=?1 AND lifecycle='idle'",
        [&session_id],
    )?;
    transaction.execute(
        "INSERT INTO turns(
           turn_id,session_id,context_revision,trigger_kind,lifecycle,batch_id
         ) VALUES (?1,?2,?3,?4,'starting',?5)",
        params![turn_id, session_id, context_revision, trigger_kind, batch_id,],
    )?;
    transaction.commit()?;
    Ok(Some(TurnClaim {
        turn_id,
        batch_id,
        provider_session_id,
        repository,
        number,
        context_revision,
        profile_id,
        references,
        trusted_mention,
        trigger_kind: trigger_kind.into(),
    }))
}

fn mark_turn_started(
    database: &Path,
    turn_id: &str,
    provider_turn_id: &str,
) -> Result<(), StoreError> {
    require_current_schema(database)?;
    let connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let updated = connection.execute(
        "UPDATE turns SET provider_turn_id=?2,lifecycle='running',started_at=?3
         WHERE turn_id=?1 AND lifecycle='starting'",
        params![turn_id, provider_turn_id, now_rfc3339()],
    )?;
    if updated == 1 {
        Ok(())
    } else {
        Err(StoreError::InvalidData(format!("turn {turn_id} is not starting")))
    }
}

fn mark_turn_terminal(database: &Path, turn_id: &str, lifecycle: &str) -> Result<(), StoreError> {
    require_current_schema(database)?;
    if !matches!(lifecycle, "completed" | "interrupted" | "failed" | "unknown") {
        return Err(StoreError::InvalidData(format!("invalid turn terminal {lifecycle}")));
    }
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let (session_id, trigger_kind, agent_id, assignment_id, work_item_node_id) = transaction
        .query_row(
            "SELECT t.session_id,t.trigger_kind,ps.agent_id,ai.assignment_id,a.work_item_node_id
             FROM turns t
             JOIN provider_sessions ps ON ps.session_id=t.session_id
             JOIN agent_instances ai ON ai.agent_id=ps.agent_id
             JOIN assignments a ON a.assignment_id=ai.assignment_id
             WHERE t.turn_id=?1 AND t.lifecycle IN ('starting','running')",
            [turn_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| StoreError::InvalidData(format!("turn {turn_id} is not active")))?;
    transaction.execute(
        "UPDATE turns SET lifecycle=?2,ended_at=?3 WHERE turn_id=?1",
        params![turn_id, lifecycle, now],
    )?;
    let finalization = trigger_kind == "finalization";
    let session_lifecycle = if lifecycle == "unknown" {
        "unknown"
    } else if finalization {
        "sleeping"
    } else {
        "idle"
    };
    transaction.execute(
        "UPDATE provider_sessions SET lifecycle=?2 WHERE session_id=?1",
        params![session_id, session_lifecycle],
    )?;
    if finalization && lifecycle != "unknown" {
        transaction.execute(
            "UPDATE assignments SET lifecycle='sleeping'
             WHERE assignment_id=?1 AND lifecycle='finalizing'",
            [&assignment_id],
        )?;
        transaction.execute(
            "UPDATE agent_instances SET lifecycle='sleeping'
             WHERE agent_id=?1 AND lifecycle='finalizing'",
            [&agent_id],
        )?;
        transaction.execute(
            "UPDATE events SET lifecycle='consumed'
             WHERE event_id IN (
               SELECT be.event_id FROM turns t
               JOIN wake_batch_events be ON be.batch_id=t.batch_id
               WHERE t.turn_id=?1
             ) AND lifecycle='pending'",
            [turn_id],
        )?;
        transaction.execute(
            "UPDATE wake_batches SET lifecycle='consumed',updated_at=?2
             WHERE work_item_node_id=?1 AND lifecycle IN ('pending','runnable')",
            params![work_item_node_id, now],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn claim_urgent_steer(database: &Path, turn_id: &str) -> Result<Option<TurnClaim>, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_only(database)?;
    let selected = connection
        .query_row(
            "SELECT b.batch_id,ps.provider_session_id,
                    r.name_with_owner,w.number,COALESCE(w.context_revision,ps.context_revision),
                    ai.profile_id
             FROM turns t
             JOIN provider_sessions ps ON ps.session_id=t.session_id
             JOIN agent_instances ai ON ai.agent_id=ps.agent_id
             JOIN assignments a ON a.assignment_id=ai.assignment_id
             JOIN work_items w ON w.node_id=a.work_item_node_id
             JOIN repositories r ON r.node_id=w.repository_node_id
             JOIN wake_batches b ON b.work_item_node_id=w.node_id
             WHERE t.turn_id=?1 AND t.lifecycle='running'
               AND b.lifecycle='runnable' AND b.urgent=1
             ORDER BY b.created_at,b.batch_id LIMIT 1",
            [turn_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    sqlite_i64_to_u64(row.get(3)?, "steer Issue number")?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((batch_id, provider_session_id, repository, number, context_revision, profile_id)) =
        selected
    else {
        return Ok(None);
    };
    let (references, trusted_mention) = batch_references_connection(&connection, &batch_id)?;
    Ok(Some(TurnClaim {
        turn_id: turn_id.into(),
        batch_id,
        provider_session_id,
        repository,
        number,
        context_revision,
        profile_id,
        references,
        trusted_mention,
        trigger_kind: "urgent_steer".into(),
    }))
}

fn consume_steer_batch(database: &Path, batch_id: &str) -> Result<(), StoreError> {
    require_current_schema(database)?;
    let connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let updated = connection.execute(
        "UPDATE wake_batches SET lifecycle='consumed',updated_at=?2
         WHERE batch_id=?1 AND lifecycle='runnable' AND urgent=1",
        params![batch_id, now_rfc3339()],
    )?;
    if updated == 1 {
        Ok(())
    } else {
        Err(StoreError::InvalidData(format!("urgent batch {batch_id} is not runnable")))
    }
}

fn batch_references(
    transaction: &rusqlite::Transaction<'_>,
    batch_id: &str,
) -> Result<(Vec<String>, bool), StoreError> {
    let mut statement = transaction.prepare(
        "SELECT e.reference,COALESCE(e.trusted_mention,0)
         FROM wake_batch_events be JOIN events e ON e.event_id=be.event_id
         WHERE be.batch_id=?1 ORDER BY be.ordinal",
    )?;
    let rows = statement
        .query_map([batch_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0)))?;
    let values = rows.collect::<Result<Vec<_>, _>>()?;
    let trusted = values.iter().any(|value| value.1);
    Ok((values.into_iter().map(|value| value.0).collect(), trusted))
}

fn batch_references_connection(
    connection: &Connection,
    batch_id: &str,
) -> Result<(Vec<String>, bool), StoreError> {
    let mut statement = connection.prepare(
        "SELECT e.reference,COALESCE(e.trusted_mention,0)
         FROM wake_batch_events be JOIN events e ON e.event_id=be.event_id
         WHERE be.batch_id=?1 ORDER BY be.ordinal",
    )?;
    let rows = statement
        .query_map([batch_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0)))?;
    let values = rows.collect::<Result<Vec<_>, _>>()?;
    let trusted = values.iter().any(|value| value.1);
    Ok((values.into_iter().map(|value| value.0).collect(), trusted))
}

fn enqueue_turn_reaction(database: &Path, turn_id: &str, content: &str) -> Result<(), StoreError> {
    require_current_schema(database)?;
    if !matches!(content, "rocket" | "+1" | "confused") {
        return Err(StoreError::InvalidData(format!("invalid turn reaction {content}")));
    }
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let target = transaction
        .query_row(
            "SELECT o.repository,o.target_kind,o.target_database_id
             FROM turns t
             JOIN wake_batch_events be ON be.batch_id=t.batch_id
             JOIN events e ON e.event_id=be.event_id AND e.trusted_mention=1
             JOIN github_write_outbox o ON o.event_id=e.event_id
             WHERE t.turn_id=?1 LIMIT 1",
            [turn_id],
            |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            },
        )
        .optional()?;
    if let Some((repository, target_kind, target_database_id)) = target {
        if content != "rocket" {
            enqueue_rocket_removal(
                &transaction,
                &repository,
                &target_kind,
                &target_database_id,
                &now,
            )?;
        }
        let request_digest = hex::encode(Sha256::digest(
            format!("{repository}\0{target_kind}\0{target_database_id}\0{content}").as_bytes(),
        ));
        transaction.execute(
            "INSERT OR IGNORE INTO github_write_outbox(
               intent_id,repository,target_kind,target_database_id,operation,content,
               request_digest,lifecycle,next_attempt_at,created_at,updated_at
             ) VALUES (?1,?2,?3,?4,'reaction_add',?5,?6,'pending',?7,?7,?7)",
            params![
                Uuid::now_v7().to_string(),
                repository,
                target_kind,
                target_database_id,
                content,
                request_digest,
                now,
            ],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn enqueue_operational_status(
    database: &Path,
    turn_id: &str,
    body: &str,
) -> Result<(), StoreError> {
    require_current_schema(database)?;
    if body.trim().is_empty() {
        return Err(StoreError::InvalidData("Operational Status body is empty".into()));
    }
    let now = now_rfc3339();
    let body_digest = hex::encode(Sha256::digest(body.as_bytes()));
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let (work_item_node_id, repository, number, profile_id, generation, remote_node, remote_id) =
        transaction.query_row(
            "SELECT w.node_id,r.name_with_owner,w.number,ai.profile_id,a.generation,
                    sc.remote_comment_node_id,sc.remote_comment_database_id
             FROM turns t
             JOIN provider_sessions ps ON ps.session_id=t.session_id
             JOIN agent_instances ai ON ai.agent_id=ps.agent_id
             JOIN assignments a ON a.assignment_id=ai.assignment_id
             JOIN work_items w ON w.node_id=a.work_item_node_id
             JOIN repositories r ON r.node_id=w.repository_node_id
             LEFT JOIN status_comments sc
               ON sc.work_item_node_id=w.node_id AND sc.profile_id=ai.profile_id
             WHERE t.turn_id=?1",
            [turn_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    sqlite_i64_to_u64(row.get(2)?, "status Issue number")?,
                    row.get::<_, String>(3)?,
                    sqlite_i64_to_u64(row.get(4)?, "status assignment generation")?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )?;
    let (operation, target_kind, target_database_id) = match (remote_node, remote_id) {
        (Some(_), Some(database_id)) => ("comment_update", "issue_comment", database_id),
        _ => ("comment_create", "issue", number.to_string()),
    };
    transaction.execute(
        "INSERT INTO status_comments(
           work_item_node_id,profile_id,remote_comment_node_id,remote_comment_database_id,
           body_digest,lifecycle,updated_at
         ) VALUES (?1,?2,NULL,NULL,?3,'pending',?4)
         ON CONFLICT(work_item_node_id,profile_id) DO UPDATE SET
           body_digest=excluded.body_digest,lifecycle='pending',updated_at=excluded.updated_at",
        params![work_item_node_id, profile_id, body_digest, now],
    )?;
    let request_digest = hex::encode(Sha256::digest(
        format!(
            "operational-status\0{repository}\0{work_item_node_id}\0{profile_id}\0{generation}\0{body_digest}"
        )
        .as_bytes(),
    ));
    transaction.execute(
        "INSERT OR IGNORE INTO github_write_outbox(
           intent_id,repository,target_kind,target_database_id,operation,content,
           request_digest,lifecycle,next_attempt_at,created_at,updated_at
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,'pending',?8,?8,?8)",
        params![
            Uuid::now_v7().to_string(),
            repository,
            target_kind,
            target_database_id,
            operation,
            body,
            request_digest,
            now,
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn settle_operational_status(
    transaction: &rusqlite::Transaction<'_>,
    repository: &str,
    target_kind: &str,
    target_database_id: &str,
    body: &str,
    lifecycle: &str,
    remote_database_id: Option<&str>,
    remote_node_id: Option<&str>,
    now: &str,
) -> Result<(), StoreError> {
    let body_digest = hex::encode(Sha256::digest(body.as_bytes()));
    if target_kind == "issue" {
        transaction.execute(
            "UPDATE status_comments SET remote_comment_node_id=COALESCE(?4,remote_comment_node_id),
               remote_comment_database_id=COALESCE(?5,remote_comment_database_id),
               lifecycle=?6,updated_at=?7
             WHERE work_item_node_id=(
               SELECT w.node_id FROM work_items w
               JOIN repositories r ON r.node_id=w.repository_node_id
               WHERE r.name_with_owner=?1 AND w.kind='issue' AND w.number=?2
             ) AND body_digest=?3",
            params![
                repository,
                target_database_id,
                body_digest,
                remote_node_id,
                remote_database_id,
                lifecycle,
                now,
            ],
        )?;
    } else if target_kind == "issue_comment" {
        transaction.execute(
            "UPDATE status_comments SET body_digest=?2,lifecycle=?3,updated_at=?4
             WHERE remote_comment_database_id=?1",
            params![target_database_id, body_digest, lifecycle, now],
        )?;
    }
    Ok(())
}

fn enqueue_rocket_removal(
    transaction: &rusqlite::Transaction<'_>,
    repository: &str,
    target_kind: &str,
    target_database_id: &str,
    now: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "UPDATE github_write_outbox SET lifecycle='superseded',updated_at=?4
         WHERE repository=?1 AND target_kind=?2 AND target_database_id=?3
           AND operation='reaction_add' AND content='rocket' AND lifecycle='pending'",
        params![repository, target_kind, target_database_id, now],
    )?;
    let remote = transaction
        .query_row(
            "SELECT remote_database_id FROM github_write_outbox
             WHERE repository=?1 AND target_kind=?2 AND target_database_id=?3
               AND operation='reaction_add' AND content='rocket' AND lifecycle='applied'
               AND remote_database_id IS NOT NULL
             ORDER BY updated_at DESC LIMIT 1",
            params![repository, target_kind, target_database_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(remote_database_id) = remote else { return Ok(()) };
    let request_digest = hex::encode(Sha256::digest(
        format!(
            "{repository}\0{target_kind}\0{target_database_id}\0reaction_delete\0{remote_database_id}"
        )
        .as_bytes(),
    ));
    transaction.execute(
        "INSERT OR IGNORE INTO github_write_outbox(
           intent_id,repository,target_kind,target_database_id,operation,content,
           request_digest,remote_database_id,lifecycle,next_attempt_at,created_at,updated_at
         ) VALUES (?1,?2,?3,?4,'reaction_delete','rocket',?5,?6,'pending',?7,?7,?7)",
        params![
            Uuid::now_v7().to_string(),
            repository,
            target_kind,
            target_database_id,
            request_digest,
            remote_database_id,
            now,
        ],
    )?;
    Ok(())
}

fn scalar_u64(connection: &Connection, query: &str) -> Result<u64, StoreError> {
    let value = connection.query_row(query, [], |row| row.get::<_, i64>(0))?;
    Ok(sqlite_i64_to_u64(value, "SQLite count")?)
}

fn sqlite_u64(value: u64, name: &str) -> Result<i64, StoreError> {
    i64::try_from(value)
        .map_err(|_| StoreError::InvalidData(format!("{name} exceeds SQLite INTEGER")))
}

fn sqlite_usize(value: usize, name: &str) -> Result<i64, StoreError> {
    i64::try_from(value)
        .map_err(|_| StoreError::InvalidData(format!("{name} exceeds SQLite INTEGER")))
}

fn sqlite_i64_to_u64(value: i64, name: &str) -> Result<u64, rusqlite::Error> {
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(StoreConversionError(format!("{name} is negative: {error}"))),
        )
    })
}

#[derive(Debug)]
struct StoreConversionError(String);

impl std::fmt::Display for StoreConversionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StoreConversionError {}

fn deadline_rfc3339(seconds: u64) -> Result<String, StoreError> {
    let seconds = i64::try_from(seconds)
        .map_err(|_| StoreError::InvalidData("duration exceeds i64 seconds".into()))?;
    (OffsetDateTime::now_utc() + TimeDuration::seconds(seconds))
        .format(&Rfc3339)
        .map_err(|error| StoreError::InvalidData(format!("cannot format deadline: {error}")))
}

fn require_current_schema(database: &Path) -> Result<(), StoreError> {
    let migration_plan = plan(database)?;
    if migration_plan.current_schema == DATABASE_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(StoreError::SchemaNotReady {
            found: migration_plan.current_schema,
            required: DATABASE_SCHEMA_VERSION,
        })
    }
}

struct MigrationLease {
    _file: File,
}

impl MigrationLease {
    fn acquire(database: &Path) -> Result<Self, StoreError> {
        let mut lock_name = OsString::from(database.as_os_str());
        lock_name.push(".migrate.lock");
        let path = PathBuf::from(lock_name);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|source| StoreError::Io { path: path.clone(), source })?;
        fs2::FileExt::try_lock_exclusive(&file).map_err(|source| {
            if source.kind() == std::io::ErrorKind::WouldBlock {
                StoreError::MigrationBusy(path.clone())
            } else {
                StoreError::Io { path: path.clone(), source }
            }
        })?;
        Ok(Self { _file: file })
    }
}

fn read_ledger(database: &Path) -> Result<Vec<LedgerEntry>, StoreError> {
    if !database.is_file() || fs::metadata(database).map_or(0, |metadata| metadata.len()) == 0 {
        return Ok(Vec::new());
    }
    let connection = open_read_only(database)?;
    let table_names = user_table_names(&connection)?;
    if !table_names.iter().any(|name| name == "schema_migrations") {
        if table_names.is_empty() {
            return Ok(Vec::new());
        }
        return Err(StoreError::ForeignDatabase { path: database.to_path_buf() });
    }
    let mut statement = connection
        .prepare("SELECT version, checksum FROM schema_migrations ORDER BY version ASC")?;
    let rows = statement
        .query_map([], |row| Ok(LedgerEntry { version: row.get(0)?, checksum: row.get(1)? }))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
}

fn validate_ledger(ledger: &[LedgerEntry]) -> Result<(), StoreError> {
    if let Some(found) = ledger.last().map(|entry| entry.version)
        && found > DATABASE_SCHEMA_VERSION
    {
        return Err(StoreError::NewerSchema { found, supported: DATABASE_SCHEMA_VERSION });
    }
    for (index, entry) in ledger.iter().enumerate() {
        let expected_version = u32::try_from(index + 1).expect("migration count fits u32");
        if entry.version != expected_version {
            return Err(StoreError::NonContiguous { version: entry.version });
        }
        let migration = MIGRATIONS
            .iter()
            .find(|migration| migration.version == entry.version)
            .ok_or(StoreError::NewerSchema {
                found: entry.version,
                supported: DATABASE_SCHEMA_VERSION,
            })?;
        if entry.checksum != migration_checksum(migration) {
            return Err(StoreError::ChecksumMismatch { version: entry.version });
        }
    }
    Ok(())
}

fn apply_one(connection: &mut Connection, migration: &Migration) -> Result<(), StoreError> {
    connection.execute_batch("BEGIN EXCLUSIVE").map_err(|error| StoreError::Migration {
        version: migration.version,
        message: error.to_string(),
    })?;
    let result = (|| -> Result<(), rusqlite::Error> {
        connection.execute_batch(migration.sql)?;
        connection.execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES (?1, ?2, ?3, ?4)",
            (
                migration.version,
                migration.name,
                migration_checksum(migration),
                now_rfc3339(),
            ),
        )?;
        connection.execute_batch("COMMIT")?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = connection.execute_batch("ROLLBACK");
        return Err(StoreError::Migration {
            version: migration.version,
            message: error.to_string(),
        });
    }
    Ok(())
}

fn create_backup(
    source: &Connection,
    backups: &Path,
    next_version: u32,
) -> Result<PathBuf, StoreError> {
    let timestamp = OffsetDateTime::now_utc().unix_timestamp_nanos();
    let target = backups.join(format!("braid.before-v{next_version}.{timestamp}.sqlite3"));
    if target.exists() {
        return Err(StoreError::BackupExists(target));
    }
    let mut destination = Connection::open(&target)?;
    let backup = Backup::new(source, &mut destination)?;
    backup.run_to_completion(16, Duration::from_millis(10), None)?;
    drop(backup);
    destination.close().map_err(|(_, error)| StoreError::Sqlite(error))?;
    Ok(target)
}

fn open_read_only(path: &Path) -> Result<Connection, StoreError> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(StoreError::from)
}

fn open_read_write(path: &Path) -> Result<Connection, StoreError> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(StoreError::from)
}

fn configure_connection(connection: &Connection) -> Result<(), StoreError> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;",
    )?;
    Ok(())
}

fn user_table_names(connection: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
}

fn migration_checksum(migration: &Migration) -> String {
    hex::encode(Sha256::digest(migration.sql.as_bytes()))
}

fn create_dir_all(path: &Path) -> Result<(), StoreError> {
    fs::create_dir_all(path).map_err(|source| StoreError::Io { path: path.to_path_buf(), source })
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc().format(&Rfc3339).expect("UTC timestamp formats as RFC 3339")
}

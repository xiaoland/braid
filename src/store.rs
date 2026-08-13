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
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const DATABASE_SCHEMA_VERSION: u32 = 2;

const INITIAL_SQL: &str = include_str!("../migrations/0001_initial.sql");
const CONTEXT_LEDGER_SQL: &str = include_str!("../migrations/0002_context_ledger.sql");
const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, name: "initial", sql: INITIAL_SQL },
    Migration { version: 2, name: "context-ledger", sql: CONTEXT_LEDGER_SQL },
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

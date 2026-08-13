use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, backup::Backup};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const DATABASE_SCHEMA_VERSION: u32 = 1;

const INITIAL_SQL: &str = include_str!("../migrations/0001_initial.sql");
const MIGRATIONS: &[Migration] = &[Migration { version: 1, name: "initial", sql: INITIAL_SQL }];

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

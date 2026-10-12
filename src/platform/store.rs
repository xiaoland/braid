use super::config::PlatformConfig;
use anyhow::{Result, ensure};
use rusqlite::{Connection, params};
use std::path::Path;
const MIGRATIONS: [(u32, &str, &str); 3] = [
    (1, "initial", include_str!("schema.sql")),
    (2, "schema_checksums", include_str!("migrations/0002_schema_checksums.sql")),
    (3, "issue_tree", include_str!("migrations/0003_issue_tree.sql")),
];
fn checksum(sql: &str) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(sql.as_bytes()))
}
pub fn verify_schema(connection: &Connection) -> Result<()> {
    let mut statement = connection
        .prepare("SELECT version,name,checksum FROM platform_schema_migrations ORDER BY version")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(rows.len() == MIGRATIONS.len(), "platform database schema is not current");
    for ((version, name, digest), (expected, expected_name, sql)) in
        rows.into_iter().zip(MIGRATIONS)
    {
        ensure!(
            version == expected && name == expected_name && digest == checksum(sql),
            "platform migration {version} checksum/name differs from its embedded immutable migration"
        );
    }
    Ok(())
}
fn upgrade_schema(connection: &Connection) -> Result<()> {
    let version: u32 =
        connection.query_row("SELECT MAX(version) FROM platform_schema_migrations", [], |row| {
            row.get(0)
        })?;
    ensure!((1..=3).contains(&version), "unsupported platform database schema {version}");
    if version == 1 {
        connection.execute_batch(MIGRATIONS[1].2)?;
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)?;
        connection.execute("UPDATE platform_schema_migrations SET name=?1,checksum=?2,applied_at=?3 WHERE version=1",params![MIGRATIONS[0].1,checksum(MIGRATIONS[0].2),now])?;
        connection.execute("INSERT INTO platform_schema_migrations(version,name,checksum,applied_at) VALUES(2,?1,?2,?3)",params![MIGRATIONS[1].1,checksum(MIGRATIONS[1].2),now])?;
    }
    if version < 3 {
        connection.execute_batch(MIGRATIONS[2].2)?;
        connection.execute("INSERT INTO platform_schema_migrations(version,name,checksum,applied_at) VALUES(3,?1,?2,datetime('now'))",params![MIGRATIONS[2].1,checksum(MIGRATIONS[2].2)])?;
    }
    verify_schema(connection)
}
pub fn initialize(config: &PlatformConfig) -> Result<()> {
    config.validate()?;
    if let Some(parent) = config.database.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(&config.backups)?;
    let mut connection = Connection::open(&config.database)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    let has_ledger:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='platform_schema_migrations')",[],|row|row.get(0))?;
    let old_version: Option<u32> = if has_ledger {
        Some(connection.query_row(
            "SELECT MAX(version) FROM platform_schema_migrations",
            [],
            |row| row.get(0),
        )?)
    } else {
        None
    };
    if old_version.is_some_and(|version| version < 3) {
        connection.backup(
            rusqlite::MAIN_DB,
            config.backups.join(format!(
                "platform-schema{}-{}.sqlite3",
                old_version.unwrap(),
                uuid::Uuid::now_v7()
            )),
            None,
        )?;
    }
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let tables: i64 = tx.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        [],
        |r| r.get(0),
    )?;
    if tables == 0 {
        tx.execute_batch(include_str!("schema.sql"))?;
    }
    upgrade_schema(&tx)?;
    tx.execute("INSERT OR IGNORE INTO local_space(singleton,repository,source,space_id,default_branch) VALUES(1,?1,?2,?3,?4)",params![config.repository,config.source.to_str(),uuid::Uuid::now_v7().to_string(),config.default_branch])?;
    let (repo, source, branch): (String, String, String) = tx.query_row(
        "SELECT repository,source,default_branch FROM local_space WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    ensure!(
        repo == config.repository
            && source == config.source.to_string_lossy()
            && branch == config.default_branch,
        "platform database belongs to another repository/source/default branch"
    );
    tx.commit()?;
    Ok(())
}
/// Import under a source write-exclusion transaction. The original authority is preserved.
pub fn import(config: &PlatformConfig, source: &Path) -> Result<()> {
    ensure!(
        !config.database.exists(),
        "import destination already exists; choose a new platform database"
    );
    let source = source.canonicalize()?;
    let old = Connection::open_with_flags(&source, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    old.busy_timeout(std::time::Duration::from_secs(1))?;
    old.execute_batch("BEGIN IMMEDIATE")?;
    let version: u32 =
        old.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0))?;
    ensure!(version >= 5, "import requires runtime schema 5 or newer");
    let now =
        time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?;
    let leased: bool = old.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_leases WHERE scope='runtime' AND expires_at>?1)",
        [now],
        |r| r.get(0),
    )?;
    ensure!(!leased, "source runtime still has an active lease; stop its old binary before import");
    let prepared:bool=old.query_row("SELECT EXISTS(SELECT 1 FROM control_receipts WHERE scope='objects' AND state='prepared') OR EXISTS(SELECT 1 FROM local_merges WHERE lifecycle='prepared')",[],|r|r.get(0))?;
    ensure!(
        !prepared,
        "source contains unresolved prepared object commands/merges; settle them with its original authority before import"
    );
    let (repo, bound): (String, String) =
        old.query_row("SELECT repository,source FROM local_space WHERE singleton=1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    ensure!(
        repo == config.repository && Path::new(&bound).canonicalize()? == config.source,
        "import repository/source differs from platform declaration"
    );
    let temporary =
        config.database.with_extension(format!("import-{}.sqlite3", uuid::Uuid::now_v7()));
    let mut staged = config.clone();
    staged.database.clone_from(&temporary);
    let result = (|| -> Result<()> {
        initialize(&staged)?;
        let mut destination = Connection::open(&temporary)?;
        destination.execute("ATTACH DATABASE ?1 AS legacy", [source.to_str()])?;
        let tx = destination.transaction()?;
        for table in [
            "local_items",
            "local_comments",
            "local_links",
            "local_events",
            "local_reactions",
            "local_write_receipts",
            "local_merges",
        ] {
            tx.execute_batch(&format!("INSERT INTO main.{table} SELECT * FROM legacy.{table}"))?;
        }
        tx.execute_batch("INSERT INTO main.control_receipts SELECT * FROM legacy.control_receipts WHERE scope='objects'")?;
        tx.commit()?;
        drop(destination);
        super::objects::PlatformObjects::open(&temporary, &config.repository)?
            .pin_existing_candidates(&config.source)?;
        std::fs::hard_link(&temporary, &config.database)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&temporary);
    old.execute_batch("ROLLBACK")?;
    result
}

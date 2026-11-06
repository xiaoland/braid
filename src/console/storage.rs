use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Debug, Serialize)]
pub struct Record {
    pub id: i64,
    pub batch_id: i64,
    pub duplicate_of: Option<i64>,
    pub signal: String,
    pub received_at: i64,
    pub instance: String,
    pub service: String,
    pub session: String,
    pub turn_id: String,
    pub trace_id: String,
    pub span_id: String,
    pub resource: Value,
    pub scope: Value,
    pub data: Value,
}

pub struct DecodedRecord {
    pub instance: String,
    pub service: String,
    pub session: String,
    pub turn_id: String,
    pub trace_id: String,
    pub span_id: String,
    pub resource: Value,
    pub scope: Value,
    pub data: Value,
}

#[derive(Default, Deserialize)]
pub struct Filter {
    pub signal: Option<String>,
    pub instance: Option<String>,
    pub session: Option<String>,
    pub trace: Option<String>,
    pub turn: Option<String>,
    pub after: Option<i64>,
    pub limit: Option<usize>,
}

pub struct Storage(Connection);

impl Storage {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = Connection::open(path).context("open Console telemetry database")?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let application_id: i64 = db.query_row("PRAGMA application_id", [], |row| row.get(0))?;
        let foreign_tables: i64 = db.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT IN ('console_batches','console_records','console_receive_errors')", [], |row| row.get(0))?;
        if !matches!(application_id, 0 | 1_112_498_510) || foreign_tables != 0 {
            bail!(
                "Console requires a separate telemetry database; this database belongs to another component"
            );
        }
        db.execute_batch("PRAGMA application_id=1112498510;")?;
        db.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS console_batches (
                id INTEGER PRIMARY KEY, signal TEXT NOT NULL, received_at INTEGER NOT NULL,
                encoding TEXT NOT NULL, payload BLOB NOT NULL, sha256 TEXT NOT NULL, duplicate_of INTEGER);
            CREATE INDEX IF NOT EXISTS console_batches_digest ON console_batches(signal,encoding,sha256);
            CREATE TABLE IF NOT EXISTS console_receive_errors (id INTEGER PRIMARY KEY, received_at INTEGER NOT NULL, signal TEXT NOT NULL, status INTEGER NOT NULL, reason TEXT NOT NULL, payload BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS console_records (
                id INTEGER PRIMARY KEY, batch_id INTEGER NOT NULL REFERENCES console_batches(id),
                signal TEXT NOT NULL, received_at INTEGER NOT NULL, instance TEXT NOT NULL,
                service TEXT NOT NULL, session TEXT NOT NULL, turn_id TEXT NOT NULL, trace_id TEXT NOT NULL,
                span_id TEXT NOT NULL, resource TEXT NOT NULL, scope TEXT NOT NULL, data TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS console_records_instance ON console_records(instance,id);
            CREATE INDEX IF NOT EXISTS console_records_session ON console_records(session,id);
            CREATE INDEX IF NOT EXISTS console_records_trace ON console_records(trace_id,id);")?;
        Ok(Self(db))
    }

    pub fn ingest(
        &mut self,
        signal: &str,
        encoding: &str,
        payload: &[u8],
        records: Vec<DecodedRecord>,
    ) -> Result<()> {
        let received = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
        let digest = hex::encode(Sha256::digest(payload));
        let tx = self.0.transaction()?;
        let duplicate_of: Option<i64> = tx.query_row(
            "SELECT min(id) FROM console_batches WHERE signal=? AND encoding=? AND sha256=?",
            params![signal, encoding, digest],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO console_batches(signal,received_at,encoding,payload,sha256,duplicate_of) VALUES(?,?,?,?,?,?)",
            params![signal, received, encoding, payload,digest,duplicate_of],
        )?;
        let batch_id = tx.last_insert_rowid();
        for record in records {
            tx.execute("INSERT INTO console_records(batch_id,signal,received_at,instance,service,session,turn_id,trace_id,span_id,resource,scope,data) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
                params![batch_id,signal,received,record.instance,record.service,record.session,record.turn_id,record.trace_id,record.span_id,serde_json::to_string(&record.resource)?,serde_json::to_string(&record.scope)?,serde_json::to_string(&record.data)?])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn records(&self, filter: &Filter) -> Result<Value> {
        let limit = i64::try_from(filter.limit.unwrap_or(100).clamp(1, 200))?;
        let mut statement = self.0.prepare("SELECT id,batch_id,signal,received_at,instance,service,session,turn_id,trace_id,span_id,resource,scope,data,(SELECT duplicate_of FROM console_batches WHERE id=batch_id) FROM console_records
            WHERE id>? AND (? IS NULL OR signal=?) AND (? IS NULL OR instance=?) AND (? IS NULL OR session=?) AND (? IS NULL OR trace_id=?) AND (? IS NULL OR turn_id=?) ORDER BY id LIMIT ?")?;
        let mut rows = statement.query(params![
            filter.after.unwrap_or(0),
            filter.signal,
            filter.signal,
            filter.instance,
            filter.instance,
            filter.session,
            filter.session,
            filter.trace,
            filter.trace,
            filter.turn,
            filter.turn,
            limit
        ])?;
        let mut records = Vec::new();
        while let Some(row) = rows.next()? {
            records.push(Record {
                id: row.get(0)?,
                batch_id: row.get(1)?,
                duplicate_of: row.get(13)?,
                signal: row.get(2)?,
                received_at: row.get(3)?,
                instance: row.get(4)?,
                service: row.get(5)?,
                session: row.get(6)?,
                turn_id: row.get(7)?,
                trace_id: row.get(8)?,
                span_id: row.get(9)?,
                resource: serde_json::from_str(&row.get::<_, String>(10)?)?,
                scope: serde_json::from_str(&row.get::<_, String>(11)?)?,
                data: serde_json::from_str(&row.get::<_, String>(12)?)?,
            });
        }
        Ok(
            json!({"next_after":records.last().map_or(filter.after.unwrap_or(0),|r|r.id),"records":records}),
        )
    }

    pub fn resources(&self) -> Result<Value> {
        let mut statement = self.0.prepare("SELECT instance,service,min(received_at),max(received_at),count(*) FROM console_records GROUP BY instance,service ORDER BY max(received_at) DESC LIMIT 200")?;
        let resources = statement.query_map([],|r| Ok(json!({"instance":r.get::<_,String>(0)?,"service":r.get::<_,String>(1)?,"first_received":r.get::<_,i64>(2)?,"last_received":r.get::<_,i64>(3)?,"records":r.get::<_,i64>(4)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let batches: i64 =
            self.0.query_row("SELECT count(*) FROM console_batches", [], |r| r.get(0))?;
        let (records, last_received): (i64, Option<i64>) =
            self.0.query_row("SELECT count(*),max(received_at) FROM console_records", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        let errors: i64 =
            self.0.query_row("SELECT count(*) FROM console_receive_errors", [], |r| r.get(0))?;
        let duplicate_batches: i64 = self.0.query_row(
            "SELECT count(*) FROM console_batches WHERE duplicate_of IS NOT NULL",
            [],
            |r| r.get(0),
        )?;
        Ok(
            json!({"receive_errors":errors,"duplicate_batches":duplicate_batches,"resources":resources,"batches":batches,"records":records,"last_received":last_received,"authority":"OTLP collector observations; runtime status comes from controller","gaps":"unknown"}),
        )
    }

    pub fn reject(&self, signal: &str, status: u16, reason: &str, payload: &[u8]) -> Result<()> {
        let received = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
        self.0.execute("INSERT INTO console_receive_errors(received_at,signal,status,reason,payload) VALUES(?,?,?,?,?)",params![received,signal,status,reason,payload])?;
        Ok(())
    }

    pub fn receive_errors(&self, filter: &Filter) -> Result<Value> {
        let limit = i64::try_from(filter.limit.unwrap_or(100).clamp(1, 200))?;
        let mut statement=self.0.prepare("SELECT id,received_at,signal,status,reason,length(payload) FROM console_receive_errors WHERE id>? ORDER BY id LIMIT ?")?;
        let errors=statement.query_map(params![filter.after.unwrap_or(0),limit],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"received_at":r.get::<_,i64>(1)?,"signal":r.get::<_,String>(2)?,"status":r.get::<_,u16>(3)?,"reason":r.get::<_,String>(4)?,"wire_bytes":r.get::<_,i64>(5)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(
            json!({"errors":errors,"next_after":errors.last().and_then(|e|e["id"].as_i64()).unwrap_or(filter.after.unwrap_or(0))}),
        )
    }

    pub fn rejected_body(&self, id: i64) -> Result<Option<Vec<u8>>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .0
            .query_row("SELECT payload FROM console_receive_errors WHERE id=?", [id], |r| r.get(0))
            .optional()?)
    }

    pub fn batch(&self, id: i64) -> Result<Option<(String, Vec<u8>)>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .0
            .query_row("SELECT encoding,payload FROM console_batches WHERE id=?", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?)
    }
}

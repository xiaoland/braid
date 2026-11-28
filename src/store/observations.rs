//! A processed observation is a delivery commitment, not a Context cache update.
use super::{CanonicalObjectState, IngressEvent, ObservedRootFields, SchedulerPolicy, StoreError};
use rusqlite::{Connection, OptionalExtension as _, params};
use sha2::{Digest as _, Sha256};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
#[error(
    "complete observation predates classified object {node} ({kind}): snapshot={snapshot_version} [{snapshot_lifecycle}], classified={classified_version} evidence={classified_evidence}"
)]
pub struct ObservationConflict {
    pub node: String,
    pub kind: String,
    pub snapshot_version: String,
    pub snapshot_lifecycle: String,
    pub classified_version: String,
    pub classified_evidence: String,
}

/// Captured before the API traversal, never reconstructed from its result.
#[derive(Debug, Clone)]
pub(crate) struct ObservationBasis {
    work_item_node_id: String,
    pub objects: Vec<CanonicalObjectState>,
    admitted_events: i64,
}

pub(super) fn begin(database: &Path, work_item: &str) -> Result<ObservationBasis, StoreError> {
    super::require_current_schema(database)?;
    let mut connection = super::open_read_only(database)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    let tx = connection.transaction()?;
    let basis = ObservationBasis {
        work_item_node_id: work_item.into(),
        objects: load(&tx, work_item)?,
        admitted_events: watermark(&tx, work_item)?,
    };
    tx.commit()?;
    Ok(basis)
}

pub(super) fn require_current_events(
    connection: &Connection,
    basis: &ObservationBasis,
    node: &str,
) -> Result<(), StoreError> {
    if basis.work_item_node_id != node {
        return Err(StoreError::InvalidData("observation belongs to a different object".into()));
    }
    let admitted = watermark(connection, node)?;
    if admitted != basis.admitted_events {
        return Err(StoreError::ObservationChanged {
            node: node.into(),
            expected: basis.admitted_events,
            admitted,
        });
    }
    Ok(())
}

fn watermark(connection: &Connection, work_item: &str) -> Result<i64, StoreError> {
    connection
        .query_row(
            "SELECT COALESCE(MAX(rowid),0) FROM events WHERE work_item_node_id=?1",
            [work_item],
            |r| r.get(0),
        )
        .map_err(Into::into)
}

/// The producer prepares classification outside `SQLite`. This compare-and-commit
/// either persists every resulting event and the new frontier, or neither.
pub(crate) struct ObservationCommit {
    pub basis: ObservationBasis,
    pub current: Vec<CanonicalObjectState>,
    pub events: Vec<IngressEvent>,
    pub policy: SchedulerPolicy,
}

pub(super) fn load(
    connection: &Connection,
    work_item: &str,
) -> Result<Vec<CanonicalObjectState>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT node_id,database_id,object_kind,version,digest,lifecycle,author_node_id,
                author_login,metadata_digest,title_digest,body_digest,title_event_id
         FROM processed_objects WHERE work_item_node_id=?1 ORDER BY node_id",
    )?;
    let rows =
        statement.query_map([work_item], |row| {
            let title: Option<String> = row.get(9)?;
            let body: Option<String> = row.get(10)?;
            let title_event_id = row.get(11)?;
            Ok(CanonicalObjectState {
                root_fields: title.zip(body).map(|(title_digest, body_digest)| {
                    ObservedRootFields { title_digest, body_digest, title_event_id }
                }),
                node_id: row.get(0)?,
                database_id: row.get(1)?,
                object_kind: row.get(2)?,
                version: row.get(3)?,
                digest: row.get(4)?,
                lifecycle: row.get(5)?,
                author_node_id: row.get(6)?,
                author_login: row.get(7)?,
                metadata_digest: row.get(8)?,
            })
        })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

// Observation, canonical frontier and synthesized events commit together.
#[allow(clippy::too_many_lines)]
pub(super) fn commit(
    database: &Path,
    observation: &ObservationCommit,
) -> Result<usize, StoreError> {
    super::require_current_schema(database)?;
    let mut connection = super::open_read_write(database)?;
    super::configure_connection(&connection)?;
    let tx = connection.transaction()?;
    let work_item = &observation.basis.work_item_node_id;
    let mut expected = observation.basis.objects.clone();
    expected.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    if load(&tx, work_item)? != expected {
        return Err(StoreError::InvalidData(
            "processed observation changed before commit; reread canonical state".into(),
        ));
    }
    require_current_events(&tx, &observation.basis, work_item)?;
    for object in &observation.current {
        if let Some(at) = comparable_timestamp(&object.version, &object.object_kind)
            && let Some(known) =
                latest_positive_version(&tx, work_item, &object.node_id, &object.object_kind)?
            && known.at > at
        {
            return Err(StoreError::ObservationConflict(Box::new(ObservationConflict {
                node: object.node_id.clone(),
                kind: object.object_kind.clone(),
                snapshot_version: object.version.clone(),
                snapshot_lifecycle: object.lifecycle.clone(),
                classified_version: known.version,
                classified_evidence: known.source,
            })));
        }
    }
    let mut changes = 0;
    for event in &observation.events {
        if event.work_item_node_id.as_deref() != Some(work_item) {
            return Err(StoreError::InvalidData(
                "observation event belongs to a different responsibility".into(),
            ));
        }
        if event.identity == super::ChangeIdentity::ObservedTransition {
            let node = event.object_node_id.as_deref();
            let before = expected.iter().find(|o| Some(o.node_id.as_str()) == node);
            let after = observation.current.iter().find(|o| Some(o.node_id.as_str()) == node);
            let valid = before.zip(after).is_some_and(|(before, after)| {
                before.object_kind == "review_thread"
                    && after.object_kind == "review_thread"
                    && before.lifecycle != after.lifecycle
                    && before.metadata_digest.is_some()
                    && before.metadata_digest == after.metadata_digest
                    && matches!(
                        (event.detail, after.lifecycle.as_str()),
                        (Some("resolved"), "resolved") | (Some("unresolved"), "active")
                    )
            });
            if !valid {
                return Err(StoreError::InvalidData(
                    "state receipt cannot acknowledge changed metadata or an unproven transition"
                        .into(),
                ));
            }
        }
        let result = super::ingest_event_on(
            &tx,
            event,
            observation.policy,
            super::Admission::CanonicalObservation,
        )?;
        changes += usize::from(result.event_id.is_some());
    }
    // Empty snapshots are not valid observations of an existing Work Item.
    if !observation.current.iter().any(|object| object.node_id == *work_item) {
        return Err(StoreError::InvalidData("complete observation has no root".into()));
    }
    tx.execute("DELETE FROM processed_objects WHERE work_item_node_id=?1", [work_item])?;
    for object in &observation.current {
        let root = object.root_fields.as_ref();
        tx.execute(
            "INSERT INTO processed_objects(work_item_node_id,node_id,database_id,object_kind,
               version,digest,lifecycle,author_node_id,author_login,metadata_digest,title_digest,
               body_digest,title_event_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                work_item,
                object.node_id,
                object.database_id,
                object.object_kind,
                object.version,
                object.digest,
                object.lifecycle,
                object.author_node_id,
                object.author_login,
                object.metadata_digest,
                root.map(|r| &r.title_digest),
                root.map(|r| &r.body_digest),
                root.and_then(|r| r.title_event_id.as_deref()),
            ],
        )?;
    }
    tx.commit()?;
    for object in &observation.current {
        if let Some(before) =
            expected.iter().find(|old| old.node_id == object.node_id && old.lifecycle == "deleted")
            && object.lifecycle != "deleted"
        {
            tracing::info!(object_node_id=%object.node_id,object_kind=%object.object_kind,previous_version=%before.version,current_version=%object.version,current_lifecycle=%object.lifecycle,"fresh canonical presence corrected an absence observation");
        }
    }
    Ok(changes)
}

// Ordering is based on upstream timestamps, not lifecycle suffixes, cache read
// time, or the lexical order of a review-thread projection.
fn comparable_timestamp(version: &str, kind: &str) -> Option<time::OffsetDateTime> {
    if kind == "review_thread" {
        return None;
    }
    let end = [":active", ":minimized", ":deleted", ":dismissed"]
        .iter()
        .filter_map(|suffix| version.find(suffix))
        .min()
        .unwrap_or(version.len());
    time::OffsetDateTime::parse(&version[..end], &time::format_description::well_known::Rfc3339)
        .ok()
}

struct ClassifiedVersion {
    at: time::OffsetDateTime,
    version: String,
    source: String,
}

fn latest_positive_version(
    connection: &Connection,
    work_item: &str,
    node: &str,
    kind: &str,
) -> Result<Option<ClassifiedVersion>, StoreError> {
    let processed: Option<(String,String)> = connection
        .query_row(
            "SELECT version,lifecycle FROM processed_objects WHERE work_item_node_id=?1 AND node_id=?2 AND lifecycle!='deleted'",
            params![work_item, node],
            |r| Ok((r.get(0)?,r.get(1)?)),
        )
        .optional()?;
    let mut latest = processed.and_then(|(version, lifecycle)| {
        comparable_timestamp(&version, kind).map(|at| ClassifiedVersion {
            at,
            version,
            source: format!("processed/{lifecycle}"),
        })
    });
    let root = matches!(kind, "issue" | "pr");
    // Absence/deletion observations have no upstream edit clock. In particular,
    // old projection tombstones contain local observation time, not updatedAt.
    let mut statement = connection.prepare("SELECT object_version,kind,detail FROM events WHERE object_node_id=?1 AND object_version IS NOT NULL AND lifecycle!='superseded' AND detail IS NOT 'deleted' AND (?2=0 OR kind='lifecycle' OR (kind IN ('invalidate','origin_echo') AND detail='edited'))")?;
    for row in statement.query_map(params![node, root], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))
    })? {
        let (version, event_kind, detail) = row?;
        if let Some(at) = comparable_timestamp(&version, kind)
            && latest.as_ref().is_none_or(|old| old.at < at)
        {
            latest = Some(ClassifiedVersion {
                at,
                version,
                source: format!("event/{event_kind}/{}", detail.as_deref().unwrap_or("none")),
            });
        }
    }
    Ok(latest)
}

pub(super) fn event_is_stale(
    connection: &Connection,
    event: &IngressEvent,
    kind: Option<&str>,
) -> Result<bool, StoreError> {
    let (Some(kind), Some(node), Some(version), Some(work_item)) = (
        kind,
        event.object_node_id.as_deref(),
        event.object_version.as_deref(),
        event.work_item_node_id.as_deref(),
    ) else {
        return Ok(false);
    };
    if event.detail != Some("deleted") && connection.query_row("SELECT EXISTS(SELECT 1 FROM processed_objects WHERE work_item_node_id=?1 AND node_id=?2 AND lifecycle='deleted')",params![work_item,node],|r|r.get::<_,bool>(0))? {
        // Only a fresh complete observation may restore an absent object.
        return Ok(true);
    }
    let Some(at) = comparable_timestamp(version, kind) else {
        return Ok(false);
    };
    Ok(latest_positive_version(connection, work_item, node, kind)?
        .is_some_and(|known| known.at > at))
}

pub(super) fn matching_latest_transition(
    connection: &Connection,
    event: &IngressEvent,
) -> Result<Option<String>, StoreError> {
    let (Some(node), Some(target)) =
        (event.object_node_id.as_deref(), event.work_item_node_id.as_deref())
    else {
        return Ok(None);
    };
    let latest: Option<(String,String,Option<String>)> = connection.query_row(
        "SELECT event_id,kind,detail FROM events WHERE object_node_id=?1 AND work_item_node_id=?2 AND lifecycle!='superseded' ORDER BY rowid DESC LIMIT 1",
        params![node,target],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).optional()?;
    Ok(latest.and_then(|(id, kind, detail)| {
        (detail.as_deref() == event.detail
            && (kind == event.kind.as_str() || kind == "origin_echo"))
            .then_some(id)
    }))
}

/// Hash exact published content and version, never presentation attribution.
pub(crate) fn publication_digest(
    node: &str,
    version: &str,
    title: Option<&str>,
    body: &str,
) -> String {
    let mut digest = Sha256::new();
    for value in ["braid-publication-v1", node, version, title.unwrap_or(""), body] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    hex::encode(digest.finalize())
}

pub(super) fn record_publication(
    database: &Path,
    intent: &str,
    node: &str,
    version: &str,
    digest: &str,
) -> Result<(), StoreError> {
    super::require_current_schema(database)?;
    let connection = super::open_read_write(database)?;
    super::configure_connection(&connection)?;
    connection.execute("INSERT OR IGNORE INTO publication_versions(node_id,version,digest,intent_id) VALUES(?1,?2,?3,?4)",params![node,version,digest,intent])?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VersionOrigin {
    External,
    Agent,
    Unknown,
}

pub(crate) struct NativeVersionEvidence {
    pub(crate) origin: VersionOrigin,
    pub(crate) prior_title: Option<String>,
    pub(crate) prior_body: Option<String>,
}

impl NativeVersionEvidence {
    pub(crate) fn unknown() -> Self {
        Self { origin: VersionOrigin::Unknown, prior_title: None, prior_body: None }
    }
}

#[derive(serde::Deserialize, Default)]
struct NativeEditPayload {
    #[serde(default)]
    changes: NativeEditChanges,
}

#[derive(serde::Deserialize, Default)]
struct NativeEditChanges {
    title: Option<NativeEditField>,
    body: Option<NativeEditField>,
}

#[derive(serde::Deserialize)]
struct NativeEditField {
    from: Option<String>,
}

/// Only received native events can supplement canonical actor evidence.
pub(super) fn native_version_evidence(
    connection: &Connection,
    node: &str,
    version: &str,
    digest: &str,
    action: &str,
) -> Result<NativeVersionEvidence, StoreError> {
    let origin: Option<(String, Vec<u8>)> = connection.query_row(
        "SELECT e.origin,d.raw_payload FROM events e JOIN deliveries d ON d.delivery_guid=e.delivery_guid
         WHERE e.object_node_id=?1 AND e.object_version=?2 AND e.body_digest=?3
           AND d.action=?4 AND e.lifecycle!='superseded' AND e.origin IN ('external','agent')
           AND d.event_name IN ('issues','pull_request','issue_comment','pull_request_review',
                                'pull_request_review_comment')
           AND d.delivery_guid NOT LIKE 'reconcile-%'
         ORDER BY e.origin='external' DESC LIMIT 1",
        params![node, version, digest, action],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    let Some((origin, payload)) = origin else { return Ok(NativeVersionEvidence::unknown()) };
    if origin == "external" {
        return Ok(NativeVersionEvidence {
            origin: VersionOrigin::External,
            ..NativeVersionEvidence::unknown()
        });
    }
    let changes = serde_json::from_slice::<NativeEditPayload>(&payload).unwrap_or_default().changes;
    Ok(NativeVersionEvidence {
        origin: VersionOrigin::Agent,
        prior_title: changes.title.and_then(|title| title.from),
        prior_body: changes.body.and_then(|body| body.from),
    })
}

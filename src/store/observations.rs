//! A processed observation is a delivery commitment, not a Context cache update.
use super::{CanonicalObjectState, IngressEvent, SchedulerPolicy, StoreError};
use rusqlite::{Connection, OptionalExtension as _, params};
use sha2::{Digest as _, Sha256};
use std::path::Path;

/// The producer prepares classification outside `SQLite`. This compare-and-commit
/// either persists every resulting event and the new frontier, or neither.
pub(crate) struct ObservationCommit {
    pub work_item_node_id: String,
    pub expected: Vec<CanonicalObjectState>,
    pub current: Vec<CanonicalObjectState>,
    pub events: Vec<IngressEvent>,
    pub policy: SchedulerPolicy,
}

pub(super) fn load(
    connection: &Connection,
    work_item: &str,
) -> Result<Vec<CanonicalObjectState>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT node_id,database_id,object_kind,version,digest,lifecycle,author_node_id,author_login,metadata_digest
         FROM processed_objects WHERE work_item_node_id=?1 ORDER BY node_id",
    )?;
    let rows = statement.query_map([work_item], |row| {
        Ok(CanonicalObjectState {
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

pub(super) fn commit(
    database: &Path,
    observation: &ObservationCommit,
) -> Result<usize, StoreError> {
    super::require_current_schema(database)?;
    let mut connection = super::open_read_write(database)?;
    super::configure_connection(&connection)?;
    let tx = connection.transaction()?;
    let mut expected = observation.expected.clone();
    expected.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    if load(&tx, &observation.work_item_node_id)? != expected {
        return Err(StoreError::InvalidData(
            "processed observation changed before commit; reread canonical state".into(),
        ));
    }
    for object in &observation.current {
        if let Some(at) = comparable_timestamp(&object.version, &object.object_kind)
            && latest_classified_time(
                &tx,
                &observation.work_item_node_id,
                &object.node_id,
                &object.object_kind,
            )?
            .is_some_and(|known| known > at)
        {
            return Err(StoreError::InvalidData(
                "complete observation predates an admitted object change; reread canonical state"
                    .into(),
            ));
        }
    }
    let mut changes = 0;
    for event in &observation.events {
        if event.work_item_node_id.as_deref() != Some(&observation.work_item_node_id) {
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
        let result = super::ingest_event_on(&tx, event, observation.policy)?;
        changes += usize::from(result.event_id.is_some());
    }
    // Empty snapshots are not valid observations of an existing Work Item.
    if !observation.current.iter().any(|object| object.node_id == observation.work_item_node_id) {
        return Err(StoreError::InvalidData("complete observation has no root".into()));
    }
    tx.execute(
        "DELETE FROM processed_objects WHERE work_item_node_id=?1",
        [&observation.work_item_node_id],
    )?;
    for object in &observation.current {
        tx.execute("INSERT INTO processed_objects(work_item_node_id,node_id,database_id,object_kind,version,digest,lifecycle,author_node_id,author_login,metadata_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![observation.work_item_node_id,object.node_id,object.database_id,object.object_kind,object.version,object.digest,object.lifecycle,object.author_node_id,object.author_login,object.metadata_digest])?;
    }
    tx.commit()?;
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

fn latest_classified_time(
    connection: &Connection,
    work_item: &str,
    node: &str,
    kind: &str,
) -> Result<Option<time::OffsetDateTime>, StoreError> {
    let processed: Option<String> = connection
        .query_row(
            "SELECT version FROM processed_objects WHERE work_item_node_id=?1 AND node_id=?2",
            params![work_item, node],
            |r| r.get(0),
        )
        .optional()?;
    let mut latest = processed.as_deref().and_then(|v| comparable_timestamp(v, kind));
    let root = matches!(kind, "issue" | "pr");
    let mut statement = connection.prepare("SELECT object_version FROM events WHERE object_node_id=?1 AND object_version IS NOT NULL AND lifecycle!='superseded' AND (?2=0 OR kind='lifecycle' OR (kind IN ('invalidate','origin_echo') AND detail='edited'))")?;
    for version in statement.query_map(params![node, root], |r| r.get::<_, String>(0))? {
        if let Some(at) = comparable_timestamp(&version?, kind) {
            latest = Some(latest.map_or(at, |old| old.max(at)));
        }
    }
    Ok(latest)
}

pub(super) fn event_is_stale(
    connection: &Connection,
    event: &IngressEvent,
    kind: Option<&str>,
) -> Result<bool, StoreError> {
    // A complete observation has a stronger, atomic precondition checked above;
    // partial webhook hints must not acknowledge it through a mutable cache.
    if event.delivery_guid.starts_with("reconcile-") {
        return Ok(false);
    }
    let (Some(kind), Some(node), Some(version), Some(work_item)) = (
        kind,
        event.object_node_id.as_deref(),
        event.object_version.as_deref(),
        event.work_item_node_id.as_deref(),
    ) else {
        return Ok(false);
    };
    let Some(at) = comparable_timestamp(version, kind) else {
        return Ok(false);
    };
    Ok(latest_classified_time(connection, work_item, node, kind)?.is_some_and(|known| known > at))
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

pub(super) fn is_agent_version(
    connection: &Connection,
    node: &str,
    version: &str,
    digest: &str,
) -> Result<bool, StoreError> {
    connection.query_row("SELECT EXISTS(SELECT 1 FROM publication_versions WHERE node_id=?1 AND version=?2 AND digest=?3) OR EXISTS(SELECT 1 FROM events WHERE object_node_id=?1 AND object_version=?2 AND body_digest=?3 AND origin='agent')",params![node,version,digest],|row| row.get(0)).map_err(Into::into)
}

/// PR existence and activation delivery are separate commitments. Only this
/// domain operation can bind the ensure plan to its durable activation event.
pub(super) fn accept_pr_activation(
    database: &Path,
    intent: &str,
    repository_node: &str,
    node: &str,
    number: u64,
    policy: SchedulerPolicy,
) -> Result<(), StoreError> {
    super::require_current_schema(database)?;
    let mut connection = super::open_read_write(database)?;
    super::configure_connection(&connection)?;
    let tx = connection.transaction()?;
    let (repository, request, comment, expected_node, expected_number, source): (String,String,i64,Option<String>,Option<i64>,Option<String>) = tx.query_row(
        "SELECT i.repository,w.public_request_id,i.comment_database_id,i.pull_request_node_id,i.pull_request_number,w.source_binding
         FROM implementation_requests i JOIN write_intents w ON w.intent_id=i.intent_id
         WHERE i.intent_id=?1 AND w.operation='pr_ensure'", [intent],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)),
    )?;
    if expected_node.as_deref() != Some(node)
        || expected_number != Some(super::sqlite_u64(number, "PR number")?)
    {
        return Err(StoreError::InvalidData(
            "activation target differs from frozen ensure plan".into(),
        ));
    }
    if let Some(source) = source {
        let source = serde_json::from_str(&source)
            .map_err(|e| StoreError::InvalidData(format!("invalid activation source: {e}")))?;
        super::publication_source::validate(&tx, &source)?;
    }
    let delivery = format!("braid-pr-ensure-v2-{request}");
    let event = IngressEvent {
        identity: super::ChangeIdentity::Occurrence,
        delivery_guid: delivery.clone(),
        event_name: "braid".into(),
        action: Some("pr_ensure".into()),
        repository_node_id: repository_node.into(),
        repository: repository.clone(),
        work_item_node_id: Some(node.into()),
        work_item_kind: Some("pr"),
        work_item_number: Some(number),
        work_item_state: Some("open".into()),
        object_node_id: Some(node.into()),
        object_version: Some(request),
        object_digest: None,
        visible_body: None,
        actor_node_id: None,
        actor_login: None,
        kind: super::EventKind::Assign,
        detail: Some("pr_ensure"),
        cross_surface_invalidation: false,
        requires_attention: true,
        origin: "braid",
        reference: format!(
            "Implementation Request comment {repository}#issuecomment-{comment} ensured GitHub PR {repository}#{number}."
        ),
        mention_candidate: false,
        reaction_target: None,
        known: true,
        raw_payload: Vec::new(),
    };
    super::ingest_event_on(&tx, &event, policy)?;
    let event_id: String = tx.query_row("SELECT event_id FROM events WHERE delivery_guid=?1 AND work_item_node_id=?2 AND kind='assign' AND detail='pr_ensure'",params![delivery,node],|r|r.get(0)).optional()?.ok_or_else(||StoreError::InvalidData("ensure activation was not durably accepted".into()))?;
    tx.execute("UPDATE implementation_requests SET activation_event_id=?2,updated_at=?3 WHERE intent_id=?1",params![intent,event_id,super::now_rfc3339()])?;
    tx.commit()?;
    Ok(())
}

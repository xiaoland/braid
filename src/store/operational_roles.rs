//! Identifies operational comments from canonical GitHub conversation evidence.
use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, Transaction, params};
use sha2::{Digest as _, Sha256};

use super::{
    Path, StoreError, configure_connection, deadline_rfc3339, open_read_only, open_read_write,
    require_current_schema,
};

const ROLE_MARKER_PREFIX: &str = "\n\n<!-- braid-operational-status:v1:";
const ROLE_MARKER_SUFFIX: &str = " -->";

pub(crate) struct OperationalCommentCandidate {
    node_id: String,
    body: String,
    created_at: String,
}

impl OperationalCommentCandidate {
    pub(crate) fn from_discussion(
        trusted_app: &str,
        author_node_id: Option<&str>,
        node_id: &str,
        body: Option<&str>,
        created_at: &str,
    ) -> Option<Self> {
        if author_node_id != Some(trusted_app) {
            return None;
        }
        Some(Self { node_id: node_id.into(), body: body?.into(), created_at: created_at.into() })
    }
}

/// Declare each actual rendered version before it can be sent to GitHub.
pub(super) fn declare(
    tx: &Transaction<'_>,
    work_item_node_id: &str,
    profile_id: &str,
    generation: i64,
    body: &str,
) -> Result<String, StoreError> {
    tx.execute(
        "INSERT INTO operational_comment_roles
            (role_id,work_item_node_id,profile_id,assignment_generation)
         VALUES(lower(hex(randomblob(16))),?1,?2,?3)
         ON CONFLICT(work_item_node_id,profile_id,assignment_generation) DO NOTHING",
        params![work_item_node_id, profile_id, generation],
    )?;
    let role_id: String = tx.query_row(
        "SELECT role_id FROM operational_comment_roles
         WHERE work_item_node_id=?1 AND profile_id=?2 AND assignment_generation=?3",
        params![work_item_node_id, profile_id, generation],
        |row| row.get(0),
    )?;
    let marked = format!("{body}{ROLE_MARKER_PREFIX}{role_id}{ROLE_MARKER_SUFFIX}");
    tx.execute(
        "INSERT INTO operational_comment_role_bodies (role_id,body_digest) VALUES(?1,?2)
         ON CONFLICT DO NOTHING",
        params![role_id, body_digest(&marked)],
    )?;
    Ok(marked)
}

pub(super) fn reconcile(
    database: &Path,
    work_item_node_id: &str,
    candidates: &[OperationalCommentCandidate],
) -> Result<BTreeSet<String>, StoreError> {
    require_current_schema(database)?;
    if candidates.is_empty() {
        return known_ids(&open_read_only(database)?, work_item_node_id);
    }
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let tx = connection.transaction()?;
    claim_candidates(&tx, work_item_node_id, candidates)?;
    let ids = known_ids(&tx, work_item_node_id)?;
    tx.commit()?;
    Ok(ids)
}

fn known_ids(db: &Connection, work_item_node_id: &str) -> Result<BTreeSet<String>, StoreError> {
    let mut statement = db.prepare(
        "SELECT comment_node_id FROM operational_comment_roles
         WHERE work_item_node_id=?1 AND comment_node_id IS NOT NULL",
    )?;
    let rows = statement.query_map([work_item_node_id], |row| row.get(0))?;
    rows.collect::<Result<_, _>>().map_err(StoreError::from)
}

fn claim_candidates(
    tx: &Transaction<'_>,
    work_item_node_id: &str,
    candidates: &[OperationalCommentCandidate],
) -> Result<(), StoreError> {
    let mut matching = tx.prepare(
        "SELECT r.role_id FROM operational_comment_roles r
         WHERE r.work_item_node_id=?1 AND r.comment_node_id IS NULL AND r.ambiguous=0
           AND NOT EXISTS (SELECT 1 FROM operational_comment_roles seen WHERE seen.comment_node_id=?2)
           AND ((r.role_id=?3 AND EXISTS (
                  SELECT 1 FROM operational_comment_role_bodies b
                  WHERE b.role_id=r.role_id AND b.body_digest=?4))
                OR EXISTS (
                  SELECT 1 FROM legacy_operational_comment_bodies old
                  WHERE old.role_id=r.role_id AND old.body=?5
                    AND unixepoch(?6) BETWEEN unixepoch(old.not_before) AND unixepoch(?7)))",
    )?;
    let recovery_until = deadline_rfc3339(5)?;
    let mut candidates_by_role = BTreeMap::<String, BTreeSet<&str>>::new();
    let mut roles_by_candidate = BTreeMap::<&str, BTreeSet<String>>::new();
    for candidate in candidates {
        let marker = role_marker(&candidate.body);
        let rows = matching.query_map(
            params![
                work_item_node_id,
                candidate.node_id,
                marker,
                marker.map(|_| body_digest(&candidate.body)),
                candidate.body,
                candidate.created_at,
                recovery_until,
            ],
            |row| row.get::<_, String>(0),
        )?;
        for role in rows {
            let role = role?;
            candidates_by_role.entry(role.clone()).or_default().insert(&candidate.node_id);
            roles_by_candidate.entry(&candidate.node_id).or_default().insert(role);
        }
    }
    for (role_id, nodes) in &candidates_by_role {
        // Both directions must be unique, including old requests with identical bodies.
        let node = nodes.first().copied().filter(|node| {
            nodes.len() == 1 && roles_by_candidate.get(node).is_some_and(|roles| roles.len() == 1)
        });
        tx.execute(
            "UPDATE operational_comment_roles SET comment_node_id=?2,ambiguous=?3 WHERE role_id=?1",
            params![role_id, node, node.is_none()],
        )?;
        tx.execute("DELETE FROM legacy_operational_comment_bodies WHERE role_id=?1", [role_id])?;
    }
    Ok(())
}

fn role_marker(body: &str) -> Option<&str> {
    let (_, suffix) = body.rsplit_once(ROLE_MARKER_PREFIX)?;
    let role_id = suffix.strip_suffix(ROLE_MARKER_SUFFIX)?;
    (role_id.len() == 32 && role_id.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(role_id)
}

fn body_digest(body: &str) -> String {
    hex::encode(Sha256::digest(body.as_bytes()))
}

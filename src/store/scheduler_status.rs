//! Read-only scheduler diagnostics. Provider physical availability is not a ledger fact.
use super::{Serialize, StoreError};

// Keep the durable part of claim selection and status eligibility identical.
pub(super) const DURABLE_TURN_JOINS: &str = "
 JOIN assignments a ON a.work_item_node_id=w.node_id
   AND a.lifecycle IN ('active','finalizing')
 JOIN agent_instances ai ON ai.assignment_id=a.assignment_id
   AND ((a.lifecycle='active' AND ai.lifecycle='idle')
        OR (a.lifecycle='finalizing' AND ai.lifecycle='finalizing'))
 JOIN provider_sessions ps ON ps.agent_id=ai.agent_id AND ps.lifecycle='idle'";

#[derive(Debug, Clone, Serialize)]
pub struct BatchEligibility {
    pub durable_eligible: bool,
    pub ineligible_reason: Option<&'static str>,
    pub provider_availability: &'static str,
}

pub(super) fn batch_eligibility(
    connection: &rusqlite::Connection,
    batch: &str,
) -> Result<BatchEligibility, StoreError> {
    let eligible: bool = connection.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM wake_batches b
         JOIN work_items w ON w.node_id=b.work_item_node_id {DURABLE_TURN_JOINS}
         WHERE b.batch_id=?1 AND b.lifecycle='runnable')"
        ),
        [batch],
        |r| r.get(0),
    )?;
    let reason = if eligible {
        None
    } else {
        let (runnable, assignment, agent): (bool, bool, bool) = connection.query_row(
            "SELECT b.lifecycle='runnable',
             EXISTS(SELECT 1 FROM assignments a WHERE a.work_item_node_id=b.work_item_node_id AND a.lifecycle IN ('active','finalizing')),
             EXISTS(SELECT 1 FROM assignments a JOIN agent_instances ai ON ai.assignment_id=a.assignment_id
                WHERE a.work_item_node_id=b.work_item_node_id AND
                ((a.lifecycle='active' AND ai.lifecycle='idle') OR (a.lifecycle='finalizing' AND ai.lifecycle='finalizing')))
             FROM wake_batches b WHERE b.batch_id=?1", [batch], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        Some(if !runnable {
            "batch_not_runnable"
        } else if !assignment {
            "no_active_or_finalizing_assignment"
        } else if !agent {
            "no_matching_idle_or_finalizing_agent"
        } else {
            "no_idle_provider_session"
        })
    };
    Ok(BatchEligibility {
        durable_eligible: eligible,
        ineligible_reason: reason,
        provider_availability: "unknown_not_in_database",
    })
}

#[cfg(test)]
mod tests {
    use super::super::{StoreActor, claim_runnable_turn};
    use rusqlite::Connection;

    #[test]
    fn status_distinguishes_generations_terminal_residue_and_finalization() {
        let dir =
            std::env::temp_dir().join(format!("braid-scheduler-status-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let database = dir.join("state.sqlite3");
        let store = StoreActor::start(database.clone(), dir.join("backups")).unwrap();
        store.apply().unwrap();
        let conn = Connection::open(&database).unwrap();
        conn.execute_batch("
          INSERT INTO repositories VALUES('R','owner/repo',1,'now');
          INSERT INTO work_items(node_id,repository_node_id,kind,number,state,context_revision,observed_at) VALUES('W','R','issue',1,'CLOSED','revision','now');
          INSERT INTO profiles VALUES('p',1,printf('%064d',0),'pi','[]');
          INSERT INTO assignments VALUES('old','W',1,'blocked','1',NULL);
          INSERT INTO assignments VALUES('current','W',2,'active','2',NULL);
          INSERT INTO agent_instances(agent_id,assignment_id,profile_id,profile_revision,role,lifecycle) VALUES('old-agent','old','p',1,'issue','blocked'),('current-agent','current','p',1,'issue','idle');
          INSERT INTO provider_sessions(session_id,agent_id,provider_kind,provider_session_id,context_revision,instruction_revision,lifecycle,started_at) VALUES('s','current-agent','pi','physical','revision','1','idle','now');
          INSERT INTO wake_batches(batch_id,work_item_node_id,context_revision,event_count,quiet_deadline,urgent,lifecycle,created_at,updated_at) VALUES('b','W','revision',0,'now',0,'runnable','now','now');
        ").unwrap();
        let status = store.runtime_status().unwrap();
        assert_eq!(status.runnable_batches, 1);
        assert_eq!(status.durable_eligible_batches, 1);
        assert!(!status.agent_groups[0].current_generation);
        assert_eq!(status.agent_groups[0].assignment_lifecycle, "blocked");
        assert!(status.agent_groups[1].current_generation);
        assert_eq!(status.agent_groups[1].session_lifecycle.as_deref(), Some("idle"));
        assert_eq!(status.batches[0].scheduler.provider_availability, "unknown_not_in_database");
        // DB eligibility does not claim a physically unavailable provider.
        assert!(claim_runnable_turn(&database, "issue", "p", &[]).unwrap().is_none());
        conn.execute(
            "UPDATE assignments SET lifecycle='completed' WHERE assignment_id='current'",
            [],
        )
        .unwrap();
        let status = store.runtime_status().unwrap();
        assert_eq!(status.runnable_batches, 1);
        assert_eq!(status.durable_eligible_batches, 0);
        assert_eq!(
            status.batches[0].scheduler.ineligible_reason,
            Some("no_active_or_finalizing_assignment")
        );
        conn.execute(
            "UPDATE assignments SET lifecycle='finalizing' WHERE assignment_id='current'",
            [],
        )
        .unwrap();
        assert_eq!(
            store.runtime_status().unwrap().batches[0].scheduler.ineligible_reason,
            Some("no_matching_idle_or_finalizing_agent")
        );
        conn.execute(
            "UPDATE agent_instances SET lifecycle='finalizing' WHERE agent_id='current-agent'",
            [],
        )
        .unwrap();
        assert_eq!(store.runtime_status().unwrap().durable_eligible_batches, 1);
        conn.execute("UPDATE provider_sessions SET lifecycle='running'", []).unwrap();
        assert_eq!(
            store.runtime_status().unwrap().batches[0].scheduler.ineligible_reason,
            Some("no_idle_provider_session")
        );
        conn.execute("UPDATE provider_sessions SET lifecycle='idle'", []).unwrap();
        let claim =
            claim_runnable_turn(&database, "issue", "p", &["physical".into()]).unwrap().unwrap();
        assert_eq!(claim.trigger_kind, "finalization");
        assert_eq!(store.runtime_status().unwrap().runnable_batches, 0);
        drop(conn);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

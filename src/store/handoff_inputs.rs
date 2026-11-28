//! Durable facts used to consume an accepted handoff with current canonical Context.
use rusqlite::{Transaction, params};
use std::path::Path;

use super::{SchedulerPolicy, StoreError, TurnClaim, now_rfc3339, open_read_only, schedule_event};

#[derive(Clone, Copy)]
pub(super) struct NewHandoffInput<'a> {
    pub(super) source_repository: &'a str,
    pub(super) source_number: i64,
    pub(super) target_work_item: &'a str,
    pub(super) target_assignment: &'a str,
    pub(super) target_generation: i64,
}

pub(super) fn accept(
    tx: &Transaction<'_>,
    input: NewHandoffInput<'_>,
    policy: SchedulerPolicy,
) -> Result<String, StoreError> {
    let event = uuid::Uuid::now_v7().to_string();
    let now = now_rfc3339();
    let repository = input.source_repository;
    let number = input.source_number;
    let reference = format!(
        "Explicit Issue handoff: {repository}#{number}; read current canonical design and discussion"
    );
    tx.execute(
        "INSERT INTO events(event_id,work_item_node_id,kind,detail,origin,reference,lifecycle,
                            observed_at,next_turn_only)
         VALUES (?1,?2,'wake','pr_handoff','internal',?3,'pending',?4,1)",
        params![event, input.target_work_item, reference, now],
    )?;
    tx.execute(
        "INSERT INTO handoff_inputs(event_id,source_repository,source_number,
                                    target_assignment_id,target_generation)
         VALUES (?1,?2,?3,?4,?5)",
        params![event, repository, number, input.target_assignment, input.target_generation],
    )?;
    schedule_event(tx, input.target_work_item, &event, policy, false, &now)?;
    Ok(event)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetirementReason {
    SourceLocatorUnavailable,
    TargetGenerationChanged,
    TargetClosed,
    SourceNotOpenAssociated,
}

#[derive(Debug, Clone)]
pub(crate) struct RetiredInput {
    pub event_id: String,
    pub reason: RetirementReason,
}

pub(crate) struct BatchSource {
    pub event_id: String,
    pub repository: String,
    pub number: u64,
}

#[derive(Default)]
pub(crate) struct BatchInputs {
    pub sources: Vec<BatchSource>,
    pub retired: Vec<RetiredInput>,
}

/// The historical source locator authorizes a fresh canonical association check.
/// The target remains pinned to the starting turn's original assignment generation.
pub(crate) fn for_turn(database: &Path, claim: &TurnClaim) -> Result<BatchInputs, StoreError> {
    let connection = open_read_only(database)?;
    let mut statement = connection.prepare(
        "SELECT e.event_id,h.source_repository,h.source_number,
                EXISTS (
                    SELECT 1 FROM assignments a
                    JOIN agent_instances ai ON ai.assignment_id=a.assignment_id
                    JOIN provider_sessions ps ON ps.agent_id=ai.agent_id
                    JOIN turns t ON t.session_id=ps.session_id
                    WHERE a.assignment_id=h.target_assignment_id
                      AND a.generation=h.target_generation
                      AND a.work_item_node_id=e.work_item_node_id
                      AND t.turn_id=?2 AND t.batch_id=be.batch_id AND t.lifecycle='starting'
                      AND NOT EXISTS (
                          SELECT 1 FROM assignments newer
                          WHERE newer.work_item_node_id=a.work_item_node_id
                            AND newer.generation>a.generation))
         FROM wake_batch_events be JOIN events e ON e.event_id=be.event_id
         LEFT JOIN handoff_inputs h ON h.event_id=e.event_id
         WHERE be.batch_id=?1 AND e.kind='wake' AND e.detail='pr_handoff'
         ORDER BY be.ordinal",
    )?;
    let mut rows = statement.query(params![claim.batch_id, claim.turn_id])?;
    let mut inputs = BatchInputs::default();
    while let Some(row) = rows.next()? {
        let event_id = row.get(0)?;
        let repository = row.get::<_, Option<String>>(1)?;
        let number = row.get::<_, Option<i64>>(2)?;
        let current_generation: bool = row.get(3)?;
        let reason = match (repository, number) {
            (Some(repository), Some(number)) if current_generation => {
                inputs.sources.push(BatchSource {
                    event_id,
                    repository,
                    number: super::sqlite_i64_to_u64(number, "handoff source number")?,
                });
                continue;
            }
            (Some(_), Some(_)) => RetirementReason::TargetGenerationChanged,
            _ => RetirementReason::SourceLocatorUnavailable,
        };
        inputs.retired.push(RetiredInput { event_id, reason });
    }
    Ok(inputs)
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use crate::store::{MIGRATIONS, StoreActor, apply_one, configure_connection, open_read_write};

    #[test]
    fn migrates_accepted_handoffs_and_retires_unrecoverable_legacy_inputs() {
        let root =
            std::env::temp_dir().join(format!("braid-handoff-upgrade-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let database = root.join("state.sqlite3");
        let mut db = open_read_write(&database).unwrap();
        configure_connection(&db).unwrap();
        for migration in MIGRATIONS.iter().filter(|entry| entry.version <= 18) {
            apply_one(&mut db, migration).unwrap();
        }
        db.execute_batch(
            "INSERT INTO repositories VALUES ('repo','owner/repository',1,'now');
             INSERT INTO work_items(node_id,repository_node_id,kind,number,state,observed_at)
             VALUES ('issue','repo','issue',17,'open','now'),('pr','repo','pr',93,'open','now');
             INSERT INTO assignments VALUES
               ('source','issue',1,'retired','now','now'),('target','pr',2,'active','now',NULL);
             INSERT INTO profiles VALUES ('profile',1,hex(zeroblob(32)),'codex','pr');
             INSERT INTO agent_instances(agent_id,assignment_id,profile_id,profile_revision,role,lifecycle)
             VALUES('agent','target','profile',1,'pr_implementation_agent','idle');
             INSERT INTO provider_sessions(session_id,agent_id,provider_kind,provider_session_id,
                                           context_revision,instruction_revision,lifecycle,started_at)
             VALUES ('session','agent','codex','provider','context','instructions','idle','now');
             INSERT INTO wake_batches(batch_id,work_item_node_id,quiet_deadline,urgent,
                                      event_count,lifecycle,created_at,updated_at)
             VALUES ('batch','pr','now',1,3,'runnable','now','now');
             INSERT INTO events(event_id,work_item_node_id,kind,detail,origin,reference,lifecycle,observed_at)
             VALUES ('valid','pr','wake','pr_handoff','internal','Legacy accepted handoff','pending','now'),
                    ('damaged','pr','wake','pr_handoff','internal','Undeliverable handoff','pending','now'),
                    ('unlinked','pr','wake','pr_handoff','internal','Unlinked handoff','pending','now');
             INSERT INTO wake_batch_events VALUES ('batch','valid',0),('batch','damaged',1),('batch','unlinked',2);
             INSERT INTO write_intents(intent_id,operation,request_digest,lifecycle,created_at,
                                       updated_at,repository,source_binding)
             VALUES ('valid','pr_handoff',hex(zeroblob(32)),'applied','now','now','owner/repository',
                     '{\"assignment_id\":\"source\"}'),
                    ('damaged','pr_handoff',hex(zeroblob(32)),'applied','now','now','owner/repository','{'),
                    ('pending','pr_handoff',hex(zeroblob(32)),'pending','now','now','owner/repository',NULL);
             INSERT INTO pr_handoffs VALUES ('valid','target','valid'),('damaged','target','damaged'),
                                            ('pending','target',NULL);",
        ).unwrap();
        drop(db);

        let store = StoreActor::start(database.clone(), root.join("backups")).unwrap();
        assert_eq!(
            store.apply().unwrap().applied,
            (19..=crate::store::DATABASE_SCHEMA_VERSION).collect::<Vec<_>>()
        );
        let db = open_read_write(&database).unwrap();
        let pending_target: String = db.query_row(
            "SELECT target_assignment_id FROM pr_handoffs WHERE intent_id='pending' AND event_id IS NULL",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(pending_target, "target");
        let advanced = store
            .claim_runnable_turn("pr".into(), "profile".into(), vec!["provider".into()])
            .unwrap()
            .unwrap();
        let handoffs = for_turn(&database, &advanced).unwrap();
        assert_eq!(handoffs.sources.len(), 1);
        assert_eq!(handoffs.sources[0].repository, "owner/repository");
        assert_eq!(handoffs.sources[0].number, 17);
        assert_eq!(handoffs.retired.len(), 2);
        assert!(
            handoffs
                .retired
                .iter()
                .all(|input| input.reason == RetirementReason::SourceLocatorUnavailable)
        );
        let surviving = store.retire_handoff_inputs(advanced, handoffs.retired).unwrap().unwrap();
        assert_eq!(surviving.references, ["Legacy accepted handoff"]);
        let retired: i64 = db.query_row(
            "SELECT count(*) FROM events WHERE event_id IN ('damaged','unlinked') AND lifecycle='consumed'
             AND next_turn_only=1", [], |row| row.get(0),
        ).unwrap();
        assert_eq!(retired, 2);
        let violations: i64 = db
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(violations, 0);
        drop(db);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}

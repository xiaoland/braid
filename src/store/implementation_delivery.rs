//! Complete an already published implementation's durable responsibility handover.
use super::{
    ChangeIdentity, EventKind, IngressEvent, ObservationBasis, RuntimeLease, SchedulerPolicy,
    StoreError,
};
use rusqlite::{Connection, OptionalExtension as _, params};
use std::path::Path;

pub(crate) struct PrActivation {
    pub intent_id: String,
    pub repository_node: String,
    pub pr_node: String,
    pub pr_number: u64,
    pub policy: SchedulerPolicy,
    pub owner: RuntimeLease,
    pub issue_basis: ObservationBasis,
    pub target_basis: ObservationBasis,
}
struct Plan {
    root: String,
    repository: String,
    request: String,
    comment: i64,
    issue: String,
    profile: String,
    node: Option<String>,
    number: Option<i64>,
    event: Option<String>,
}
fn plan(connection: &Connection, intent: &str) -> Result<Plan, StoreError> {
    connection.query_row("SELECT root.intent_id,root.repository,w.public_request_id,root.comment_database_id,root.issue_node_id,root.pr_profile_id,root.pull_request_node_id,root.pull_request_number,root.activation_event_id FROM implementation_requests i JOIN implementation_requests root ON root.intent_id=COALESCE(i.root_intent_id,i.intent_id) JOIN write_intents w ON w.intent_id=root.intent_id WHERE i.intent_id=?1 AND w.operation='pr_ensure'",[intent],|r|Ok(Plan{root:r.get(0)?,repository:r.get(1)?,request:r.get(2)?,comment:r.get(3)?,issue:r.get(4)?,profile:r.get(5)?,node:r.get(6)?,number:r.get(7)?,event:r.get(8)?})).map_err(Into::into)
}
fn delivery_event(
    connection: &rusqlite::Transaction<'_>,
    plan: &Plan,
    activation: &PrActivation,
) -> Result<String, StoreError> {
    if let Some(event) = &plan.event {
        let valid:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE event_id=?1 AND work_item_node_id=?2 AND ((kind='assign' AND detail='pr_ensure' AND (object_version=?3 OR delivery_guid=?4)) OR (kind='noop' AND detail='pr_ensure_adopted' AND object_version=?3)))",params![event,activation.pr_node,plan.request,format!("braid-pr-ensure-{}-{}",plan.repository,plan.comment)],|r|r.get(0))?;
        if !valid {
            return Err(StoreError::InvalidData(
                "activation receipt does not match the implementation request and PR".into(),
            ));
        }
        return Ok(event.clone());
    }
    let existing:Option<(String,Option<String>,Option<String>)>=connection.query_row("SELECT a.lifecycle,g.profile_id,g.role FROM assignments a LEFT JOIN agent_instances g ON g.assignment_id=a.assignment_id WHERE a.work_item_node_id=?1 ORDER BY a.generation DESC LIMIT 1",[&activation.pr_node],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let adopted = if let Some((state, profile, role)) = existing {
        if !matches!(state.as_str(), "active" | "materializing")
            || profile.as_deref() != Some(plan.profile.as_str())
            || role.as_deref() != Some("pr_implementation_agent")
        {
            return Err(StoreError::InvalidData(format!(
                "existing PR assignment is {state}; ensure delivery cannot unblock or replace its generation"
            )));
        }
        true
    } else {
        false
    };
    let delivery = format!("braid-pr-ensure-v2-{}", plan.request);
    let event = IngressEvent {
        identity: ChangeIdentity::Occurrence,
        delivery_guid: delivery.clone(),
        event_name: "braid".into(),
        action: Some("pr_ensure".into()),
        repository_node_id: activation.repository_node.clone(),
        repository: plan.repository.clone(),
        work_item_node_id: Some(activation.pr_node.clone()),
        work_item_kind: Some("pr"),
        work_item_number: Some(activation.pr_number),
        work_item_state: Some("open".into()),
        object_node_id: Some(activation.pr_node.clone()),
        object_version: Some(plan.request.clone()),
        object_digest: None,
        visible_body: None,
        actor_node_id: None,
        actor_login: None,
        kind: if adopted { EventKind::Noop } else { EventKind::Assign },
        detail: Some(if adopted { "pr_ensure_adopted" } else { "pr_ensure" }),
        cross_surface_invalidation: false,
        requires_attention: !adopted,
        origin: "braid",
        reference: format!(
            "Implementation Request comment {}#issuecomment-{} ensured GitHub PR {}#{}.",
            plan.repository, plan.comment, plan.repository, activation.pr_number
        ),
        mention_candidate: false,
        reaction_target: None,
        known: true,
        raw_payload: Vec::new(),
    };
    super::ingest_event_on(connection, &event, activation.policy, super::Admission::Event)?;
    connection.query_row("SELECT event_id FROM events WHERE delivery_guid=?1 AND work_item_node_id=?2 AND object_version=?3 AND detail=?4",params![delivery,activation.pr_node,plan.request,event.detail],|r|r.get(0)).optional()?.ok_or_else(||StoreError::InvalidData("ensure activation was not durably accepted".into()))
}

pub(super) fn accept(database: &Path, activation: &PrActivation) -> Result<(), StoreError> {
    super::require_current_schema(database)?;
    let mut connection = super::open_read_write(database)?;
    super::configure_connection(&connection)?;
    let tx = connection.transaction()?;
    super::gh_writes::require_owner(&tx, &activation.owner)?;
    let plan = plan(&tx, &activation.intent_id)?;
    if plan.node.as_deref() != Some(&activation.pr_node)
        || plan.number != Some(super::sqlite_u64(activation.pr_number, "PR number")?)
    {
        return Err(StoreError::InvalidData(
            "activation target differs from frozen ensure plan".into(),
        ));
    }
    super::observations::require_current_events(&tx, &activation.issue_basis, &plan.issue)?;
    super::observations::require_current_events(
        &tx,
        &activation.target_basis,
        &activation.pr_node,
    )?;
    let event = delivery_event(&tx, &plan, activation)?;
    tx.execute("UPDATE implementation_requests SET activation_event_id=?2,stage='activation_pending',updated_at=?3 WHERE intent_id IN (?1,?4)",params![plan.root,event,super::now_rfc3339(),activation.intent_id])?;
    if activation.intent_id != plan.root {
        tx.execute("UPDATE write_intents SET lifecycle='applied',last_error=NULL,updated_at=?2 WHERE intent_id=?1",params![plan.root,super::now_rfc3339()])?;
        tx.execute("UPDATE github_write_outbox SET lifecycle='applied',last_error=NULL,updated_at=?2 WHERE intent_id=?1 AND lifecycle!='sending'",params![plan.root,super::now_rfc3339()])?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{MIGRATIONS, StoreActor, apply_one, configure_connection, open_read_write};
    use rusqlite::params;
    #[test]
    fn upgrade_requeues_only_pinned_missing_delivery_without_touching_live_roles() {
        let root = std::env::temp_dir()
            .join(format!("braid-implementation-upgrade-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let database = root.join("state.sqlite3");
        let mut db = open_read_write(&database).unwrap();
        configure_connection(&db).unwrap();
        for migration in MIGRATIONS.iter().filter(|m| m.version <= 11) {
            apply_one(&mut db, migration).unwrap();
        }
        db.execute_batch("INSERT INTO repositories(node_id,name_with_owner,observed_at) VALUES('repo','owner/repository','now'); INSERT INTO work_items(node_id,repository_node_id,kind,number,state,observed_at) VALUES('pr','repo','pr',9,'open','now'); INSERT INTO events(event_id,work_item_node_id,kind,detail,origin,reference,lifecycle,observed_at) VALUES('delivered','pr','assign','pr_ensure','braid','existing delivery','consumed','now');").unwrap();
        for (id, number, stage, pin, delivered) in [
            ("missing", 7, "activation_pending", true, false),
            ("complete", 8, "activation_pending", true, true),
            ("unpublished", 9, "planned", false, false),
        ] {
            db.execute("INSERT INTO write_intents(intent_id,operation,request_digest,lifecycle,created_at,updated_at,request_key,repository,target,profile_id,role,payload,public_request_id,remote_node_id) VALUES(?1,'pr_ensure',?2,'applied','now','now',?1,'owner/repository','request','pr-profile','Operator','{}',?3,'pr')",params![id,"a".repeat(64),format!("request-{id}")]).unwrap();
            db.execute("INSERT INTO implementation_requests(intent_id,repository,comment_database_id,comment_node_id,issue_node_id,issue_number,issue_title,base_ref,head_ref,pr_profile_id,bootstrap_authored_at,stage,pull_request_node_id,pull_request_number,activation_event_id,updated_at) VALUES(?1,'owner/repository',?2,'comment','issue',1,'Issue','main',?1,'pr-profile','now',?3,?4,?5,?6,'now')",params![id,number,stage,pin.then_some("pr"),pin.then_some(9),delivered.then_some("delivered")]).unwrap();
        }
        drop(db);
        let store = StoreActor::start(database.clone(), root.join("backups")).unwrap();
        let result = store.apply().unwrap();
        assert_eq!(result.applied, vec![12, 13, 14]);
        assert!(result.backup.is_some());
        let db = open_read_write(&database).unwrap();
        for (id, expected) in
            [("missing", "pending"), ("complete", "applied"), ("unpublished", "applied")]
        {
            let state: String = db
                .query_row("SELECT lifecycle FROM write_intents WHERE intent_id=?1", [id], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(state, expected);
        }
        assert_eq!(
            db.query_row("SELECT count(*) FROM github_write_outbox", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM assignments", [], |r| r.get::<_, i64>(0)).unwrap(),
            0,
            "migration only restores the original queue obligation"
        );
        assert_eq!(
            db.query_row(
                "SELECT remote_node_id FROM write_intents WHERE intent_id='missing'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "pr"
        );
        assert_eq!(
            db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)).unwrap(),
            "ok"
        );
        drop(db);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}

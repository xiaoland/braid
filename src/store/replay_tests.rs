use super::{
    MIGRATIONS, StoreActor, apply_one, batch_references, configure_connection, open_read_write,
};
use sha2::{Digest as _, Sha256};

#[test]
// Keep the old database, its restored backup and the replayed history in one journey.
#[allow(clippy::too_many_lines)]
fn schema13_unknown_turn_replays_input_without_erasing_history_or_duplicating_reactions() {
    let root = std::env::temp_dir().join(format!("braid-replay-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    let database = root.join("state.sqlite3");
    let mut db = open_read_write(&database).unwrap();
    configure_connection(&db).unwrap();
    for migration in MIGRATIONS.iter().filter(|m| m.version <= 13) {
        apply_one(&mut db, migration).unwrap();
    }
    db.execute_batch(
        "INSERT INTO repositories VALUES('repo','owner/repo',1,'now');
         INSERT INTO work_items(node_id,repository_node_id,kind,number,state,context_revision,observed_at)
         VALUES('item','repo','issue',1,'open','revision','now');
         INSERT INTO profiles VALUES('profile',1,printf('%064d',0),'codex','issue');
         INSERT INTO assignments VALUES('assignment','item',1,'active','now',NULL);
         INSERT INTO agent_instances(agent_id,assignment_id,profile_id,profile_revision,role,lifecycle)
         VALUES('agent','assignment','profile',1,'issue_agent','idle');
         INSERT INTO provider_sessions(session_id,agent_id,provider_kind,provider_session_id,context_revision,instruction_revision,lifecycle,started_at)
         VALUES('session','agent','codex','old-physical','revision','instructions','running','now');
         INSERT INTO events(event_id,work_item_node_id,object_node_id,kind,origin,reference,lifecycle,observed_at,mention_candidate,trusted_mention,requires_attention)
         VALUES('event','item','comment','wake','external','Review the revised design','pending','now',1,1,1);
         INSERT INTO wake_batches(batch_id,work_item_node_id,event_count,quiet_deadline,urgent,lifecycle,created_at,updated_at)
         VALUES('original','item',1,'now',1,'consumed','now','now');
         INSERT INTO wake_batch_events VALUES('original','event',0);
         INSERT INTO turns(turn_id,session_id,provider_turn_id,context_revision,trigger_kind,lifecycle,batch_id)
         VALUES('turn','session','old-turn','revision','trusted_mention','running','original');
         INSERT INTO github_write_outbox(intent_id,event_id,repository,target_kind,target_database_id,operation,content,request_digest,lifecycle,next_attempt_at,created_at,updated_at)
         VALUES('eyes','event','owner/repo','issue_comment','7','reaction_add','eyes',printf('%064d',0),'pending','now','now','now');",
    )
    .unwrap();
    let rocket_digest =
        hex::encode(Sha256::digest(["owner/repo", "issue_comment", "7", "rocket"].join("\0")));
    db.execute(
        "INSERT INTO github_write_outbox(intent_id,repository,target_kind,target_database_id,operation,content,request_digest,lifecycle,next_attempt_at,created_at,updated_at)
         VALUES('rocket','owner/repo','issue_comment','7','reaction_add','rocket',?1,'pending','now','now','now')",
        [rocket_digest],
    ).unwrap();
    drop(db);
    let store = StoreActor::start(database.clone(), root.join("backups")).unwrap();
    let migration = store.apply().unwrap();
    assert_eq!(migration.applied, (14..=crate::store::DATABASE_SCHEMA_VERSION).collect::<Vec<_>>());
    // Restore into a distinct location, as a rollback does. Existence alone
    // would accept an empty backup or one taken after migration changed history.
    let restored = root.join("restored.sqlite3");
    std::fs::copy(migration.backup.unwrap(), &restored).unwrap();
    let backup = open_read_write(&restored).unwrap();
    assert_eq!(
        backup
            .query_row("SELECT max(version) FROM schema_migrations", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        13
    );
    assert_eq!(
        backup
            .query_row("SELECT lifecycle FROM turns WHERE turn_id='turn'", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "running"
    );
    let expected = (vec!["Review the revised design".into()], true);
    assert_eq!(batch_references(&backup, "original").unwrap(), expected);
    assert_eq!(
        backup.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)).unwrap(),
        "ok"
    );
    drop(backup);

    store.mark_turn_terminal("turn".into(), "unknown".into()).unwrap();
    drop(store);
    let store = StoreActor::start(database.clone(), root.join("backups")).unwrap();
    let status = store.runtime_status().unwrap();
    assert_eq!(status.batches.len(), 1);
    assert_eq!(status.batches[0].event_count, 1);
    let replay_batch = status.batches[0].batch_id.clone();
    let db = open_read_write(&database).unwrap();
    assert!(db.query_row("SELECT execution_stopped_at IS NULL FROM provider_sessions WHERE provider_session_id='old-physical'", [], |row| row.get::<_, bool>(0)).unwrap());
    assert_eq!(batch_references(&db, "original").unwrap(), expected);
    assert_eq!(batch_references(&db, &replay_batch).unwrap(), expected);
    assert!(
        store
            .claim_runnable_turn("issue".into(), "profile".into(), vec!["old-physical".into()])
            .unwrap()
            .is_none(),
        "an unknown old session cannot execute the replay"
    );
    assert!(matches!(
        store.mark_turn_terminal("turn".into(), "unknown".into()),
        Err(super::StoreError::InvalidData(reason)) if reason == "turn turn is not active"
    ));
    assert_eq!(store.runtime_status().unwrap().batches.len(), 1);

    // Record the stop evidence the Group supplies before completing a reset.
    store.set_provider_stop("old-physical".into(), true).unwrap();
    assert!(
        db.query_row(
            "SELECT execution_stopped_at IS NOT NULL FROM provider_sessions
             WHERE provider_session_id='old-physical'",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap()
    );
    store
        .complete_context_reset(
            status.context_resets[0].reset_id.clone(),
            "replacement-physical".into(),
            "fresh-revision".into(),
            "instructions".into(),
        )
        .unwrap();
    let replay = store
        .claim_runnable_turn("issue".into(), "profile".into(), vec!["replacement-physical".into()])
        .unwrap()
        .unwrap();
    assert_eq!((replay.references, replay.trusted_mention), expected);
    store.mark_turn_started(replay.turn_id.clone(), "replacement-turn".into()).unwrap();
    store.mark_turn_terminal(replay.turn_id, "completed".into()).unwrap();
    let owner = store.acquire_runtime_lease("runtime".into(), "publisher".into(), 60).unwrap();
    assert!(store.claim_github_write(owner.clone()).unwrap().is_some());
    store.release_runtime_lease(owner).unwrap();
    for content in ["eyes", "rocket", "+1"] {
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM github_write_outbox WHERE operation='reaction_add' AND content=?1",
                [content],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
    }
    assert_eq!(batch_references(&db, "original").unwrap(), expected);
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)).unwrap(),
        "ok"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(db);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

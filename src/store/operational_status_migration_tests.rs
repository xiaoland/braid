use super::{
    MIGRATIONS, StoreActor, apply_one, configure_connection, open_read_write, operational_status,
};
use rusqlite::params;

#[test]
fn status_convergence_upgrades_an_already_published_schema12_failure_without_a_new_turn() {
    let root = std::env::temp_dir().join(format!("braid-status-upgrade-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    let database = root.join("state.sqlite3");
    let mut db = open_read_write(&database).unwrap();
    configure_connection(&db).unwrap();
    for migration in MIGRATIONS.iter().filter(|m| m.version <= 12) {
        apply_one(&mut db, migration).unwrap();
    }
    db.execute_batch("INSERT INTO repositories VALUES('repo','owner/repository',1,'2000-01-01T00:00:00Z'); INSERT INTO work_items(node_id,repository_node_id,kind,number,state,observed_at) VALUES('item','repo','pr',194,'open','2000-01-01T00:00:00Z'); INSERT INTO assignments VALUES('old','item',1,'blocked','2000-01-01T00:00:00Z',NULL),('new','item',2,'active','2002-01-01T00:00:00Z',NULL); INSERT INTO profiles VALUES('profile',1,'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','codex','pr'); INSERT INTO agent_instances(agent_id,assignment_id,profile_id,profile_revision,role,lifecycle) VALUES('agent-old','old','profile',1,'pr_implementation_agent','blocked'),('agent-new','new','profile',1,'pr_implementation_agent','idle'); INSERT INTO provider_sessions(session_id,agent_id,provider_kind,provider_session_id,context_revision,instruction_revision,lifecycle,started_at) VALUES('session','agent-new','codex','provider','context','instructions','idle','2002-01-01T00:00:00Z'); INSERT INTO turns(turn_id,session_id,provider_turn_id,context_revision,trigger_kind,lifecycle,started_at,ended_at) VALUES('confirmed','session','native-accepted','context','wake','completed','2002-01-01T00:00:00Z','2002-01-01T00:00:01Z');").unwrap();
    db.execute("INSERT INTO github_write_outbox(intent_id,repository,target_kind,target_database_id,operation,content,request_digest,lifecycle,next_attempt_at,created_at,updated_at,remote_database_id) VALUES('old-publication','owner/repository','issue','194','comment_create','Provider session unavailable',?1,'applied','2001-01-01T00:00:00Z','2001-01-01T00:00:00Z','2001-01-01T00:00:00Z','7120481524')",["a".repeat(64)]).unwrap();
    db.execute("INSERT INTO status_comments VALUES('item','profile',1,'status-node','7120481524','old-publication',?1,'applied','2001-01-01T00:00:00Z')",["b".repeat(64)]).unwrap();
    drop(db);
    let store = StoreActor::start(database.clone(), root.join("backups")).unwrap();
    let migrated = store.apply().unwrap();
    assert_eq!(migrated.applied, vec![13, 14]);
    assert!(migrated.backup.is_some());
    let owner = store.acquire_runtime_lease("runtime".into(), "publisher".into(), 3600).unwrap();
    let write = store.claim_github_write(owner.clone()).unwrap().unwrap();
    assert_eq!(write.operation, "comment_update");
    assert_eq!(write.target_database_id, "7120481524");
    assert!(write.content.contains("已由 generation 2 接替"));
    assert!(write.content.contains("2002-01-01T00:00:01Z"));
    store
        .finish_github_write(
            write,
            "applied",
            Some("7120481524".into()),
            Some("status-node".into()),
            None,
        )
        .unwrap();
    assert!(store.claim_github_write(owner.clone()).unwrap().is_none());
    store.release_runtime_lease(owner).unwrap();
    let db = open_read_write(&database).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM turns", [], |r| r.get::<_, i64>(0)).unwrap(),
        1,
        "no provider or new scheduling was needed"
    );
    assert_eq!(
        db.query_row("SELECT lifecycle FROM assignments WHERE assignment_id='old'", [], |r| r
            .get::<_, String>(
            0
        ))
        .unwrap(),
        "blocked"
    );
    assert_eq!(
        db.query_row(
            "SELECT status_generation FROM github_write_outbox WHERE intent_id='old-publication'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)).unwrap(),
        "ok"
    );
    let violations: i64 = db
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", params![], |r| r.get(0))
        .unwrap();
    assert_eq!(violations, 0);
    drop(db);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn status_convergence_confirmation_compares_instants_not_timestamp_strings() {
    assert!(
        operational_status::confirmation_after(
            "2000-01-01T00:00:00.1Z",
            Some("2000-01-01T00:00:00Z")
        )
        .unwrap()
    );
    assert!(
        !operational_status::confirmation_after(
            "2000-01-01T01:00:00+01:00",
            Some("2000-01-01T00:00:00Z")
        )
        .unwrap()
    );
    assert!(!operational_status::confirmation_after("2000-01-01T00:00:00Z", None).unwrap());
    assert!(
        operational_status::confirmation_after("not-a-time", Some("2000-01-01T00:00:00Z")).is_err()
    );
}

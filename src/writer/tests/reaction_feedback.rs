use super::*;
use crate::store::{ChangeIdentity, EventKind, ReactionTarget, TurnClaim};

const POLICY: SchedulerPolicy = SchedulerPolicy { quiet_seconds: 30, event_threshold: 8 };

fn accept_trusted_mention(f: &Fixture) -> TurnClaim {
    let mut mention = continuous_collaboration::revision("mention", "@braid Current design");
    mention.identity = ChangeIdentity::Occurrence;
    mention.event_name = "issue_comment".into();
    mention.action = Some("created".into());
    mention.object_node_id = Some("comment".into());
    mention.kind = EventKind::Wake;
    mention.detail = Some("created");
    mention.cross_surface_invalidation = false;
    mention.mention_candidate = true;
    mention.reaction_target =
        Some(ReactionTarget { kind: "issue_comment", database_id: "7".into() });
    let accepted = f.store.ingest_event(mention, POLICY).unwrap();
    f.store.resolve_mention(accepted.event_id.unwrap(), true, POLICY).unwrap();
    let claim = f
        .store
        .claim_runnable_turn("issue".into(), "issue-codex".into(), vec!["provider".into()])
        .unwrap()
        .unwrap();
    assert!(claim.trusted_mention);
    f.store.mark_turn_started(claim.turn_id.clone(), "native-turn".into()).unwrap();
    claim
}

#[test]
fn accepted_input_and_turn_terminal_survive_a_missing_publication_table() {
    for (terminal, expected) in [("completed", "+1"), ("failed", "confused")] {
        let f = Fixture::new(true);
        let db = f.db();
        db.execute_batch("ALTER TABLE github_write_outbox RENAME TO unavailable_outbox").unwrap();
        let claim = accept_trusted_mention(&f);
        f.store.mark_turn_terminal(claim.turn_id.clone(), terminal.into()).unwrap();
        assert_eq!(
            db.query_row("SELECT lifecycle FROM turns WHERE turn_id=?1", [&claim.turn_id], |row| {
                row.get::<_, String>(0)
            },)
                .unwrap(),
            terminal
        );
        let mut query =
            db.prepare("SELECT content FROM github_reaction_feedback ORDER BY sequence").unwrap();
        let intents = query
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        drop(query);
        assert_eq!(intents, ["eyes", "rocket", expected]);
        let owner =
            f.store.acquire_runtime_lease("runtime".into(), "publisher".into(), 60).unwrap();
        assert!(f.store.claim_github_write(owner.clone()).is_err());
        db.execute_batch("ALTER TABLE unavailable_outbox RENAME TO github_write_outbox").unwrap();
        for content in ["eyes", expected] {
            let write = f.store.claim_github_write(owner.clone()).unwrap().unwrap();
            assert_eq!(
                (write.operation.as_str(), write.content.as_str()),
                ("reaction_add", content)
            );
            f.store.finish_github_write(write, "applied", Some("99".into()), None, None).unwrap();
        }
        assert!(f.store.claim_github_write(owner.clone()).unwrap().is_none());
        assert_eq!(
            db.query_row(
                "SELECT lifecycle FROM github_write_outbox WHERE content='rocket'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "superseded"
        );
        f.store.release_runtime_lease(owner).unwrap();
    }
}

#[test]
fn context_reset_commits_with_publisher_unavailable_and_clears_a_late_rocket() {
    for restarted in [false, true] {
        let f = Fixture::new(true);
        let claim = accept_trusted_mention(&f);
        let owner =
            f.store.acquire_runtime_lease("runtime".into(), "publisher".into(), 60).unwrap();
        let eyes = f.store.claim_github_write(owner.clone()).unwrap().unwrap();
        assert_eq!(eyes.content, "eyes");
        f.store.finish_github_write(eyes, "applied", Some("91".into()), None, None).unwrap();
        let rocket = f.store.claim_github_write(owner.clone()).unwrap().unwrap();
        assert_eq!(rocket.content, "rocket");

        let db = f.db();
        db.execute_batch("ALTER TABLE github_write_outbox RENAME TO unavailable_outbox").unwrap();
        f.store
            .ingest_event(continuous_collaboration::revision("new-context", "Revised"), POLICY)
            .unwrap();
        let reset = f
            .store
            .begin_context_reset(Some(claim.turn_id.clone()), "issue".into(), "issue-codex".into())
            .unwrap()
            .unwrap();
        f.store
            .mark_context_reset_turn_terminal(
                reset.reset_id.clone(),
                claim.turn_id,
                "interrupted".into(),
            )
            .unwrap();
        let lifecycles: (String, String) = db
            .query_row(
                "SELECT t.lifecycle,r.lifecycle FROM context_resets r
             JOIN turns t ON t.turn_id=r.active_turn_id WHERE r.reset_id=?1",
                [&reset.reset_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(lifecycles, ("interrupted".into(), "materializing".into()));
        db.execute_batch("ALTER TABLE unavailable_outbox RENAME TO github_write_outbox").unwrap();

        let rocket = if restarted {
            f.store.recover_writes().unwrap();
            let retry = f.store.claim_github_write(owner.clone()).unwrap().unwrap();
            assert_eq!(retry.intent_id, rocket.intent_id);
            assert_eq!(retry.lifecycle, "uncertain");
            retry
        } else {
            rocket
        };
        f.store.finish_github_write(rocket, "applied", Some("92".into()), None, None).unwrap();
        let removal = f.store.claim_github_write(owner.clone()).unwrap().unwrap();
        assert_eq!(
            (removal.operation.as_str(), removal.content.as_str()),
            ("reaction_delete", "rocket")
        );
        assert_eq!(removal.remote_database_id.as_deref(), Some("92"));
        f.store.finish_github_write(removal, "applied", None, None, None).unwrap();
        assert!(f.store.claim_github_write(owner.clone()).unwrap().is_none());
        f.store.release_runtime_lease(owner).unwrap();
    }
}

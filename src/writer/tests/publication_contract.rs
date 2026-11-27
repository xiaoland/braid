use super::*;

#[tokio::test]
async fn accepted_obligation_converges_after_actor_restart_without_reacceptance() {
    let fixture = Fixture::new(false);
    let receipt = fixture.comment();
    let fixture = fixture.reopen();
    fixture.store.recover_writes().unwrap();
    let remote = Arc::new(Mutex::new(Remote::default()));
    let (github, task) = server(remote.clone(), true).await;
    fixture.drain(github.clone()).await;
    let result = fixture.receipt(&receipt.request_id);
    assert_eq!(result.lifecycle, "applied", "{:?}", result.last_error);
    assert_eq!(result.remote_database_id.as_deref(), Some("99"));
    let fixture = fixture.reopen();
    fixture.store.recover_writes().unwrap();
    fixture.drain(github).await;
    assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "applied");
    assert_eq!(remote.lock().unwrap().writes, 1);
    task.abort();
}

#[tokio::test]
async fn ambiguous_comment_recovery_records_ambiguity_without_another_mutation() {
    let fixture = Fixture::new(false);
    let receipt = fixture.comment();
    let remote = Arc::new(Mutex::new(Remote::default()));
    let (github, task) = server(remote.clone(), true).await;
    fixture.drain(github.clone()).await;
    assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "applied");
    {
        let mut remote = remote.lock().unwrap();
        let mut duplicate = remote.comments[0].clone();
        duplicate["id"] = json!(100);
        duplicate["node_id"] = json!("second-comment");
        remote.comments.push(duplicate);
    }
    fixture.lose_comment_acknowledgement();
    let fixture = fixture.reopen();
    fixture.store.recover_writes().unwrap();
    fixture.drain(github).await;
    let result = fixture.receipt(&receipt.request_id);
    assert_eq!(result.lifecycle, "ambiguous");
    assert!(result.last_error.unwrap().contains("multiple matching App comments"));
    assert_eq!(remote.lock().unwrap().writes, 1);
    task.abort();
}

#[tokio::test]
async fn permission_and_missing_target_are_rejected_by_the_production_executor() {
    for (permission, target_status, reason) in [
        (false, None, "lacks required permissions: issues"),
        (true, Some(axum::http::StatusCode::NOT_FOUND), "GitHub returned 404"),
    ] {
        let fixture = Fixture::new(false);
        let receipt = fixture.comment();
        let remote = Arc::new(Mutex::new(Remote { target_status, ..Remote::default() }));
        let (github, task) = server(remote.clone(), permission).await;
        fixture.drain(github.clone()).await;
        let result = fixture.receipt(&receipt.request_id);
        assert_eq!(result.lifecycle, "rejected");
        assert!(result.last_error.as_deref().unwrap().contains(reason), "{:?}", result.last_error);
        fixture.drain(github).await;
        assert_eq!(remote.lock().unwrap().writes, 0);
        task.abort();
    }
}

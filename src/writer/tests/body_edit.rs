use super::*;

#[allow(clippy::struct_excessive_bools)]
struct BodyRemote {
    snapshot: crate::github::BodySnapshot,
    writes: usize,
    associated: bool,
    limited: bool,
    mutation_limited: bool,
    lose_response: bool,
}
impl BodyRemote {
    fn new(pr: bool) -> Self {
        Self {
            snapshot: serde_json::from_value(json!({"id":1,"node_id":"item","number":1,"title":"Canonical","body":"Closes #7\n\nImplementation request: https://github.com/owner/repository/issues/comments/7","state":"open","updated_at":"version-1","pull_request":if pr {Some(json!({"url":"pr"}))} else {None}})).unwrap(),
            writes: 0, associated: true, limited: false, mutation_limited: false, lose_response: false,
        }
    }
}
async fn body_server(
    remote: Arc<Mutex<BodyRemote>>,
) -> (Arc<GitHubClient>, tokio::task::JoinHandle<()>) {
    let read = remote.clone();
    let patch = remote.clone();
    let association = remote.clone();
    let router = Router::new()
        .route("/repos/owner/repository/issues/1", get(move || { let remote = read.clone(); async move {
            use axum::response::IntoResponse as _;
            let remote=remote.lock().unwrap();
            if remote.limited { return (axum::http::StatusCode::TOO_MANY_REQUESTS, [("retry-after", "120")], Json(json!({"message":"limited"}))).into_response(); }
            let mut item=serde_json::to_value(&remote.snapshot).unwrap(); item["number"]=json!(1); item["html_url"]=json!("https://github.com/owner/repository/issues/1"); Json(item).into_response()
        }}).patch(move |Json(request): Json<Value>| {let remote=patch.clone(); async move {
            use axum::response::IntoResponse as _;
            let mut remote=remote.lock().unwrap();
            if remote.mutation_limited { return (axum::http::StatusCode::TOO_MANY_REQUESTS, [("retry-after", "120")], Json(json!({"message":"limited"}))).into_response(); }
            remote.writes+=1;
            remote.snapshot.title=request["title"].as_str().unwrap().into();
            remote.snapshot.body=Some(request["body"].as_str().unwrap().into());
            remote.snapshot.updated_at="version-2".into();
            if remote.lose_response { return (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(json!({"message":"lost acknowledgement"}))).into_response(); }
            Json(&remote.snapshot).into_response()
        }}))
        .route("/repos/owner/repository/issues/9", get(|| async {Json(json!({"id":9,"node_id":"source-pr","number":9,"title":"Source PR","html_url":"pr","state":"open","pull_request":{}}))}))
        .route("/graphql", post(move || { let remote=association.clone(); async move {Json(json!({"data":{"repository":{"pullRequest":{"closingIssuesReferences":{"nodes":if remote.lock().unwrap().associated {vec![json!({"number":1,"repository":{"nameWithOwner":"owner/repository"}})]}else{vec![]},"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}}))} }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let client = octocrab::Octocrab::builder()
        .base_uri(format!("http://{address}"))
        .unwrap()
        .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
        .build()
        .unwrap();
    let (template, other) = server(Arc::new(Mutex::new(Remote::default())), true).await;
    other.abort();
    let config = crate::config::GitHubConfig {
        app_id: 1,
        private_key_file: PathBuf::new(),
        projects_v2_enabled: false,
        repository: "owner/repository".into(),
        additional_repositories: Vec::new(),
        handle: "braid".into(),
        api_version: "2022-11-28".into(),
        webhook_secret_environment: None,
        webhook_secret_file: None,
    };
    (
        Arc::new(GitHubClient::publication_test_client(
            client,
            template.identity().clone(),
            config,
        )),
        task,
    )
}
fn pr_source(fixture: &Fixture, own_pr: bool) {
    fixture.db().execute("UPDATE agent_instances SET role='pr_implementation_agent'", []).unwrap();
    fixture
        .db()
        .execute("UPDATE work_items SET kind='pr',number=?1", [if own_pr { 1 } else { 9 }])
        .unwrap();
}
async fn accept(fixture: &Fixture, github: &GitHubClient) -> GhWriteReceipt {
    let target: WorkItemLocator = "owner/repository#1".parse().unwrap();
    let snapshot = edit::read_snapshot(github, &target).await.unwrap();
    edit(
        &fixture.config,
        &fixture.store,
        EditRequest {
            invocation: fixture.invocation(),
            target,
            profile_id: "issue-codex".into(),
            snapshot,
            title: Some("Current delivery".into()),
            body: Some("Scope, verification, risks".into()),
            request_id: "edit-1".into(),
        },
    )
    .unwrap()
}
#[tokio::test]
async fn own_pr_edit_preserves_marker_closing_and_recovers_unconfirmed_success_without_overwrite() {
    let fixture = Fixture::new(true);
    pr_source(&fixture, true);
    let remote = Arc::new(Mutex::new(BodyRemote::new(true)));
    let (github, task) = body_server(remote.clone()).await;
    let receipt = accept(&fixture, &github).await;
    remote.lock().unwrap().lose_response = true;
    fixture.drain(github).await;
    assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "uncertain");
    task.abort();
    let (github, task) = body_server(remote.clone()).await;
    fixture.due();
    fixture.store.recover_writes().unwrap();
    fixture.drain(github).await;
    assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "applied");
    let state = remote.lock().unwrap();
    assert_eq!(state.writes, 1);
    let body = state.snapshot.body.as_ref().unwrap();
    assert!(body.contains("Closes #7"));
    assert!(body.contains("Implementation request:"));
    assert!(body.contains("PR Implementation Agent"));
    assert_eq!(body.matches("> **Braid Agent").count(), 1);
    task.abort();
}
#[tokio::test]
async fn pr_source_corrects_direct_issue_with_pr_attribution_not_profile_target_tag() {
    let fixture = Fixture::new(true);
    pr_source(&fixture, false);
    let remote = Arc::new(Mutex::new(BodyRemote::new(false)));
    let (github, task) = body_server(remote.clone()).await;
    let receipt = accept(&fixture, &github).await;
    fixture.drain(github).await;
    let result = fixture.receipt(&receipt.request_id);
    assert_eq!(result.lifecycle, "applied", "{:?}", result.last_error);
    assert_eq!(result.role, "PR Implementation Agent");
    assert!(
        remote.lock().unwrap().snapshot.body.as_ref().unwrap().contains("PR Implementation Agent")
    );
    task.abort();
}
#[tokio::test]
async fn changed_association_closed_target_old_context_generation_session_and_human_edits_never_mutate()
 {
    for fence in
        ["unrelated", "closed", "context", "generation", "session", "human", "profile", "turn"]
    {
        let fixture = Fixture::new(true);
        pr_source(&fixture, false);
        let remote = Arc::new(Mutex::new(BodyRemote::new(false)));
        let (github, task) = body_server(remote.clone()).await;
        let receipt = accept(&fixture, &github).await;
        match fence {
            "unrelated" => remote.lock().unwrap().associated = false,
            "closed" => remote.lock().unwrap().snapshot.state = "closed".into(),
            "context" => {
                fixture
                    .db()
                    .execute("UPDATE provider_sessions SET context_revision='replaced'", [])
                    .unwrap();
            }
            "generation" => {
                fixture.db().execute("UPDATE assignments SET generation=2", []).unwrap();
            }
            "session" => {
                fixture.db().execute("INSERT INTO provider_sessions(session_id,agent_id,provider_kind,provider_session_id,context_revision,instruction_revision,lifecycle,started_at) VALUES ('new','agent','codex','new','context','instructions','idle','now')",[]).unwrap();
            }
            "human" => remote.lock().unwrap().snapshot.body = Some("Human new design".into()),
            "profile" => {
                assert!(
                    resolve_source(&fixture.config, Some("pr-codex"), &fixture.invocation())
                        .is_err()
                );
            }
            "turn" => assert!(
                resolve_source(&fixture.config, None, &Invocation::Agent("not-current".into()))
                    .is_err()
            ),
            _ => unreachable!(),
        }
        if matches!(fence, "profile" | "turn") {
            task.abort();
            continue;
        }
        fixture.drain(github).await;
        let result = fixture.receipt(&receipt.request_id);
        assert_eq!(
            result.lifecycle,
            if matches!(fence, "human" | "closed") { "conflict" } else { "rejected" },
            "{fence}: {:?}",
            result.last_error
        );
        assert_eq!(remote.lock().unwrap().writes, 0, "{fence}");
        task.abort();
    }
}
#[tokio::test]
async fn preflight_and_mutation_limits_remain_durable_then_converge_after_restart() {
    for mutation in [false, true] {
        let fixture = Fixture::new(true);
        pr_source(&fixture, true);
        let remote = Arc::new(Mutex::new(BodyRemote::new(true)));
        let (github, task) = body_server(remote.clone()).await;
        let receipt = accept(&fixture, &github).await;
        {
            let mut state = remote.lock().unwrap();
            state.limited = !mutation;
            state.mutation_limited = mutation;
        }
        fixture.drain(github).await;
        assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "pending");
        assert_eq!(remote.lock().unwrap().writes, 0);
        task.abort();
        {
            let mut state = remote.lock().unwrap();
            state.limited = false;
            state.mutation_limited = false;
        }
        let (github, task) = body_server(remote.clone()).await;
        fixture.due();
        fixture.drain(github).await;
        assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "applied");
        assert_eq!(remote.lock().unwrap().writes, 1);
        task.abort();
    }
}

#[tokio::test]
async fn own_issue_body_edit_and_input_boundaries_use_the_same_acceptance_path() {
    let fixture = Fixture::new(true);
    let remote = Arc::new(Mutex::new(BodyRemote::new(false)));
    let (github, task) = body_server(remote.clone()).await;
    let target: WorkItemLocator = "owner/repository#1".parse().unwrap();
    let snapshot = edit::read_snapshot(&github, &target).await.unwrap();
    for (title, body) in [
        (Some(String::new()), None),
        (Some("x".repeat(257)), None),
        (None, Some(String::new())),
        (None, Some("界".repeat(20_001))),
        (None, None),
    ] {
        assert!(
            edit(
                &fixture.config,
                &fixture.store,
                EditRequest {
                    invocation: fixture.invocation(),
                    target: target.clone(),
                    profile_id: "issue-codex".into(),
                    snapshot: snapshot.clone(),
                    title,
                    body,
                    request_id: "invalid".into()
                }
            )
            .is_err()
        );
    }
    assert!(resolve_source(&fixture.config, None, &Invocation::Operator).is_err());
    let receipt = accept(&fixture, &github).await;
    fixture.drain(github).await;
    assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "applied");
    assert!(remote.lock().unwrap().snapshot.body.as_ref().unwrap().contains("Issue Agent"));
    task.abort();
}

#[tokio::test]
async fn unconfirmed_success_followed_by_human_edit_is_conflict_not_success_or_overwrite() {
    let fixture = Fixture::new(true);
    pr_source(&fixture, true);
    let remote = Arc::new(Mutex::new(BodyRemote::new(true)));
    let (github, task) = body_server(remote.clone()).await;
    let receipt = accept(&fixture, &github).await;
    remote.lock().unwrap().lose_response = true;
    fixture.drain(github).await;
    assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "uncertain");
    task.abort();
    remote.lock().unwrap().snapshot.body = Some("Human intervened".into());
    let (github, task) = body_server(remote.clone()).await;
    fixture.due();
    fixture.drain(github).await;
    assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "conflict");
    assert_eq!(remote.lock().unwrap().writes, 1);
    assert_eq!(remote.lock().unwrap().snapshot.body.as_deref(), Some("Human intervened"));
    task.abort();
}

#[tokio::test]
async fn repeated_snapshot_body_edit_does_not_duplicate_attribution_or_protected_prefix() {
    let fixture = Fixture::new(true);
    pr_source(&fixture, true);
    let remote = Arc::new(Mutex::new(BodyRemote::new(true)));
    let (github, task) = body_server(remote.clone()).await;
    let receipt = accept(&fixture, &github).await;
    fixture.drain(github.clone()).await;
    assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "applied");
    let body = remote.lock().unwrap().snapshot.body.clone().unwrap();
    let target: WorkItemLocator = "owner/repository#1".parse().unwrap();
    let receipt = edit(
        &fixture.config,
        &fixture.store,
        EditRequest {
            invocation: fixture.invocation(),
            target: target.clone(),
            profile_id: "issue-codex".into(),
            snapshot: edit::read_snapshot(&github, &target).await.unwrap(),
            title: None,
            body: Some(body.clone()),
            request_id: "repeat".into(),
        },
    )
    .unwrap();
    fixture.drain(github).await;
    assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "applied");
    assert_eq!(remote.lock().unwrap().snapshot.body.as_deref(), Some(body.as_str()));
    task.abort();
    fixture.db().execute("DELETE FROM turns", []).unwrap();
    assert!(resolve_source(&fixture.config, None, &Invocation::Operator).is_err());
}

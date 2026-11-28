use super::*;

#[allow(clippy::struct_excessive_bools)]
pub(super) struct BodyRemote {
    pub(super) snapshot: crate::github::BodySnapshot,
    pub(super) issue_comments: Vec<Value>,
    pub(super) issue_edit_evidence: serde_json::Map<String, Value>,
    writes: usize,
    pub(super) associated: bool,
    associated_999: bool,
    issue_999: crate::github::BodySnapshot,
    writes_999: usize,
    pub(super) limited: bool,
    pub(super) pr_closed: bool,
    mutation_limited: bool,
    pub(super) lose_response: bool,
}
impl BodyRemote {
    pub(super) fn new(pr: bool) -> Self {
        Self {
            snapshot: serde_json::from_value(json!({"id":1,"node_id":"item","number":1,"title":"Canonical","body":"Closes #7\n\nImplementation request: https://github.com/owner/repository/issues/comments/7","state":"open","updated_at":"version-1","pull_request":if pr {Some(json!({"url":"pr"}))} else {None}})).unwrap(),
            issue_comments: Vec::new(),
            issue_edit_evidence: serde_json::Map::new(),
            writes: 0, pr_closed: false, associated: true, associated_999: false,
            issue_999: serde_json::from_value(json!({"id":999,"node_id":"issue-999","number":999,"title":"Controlled Issue","body":"Original design","state":"open","updated_at":"version-1"})).unwrap(), writes_999: 0, limited: false, mutation_limited: false, lose_response: false,
        }
    }
}
fn combined_first_pages(
    request: &Value,
    state: &BodyRemote,
    issue: &Value,
    pr: &Value,
) -> Option<Value> {
    let query = request["query"].as_str()?;
    let (root, fields, edge, reference) = if query.contains("query PrFirstPages") {
        (
            "pullRequest",
            &["assignees", "labels", "comments", "reviews", "threads"][..],
            "associated_issue_refs",
            issue,
        )
    } else if query.contains("query IssueFirstPages") {
        (
            "issue",
            &[
                "assignees",
                "labels",
                "linked_branches",
                "sub_issues",
                "blocked_by",
                "blocking",
                "comments",
                "duplicate_events",
            ][..],
            "associated_prs",
            pr,
        )
    } else {
        return None;
    };
    let empty = json!({"nodes":[], "pageInfo":{"hasNextPage":false,"endCursor":null}});
    let mut object = serde_json::Map::new();
    for field in fields {
        object.insert((*field).into(), empty.clone());
    }
    if root == "issue" {
        object.get_mut("comments").unwrap()["nodes"] = json!(state.issue_comments);
    }
    if request["variables"]["projects"].as_bool() == Some(true) {
        object.insert("project_items".into(), empty);
    }
    object.insert(edge.into(), json!({"nodes":if state.associated {vec![reference]} else {Vec::new()},"pageInfo":{"hasNextPage":false,"endCursor":null}}));
    Some(json!({"data":{"repository":{root:object}}}))
}

// Keep edit and full-Context routes in one fixture with shared scenario state.
#[allow(clippy::too_many_lines)]
pub(super) async fn body_server(
    remote: Arc<Mutex<BodyRemote>>,
) -> (Arc<GitHubClient>, tokio::task::JoinHandle<()>) {
    let read = remote.clone();
    let patch = remote.clone();
    let association = remote.clone();
    let issue_read = remote.clone();
    let issue_patch = remote.clone();
    let pr_read = remote.clone();
    let comment_post = remote.clone();
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
        .route("/repos/owner/repository/issues/1/comments", post(move |Json(request): Json<Value>| {
            let remote = comment_post.clone();
            async move {
                let now = time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap();
                let mut remote = remote.lock().unwrap();
                remote.issue_comments.push(json!({
                    "id":"status", "fullDatabaseId":"99", "author":{"id":"app", "login":"braid[bot]"},
                    "createdAt":now, "updatedAt":now, "isMinimized":false, "body":request["body"],
                }));
                Json(json!({"id":99, "node_id":"status"}))
            }
        }))
        .route("/repos/owner/repository/issues/999", get(move || { let remote=issue_read.clone(); async move {
            let mut item=serde_json::to_value(&remote.lock().unwrap().issue_999).unwrap();
            item["number"]=json!(999);
            item["html_url"]=json!("https://github.com/owner/repository/issues/999");
            Json(item)
        }}).patch(move |Json(request): Json<Value>| {let remote=issue_patch.clone(); async move {
            let mut remote=remote.lock().unwrap(); remote.writes_999+=1;
            remote.issue_999.title=request["title"].as_str().unwrap().into();
            remote.issue_999.body=Some(request["body"].as_str().unwrap().into());
            remote.issue_999.updated_at="version-2".into();
            Json(remote.issue_999.clone())
        }}))
        .route("/repos/owner/repository/issues/9", get(move || { let remote=pr_read.clone(); async move {Json(json!({"id":9,"node_id":"pr","number":9,"title":"Source PR","html_url":"pr","state":if remote.lock().unwrap().pr_closed {"closed"} else {"open"},"pull_request":{}}))}}))
        .route("/graphql", post(move |Json(request): Json<Value>| { let remote=association.clone(); async move {
            let state=remote.lock().unwrap();
            if state.limited { return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE); }
            let query=request["query"].as_str().unwrap();
            let repository=json!({"id":"repo","nameWithOwner":"owner/repository"});
            let issue_ref=json!({"id":"item","number":1,"title":"Canonical","state":state.snapshot.state.to_uppercase(),"repository":repository});
            let pr_ref=json!({"id":"pr","number":9,"title":"Existing PR","state":if state.pr_closed {"CLOSED"} else {"OPEN"},"repository":repository});
            if let Some(first_pages) = combined_first_pages(&request, &state, &issue_ref, &pr_ref) {
                return Ok(Json(first_pages));
            }
            let mut nodes=Vec::new();
            if query.contains("items:") {
                if query.contains("closingIssuesReferences") && state.associated {nodes.push(issue_ref);}
                if query.contains("closedByPullRequestsReferences") && state.associated {nodes.push(pr_ref);}
                let connection=json!({"items":{"nodes":nodes,"pageInfo":{"hasNextPage":false,"endCursor":null}}});
                let root=if query.contains("pullRequest(number:") {"pullRequest"} else {"issue"};
                return Ok(Json(json!({"data":{"repository":{root:connection}}})));
            }
            if query.contains("fullDatabaseId") {
                let mut core=if query.contains("pullRequest(number:") {
                    json!({"id":"pr","fullDatabaseId":"9","number":9,"title":"Existing PR","body":"Closes #1","state":if state.pr_closed {"CLOSED"} else {"OPEN"},"isDraft":false,"merged":false,"updatedAt":"pr-v1","baseRefName":"main","headRefName":"work","headRepository":repository})
                } else {
                    json!({"id":"item","fullDatabaseId":"1","number":1,"title":state.snapshot.title,"body":state.snapshot.body,"state":state.snapshot.state.to_uppercase(),"updatedAt":state.snapshot.updated_at})
                };
                let root=if query.contains("pullRequest(number:") {"pullRequest"} else {"issue"};
                if root == "issue" { core.as_object_mut().unwrap().extend(state.issue_edit_evidence.clone()); }
                return Ok(Json(json!({"data":{"repository":{"id":"repo","nameWithOwner":"owner/repository",root:core}}})));
            }
            if state.associated {nodes.push(json!({"number":1,"repository":{"nameWithOwner":"owner/repository"}}));}
            if state.associated_999 {nodes.push(json!({"number":999,"repository":{"nameWithOwner":"owner/repository"}}));}
            Ok(Json(json!({"data":{"repository":{"pullRequest":{"closingIssuesReferences":{"nodes":nodes,"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}})))
        }}));
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
    (Arc::new(publication_client(client, true)), task)
}
pub(super) fn pr_source(fixture: &Fixture, own_pr: bool) {
    fixture.db().execute("UPDATE agent_instances SET role='pr_implementation_agent'", []).unwrap();
    fixture
        .db()
        .execute("UPDATE work_items SET kind='pr',number=?1", [if own_pr { 1 } else { 9 }])
        .unwrap();
}
pub(super) async fn accept(fixture: &Fixture, github: &GitHubClient) -> GhWriteReceipt {
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
    for (fence, state, reason) in [
        ("unrelated", "rejected", "target is not a permitted directly associated work item"),
        ("closed", "conflict", "canonical title/body changed since snapshot"),
        ("context", "rejected", "publication source was invalidated or superseded"),
        ("generation", "rejected", "publication source was invalidated or superseded"),
        ("session", "rejected", "publication source was invalidated or superseded"),
        ("human", "conflict", "canonical title/body changed since snapshot"),
    ] {
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
            _ => unreachable!(),
        }
        fixture.drain(github).await;
        let result = fixture.receipt(&receipt.request_id);
        assert_eq!(result.lifecycle, state, "{fence}: {:?}", result.last_error);
        assert!(result.last_error.as_deref().unwrap().contains(reason), "{fence}: {result:?}");
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
        let fixture = fixture.reopen();
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
    for (title, body, expected) in [
        (Some(String::new()), None, "title must contain 1-256 bytes"),
        (Some("x".repeat(257)), None, "title must contain 1-256 bytes"),
        (None, Some(String::new()), "body must not be empty"),
        (None, Some("界".repeat(20_001)), "Braid's safe limit is 60000"),
        (None, None, "edit requires --title or --body-file"),
    ] {
        let error = edit(
            &fixture.config,
            &fixture.store,
            EditRequest {
                invocation: fixture.invocation(),
                target: target.clone(),
                profile_id: "issue-codex".into(),
                snapshot: snapshot.clone(),
                title,
                body,
                request_id: "invalid".into(),
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains(expected), "wrong failure: {error}");
    }
    for (profile, invocation, expected) in [
        (
            Some("pr-codex"),
            fixture.invocation(),
            "publication Profile does not match source assignment",
        ),
        (
            None,
            Invocation::Agent("not-current".into()),
            "publication turn does not match current runtime worktree",
        ),
    ] {
        let error = resolve_source(&fixture.config, profile, &invocation).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
    assert!(
        resolve_source(&fixture.config, None, &Invocation::Operator)
            .unwrap_err()
            .to_string()
            .contains("operator entry is unavailable inside an Agent runtime worktree")
    );
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
    assert!(
        resolve_source(&fixture.config, None, &Invocation::Operator)
            .unwrap_err()
            .to_string()
            .contains("operator entry is unavailable inside an Agent runtime worktree")
    );
}

#[tokio::test]
async fn ordinary_fixed_resolved_prose_code_lists_links_and_new_closing_text_are_applied() {
    for body in [
        "Fixed pagination bug",
        "Resolved review feedback",
        "```rust\n// Fixed pagination bug\nlet resolved = true;\n```",
        "- Fixed pagination bug\n- Resolved review feedback",
        "[Fixed pagination bug](https://example.com/fixed) and [Resolved review feedback](https://example.com/resolved)",
        "New scope\n\nCloses #999",
        "resolves [#999](https://github.com/owner/repository/issues/999)",
    ] {
        let fixture = Fixture::new(true);
        pr_source(&fixture, true);
        let remote = Arc::new(Mutex::new(BodyRemote::new(true)));
        let (github, task) = body_server(remote.clone()).await;
        let target: WorkItemLocator = "owner/repository#1".parse().unwrap();
        // Ordinary words in the existing body are not frozen either.
        remote
            .lock()
            .unwrap()
            .snapshot
            .body
            .as_mut()
            .unwrap()
            .push_str("\n\nFixed previous bug\n\n- Resolved previous feedback\n\n```rust\n// Fixed previous bug\n```\n\nFixed [previous pagination bug](https://example.com/fixed)");
        let receipt = edit(
            &fixture.config,
            &fixture.store,
            EditRequest {
                invocation: fixture.invocation(),
                target: target.clone(),
                profile_id: "issue-codex".into(),
                snapshot: edit::read_snapshot(&github, &target).await.unwrap(),
                title: None,
                body: Some(body.into()),
                request_id: "ordinary".into(),
            },
        )
        .unwrap();
        fixture.drain(github).await;
        let result = fixture.receipt(&receipt.request_id);
        assert_eq!(result.lifecycle, "applied", "{body}: {:?}", result.last_error);
        let state = remote.lock().unwrap();
        assert_eq!(state.writes, 1);
        let rendered = state.snapshot.body.as_ref().unwrap();
        assert!(rendered.contains(body));
        assert!(rendered.contains("Closes #7"));
        assert!(rendered.contains("Implementation request:"));
        assert!(!rendered.contains("previous"));
        task.abort();
    }
}

#[tokio::test]
async fn new_pr_body_references_authorize_issue_edits_only_after_canonical_confirmation() {
    for body in ["Closes #999", "See #999; closing work will follow"] {
        let fixture = Fixture::new(true);
        pr_source(&fixture, true);
        let remote = Arc::new(Mutex::new(BodyRemote::new(true)));
        let (github, task) = body_server(remote.clone()).await;
        let own: WorkItemLocator = "owner/repository#1".parse().unwrap();
        let receipt = edit(
            &fixture.config,
            &fixture.store,
            EditRequest {
                invocation: fixture.invocation(),
                target: own.clone(),
                profile_id: "issue-codex".into(),
                snapshot: edit::read_snapshot(&github, &own).await.unwrap(),
                title: None,
                body: Some(body.into()),
                request_id: "new-source".into(),
            },
        )
        .unwrap();
        fixture.drain(github.clone()).await;
        assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "applied");
        assert_eq!(remote.lock().unwrap().writes, 1);
        let issue: WorkItemLocator = "owner/repository#999".parse().unwrap();
        for confirmed in [false, true] {
            remote.lock().unwrap().associated_999 = confirmed;
            let receipt = edit(
                &fixture.config,
                &fixture.store,
                EditRequest {
                    invocation: fixture.invocation(),
                    target: issue.clone(),
                    profile_id: "issue-codex".into(),
                    snapshot: edit::read_snapshot(&github, &issue).await.unwrap(),
                    title: None,
                    body: Some("Corrected design".into()),
                    request_id: format!("issue-{confirmed}"),
                },
            )
            .unwrap();
            fixture.drain(github.clone()).await;
            let result = fixture.receipt(&receipt.request_id);
            assert_eq!(
                result.lifecycle,
                if confirmed { "applied" } else { "rejected" },
                "{:?}",
                result.last_error
            );
            assert_eq!(remote.lock().unwrap().writes_999, usize::from(confirmed));
        }
        let assignments: i64 = fixture
            .db()
            .query_row("SELECT count(*) FROM assignments", [], |row| row.get(0))
            .unwrap();
        assert_eq!(assignments, 1, "body edits never activate another Group");
        task.abort();
    }
}

#[tokio::test]
async fn heading_closing_and_ensure_blocks_survive_fences_comments_and_repeated_edits() {
    for protected in [
        "## Closes #7 ##\n\n## Implementation request: https://github.com/owner/repository/issues/comments/7 ##",
        "Closes #7\n========\n\nImplementation request: https://github.com/owner/repository/issues/comments/7\n--------",
    ] {
        for replacement in ["Scope, verification, risks", "```rust\nunclosed", "<!-- unclosed"] {
            let fixture = Fixture::new(true);
            pr_source(&fixture, true);
            let remote = Arc::new(Mutex::new(BodyRemote::new(true)));
            remote.lock().unwrap().snapshot.body = Some(protected.into());
            let (github, task) = body_server(remote.clone()).await;
            let target: WorkItemLocator = "owner/repository#1".parse().unwrap();
            for repeat in [false, true] {
                let body = if repeat {
                    remote.lock().unwrap().snapshot.body.clone().unwrap()
                } else {
                    replacement.into()
                };
                let receipt = edit(
                    &fixture.config,
                    &fixture.store,
                    EditRequest {
                        invocation: fixture.invocation(),
                        target: target.clone(),
                        profile_id: "issue-codex".into(),
                        snapshot: edit::read_snapshot(&github, &target).await.unwrap(),
                        title: None,
                        body: Some(body),
                        request_id: format!("heading-{repeat}"),
                    },
                )
                .unwrap();
                fixture.drain(github.clone()).await;
                let result = fixture.receipt(&receipt.request_id);
                assert_eq!(result.lifecycle, "applied", "{:?}", result.last_error);
                let state = remote.lock().unwrap();
                let body = state.snapshot.body.as_ref().unwrap();
                assert!(body.contains(protected), "{body}");
                assert!(body.find(protected).unwrap() < body.find(replacement).unwrap());
                assert_eq!(body.matches("> **Braid Agent").count(), 1);
                assert_eq!(body.matches("Closes #7").count(), 1);
                assert_eq!(body.matches("Implementation request:").count(), 1);
            }
            task.abort();
        }
    }
}

#[tokio::test]
async fn executor_rejects_missing_existing_request_source_before_mutation() {
    let fixture = Fixture::new(true);
    pr_source(&fixture, true);
    let remote = Arc::new(Mutex::new(BodyRemote::new(true)));
    let (github, task) = body_server(remote.clone()).await;
    // An immutable intent must retain existing sources at execution too,
    // independently of allowing newly added native closing text.
    let mut receipt = accept(&fixture, &github).await;
    let mut payload: Value = serde_json::from_str(&receipt.payload).unwrap();
    payload["body"] = json!("Closes #7\n\nCloses #999");
    receipt.payload = serde_json::to_string(&payload).unwrap();
    let mut mutation_possible = false;
    let Err(helpers::WriteFailure::GitHub(GitHubError::GraphQl(message))) =
        edit::execute(&fixture.config, &fixture.store, &github, &receipt, &mut mutation_possible)
            .await
    else {
        panic!("expected a GraphQL rejection for the missing existing request source");
    };
    assert_eq!(
        message,
        "body maintenance must retain recognized existing closing and request sources"
    );
    assert!(!mutation_possible);
    assert_eq!(remote.lock().unwrap().writes, 0);
    task.abort();
}

#[tokio::test]
async fn context_dependent_protected_blocks_fail_closed_but_title_only_is_allowed() {
    for protected in [
        "Closes [#7](https://github.com/owner/repository/issues/7)",
        "Closes [target][issue]\n\n[issue]: https://github.com/owner/repository/issues/7",
        "Closes [#7][issue]",
        "> Closes #7",
        "- Implementation request: original",
        "<!-- Closes #7 -->",
    ] {
        let fixture = Fixture::new(true);
        pr_source(&fixture, true);
        let remote = Arc::new(Mutex::new(BodyRemote::new(true)));
        remote.lock().unwrap().snapshot.body = Some(protected.into());
        let (github, task) = body_server(remote.clone()).await;
        let target: WorkItemLocator = "owner/repository#1".parse().unwrap();
        let snapshot = edit::read_snapshot(&github, &target).await.unwrap();
        for body in [Some("Scope update".into()), None] {
            let title_only = body.is_none();
            let result = edit(
                &fixture.config,
                &fixture.store,
                EditRequest {
                    invocation: fixture.invocation(),
                    target: target.clone(),
                    profile_id: "issue-codex".into(),
                    snapshot: snapshot.clone(),
                    title: Some("Current title".into()),
                    body,
                    request_id: "boundary".into(),
                },
            );
            if title_only {
                let receipt = result.unwrap();
                fixture.drain(github.clone()).await;
                assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "applied");
            } else {
                let error = result.unwrap_err().to_string();
                assert!(
                    error
                        == "body maintenance requires self-contained paragraph/heading closing and request blocks without links or HTML; keep this body unchanged"
                        || error
                            == "body maintenance cannot change context-dependent closing or request blocks",
                    "{protected}: {error}"
                );
                assert_eq!(remote.lock().unwrap().writes, 0);
            }
        }
        let state = remote.lock().unwrap();
        assert_eq!(state.writes, 1);
        assert_eq!(state.snapshot.body.as_deref(), Some(protected));
        task.abort();
    }
}

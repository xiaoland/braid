//! Production acceptance → actor/outbox → HTTP convergence, with no store/executor shims.
use super::*;
use crate::store::IngressEvent;
use axum::{
    Json, Router,
    routing::{get, post},
};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub(crate) struct Fixture {
    root: PathBuf,
    pub(crate) config: Config,
    pub(crate) store: StoreActor,
}
impl Fixture {
    pub(crate) fn new(source: bool) -> Self {
        let root = std::env::temp_dir().join(format!("braid-writer-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        std::fs::write(
            &path,
            include_str!("../../config.example.toml")
                .replace("/absolute/path/to/instance", root.to_str().unwrap()),
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        let store = StoreActor::start(
            config.runtime.database().to_owned(),
            config.runtime.backups().to_owned(),
        )
        .unwrap();
        store.apply().unwrap();
        let fixture = Self { root, config, store };
        if source {
            fixture.bind_source();
        }
        fixture
    }
    pub(crate) fn db(&self) -> Connection {
        Connection::open(self.config.runtime.database()).unwrap()
    }
    fn bind_source(&self) {
        self.db().execute_batch("INSERT INTO repositories VALUES ('repo','owner/repository',1,'now'); INSERT INTO work_items(node_id,repository_node_id,kind,number,state,context_revision,observed_at) VALUES ('item','repo','issue',1,'open','context','now'); INSERT INTO profiles VALUES ('issue-codex',1,'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','codex','issue'); INSERT INTO assignments VALUES ('assignment','item',1,'active','now',NULL); INSERT INTO agent_instances(agent_id,assignment_id,profile_id,profile_revision,role,lifecycle) VALUES ('agent','assignment','issue-codex',1,'issue_agent','idle'); INSERT INTO provider_sessions(session_id,agent_id,provider_kind,provider_session_id,context_revision,instruction_revision,lifecycle,started_at) VALUES ('session','agent','codex','provider','context','instructions','idle','now'); INSERT INTO turns(turn_id,session_id,context_revision,trigger_kind,lifecycle) VALUES ('turn','session','context','wake','completed');").unwrap();
        let revision = self
            .config
            .profile_materials(self.config.profile("issue-codex").unwrap())
            .unwrap()
            .revision;
        let revision = i64::try_from(revision).unwrap();
        self.db().execute("INSERT INTO profiles SELECT profile_id,?1,effective_digest,provider_kind,tags FROM profiles WHERE revision=1", [revision]).unwrap();
        self.db().execute("UPDATE agent_instances SET profile_revision=?1", [revision]).unwrap();
        self.db().execute("INSERT INTO worktrees(worktree_id,agent_id,path,repository_node_id,lifecycle,observed_at) VALUES ('tree','agent',?1,'repo','active','now')", [std::env::current_dir().unwrap().to_str().unwrap()]).unwrap();
    }
    fn invocation(&self) -> Invocation {
        let turn = self.db().query_row("SELECT turn_id FROM turns ORDER BY CASE lifecycle WHEN 'running' THEN 0 WHEN 'starting' THEN 0 ELSE 1 END,rowid DESC LIMIT 1", [], |r| r.get::<_, String>(0));
        turn.map_or(Invocation::Operator, Invocation::Agent)
    }
    fn comment(&self) -> GhWriteReceipt {
        create_comment(
            &self.config,
            &self.store,
            CommentCreateRequest {
                invocation: self.invocation(),
                target: "owner/repository#1".parse().unwrap(),
                profile_id: "issue-codex".into(),
                body: "update".into(),
                request_id: Some("stable".into()),
            },
        )
        .unwrap()
    }
    async fn drain(&self, github: Arc<GitHubClient>) {
        let owner =
            self.store.acquire_runtime_lease("runtime".into(), "writer".into(), 3600).unwrap();
        let clients = HashMap::from([("owner/repository".into(), github)]);
        crate::outbox::drain_one_write(&self.store, &clients, &self.config, &owner).await;
        self.store.release_runtime_lease(owner).unwrap();
    }
    fn due(&self) {
        self.db()
            .execute("UPDATE github_write_outbox SET next_attempt_at='2000-01-01T00:00:00Z'", [])
            .unwrap();
    }
    fn receipt(&self, id: &str) -> GhWriteReceipt {
        self.store.gh_write_receipt(id.into()).unwrap().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[derive(Default)]
enum TargetKind {
    #[default]
    Issue,
    PullRequest,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Authority {
    #[default]
    Intact,
    SourceClosed,
    TargetClosed,
    AssociationRemoved,
    OtherIssue,
}

#[derive(Default)]
struct Remote {
    authority: Authority,
    pr_body: Option<String>,
    create_pr: bool,
    target_kind: TargetKind,
    pr_created: bool,
    head_created: bool,
    comments: Vec<Value>,
    comment_reads: usize,
    writes: usize,
    limit: Option<u64>,
}
async fn server(
    remote: Arc<Mutex<Remote>>,
    permission: bool,
) -> (Arc<GitHubClient>, tokio::task::JoinHandle<()>) {
    let listing = remote.clone();
    let writing = remote.clone();
    let updating = remote.clone();
    let target = remote.clone();
    let continuation = remote.clone();
    let pr_read = remote.clone();
    let associations = remote.clone();
    let pr = json!({"id":88,"node_id":"pr","number":9,"html_url":"https://github.com/owner/repository/pull/9","draft":true,"state":"open","body":"Closes #1\n\nImplementation request: https://github.com/owner/repository/issues/comments/7","head":{"ref":"head","sha":"new"},"base":{"ref":"main","sha":"base"}});
    let router = Router::new()
        .route("/repos/owner/repository/issues/1", get(move || { let remote=target.clone(); async move {
            use axum::response::IntoResponse as _;
            if remote.lock().unwrap().limit.is_some() { return (axum::http::StatusCode::TOO_MANY_REQUESTS, [("retry-after","120")], Json(json!({"message":"rate limit"}))).into_response(); }
            let mut item = json!({"id":1,"node_id":"item","number":1,"title":"Issue","state":"open","html_url":"https://github.com/owner/repository/issues/1"});
            if remote.lock().unwrap().authority==Authority::SourceClosed {item["state"]=json!("closed");}
            if matches!(remote.lock().unwrap().target_kind, TargetKind::PullRequest) { item["pull_request"] = json!({"url":"pr"}); }
            Json(item).into_response()
        }}))
        .route("/repos/owner/repository/issues/1/comments", get(move || { let remote=listing.clone(); async move { let mut state=remote.lock().unwrap(); state.comment_reads+=1; Json(state.comments.clone()) } }).post(move |Json(body): Json<Value>| { let remote=writing.clone(); async move {
            let mut remote=remote.lock().unwrap(); remote.writes+=1;
            let comment=json!({"id":99,"node_id":"comment","url":"https://api.github.com/repos/owner/repository/issues/comments/99","html_url":"https://github.com/owner/repository/issues/1#issuecomment-99","issue_url":"https://api.github.com/repos/owner/repository/issues/1","body":body["body"],"created_at":time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap(),"updated_at":"now","user":{"login":"braid[bot]","id":1,"node_id":"app","type":"Bot","site_admin":false,"avatar_url":"https://example.invalid","url":"https://example.invalid","html_url":"https://example.invalid"}});
            remote.comments.push(comment); Json(json!({"id":99,"node_id":"comment"}))
        }}))
        .route("/repos/owner/repository/issues/comments/99",axum::routing::patch(move |Json(body):Json<Value>|{let remote=updating.clone();async move{let mut remote=remote.lock().unwrap();remote.writes+=1;remote.comments.last_mut().unwrap()["body"]=body["body"].clone();Json(json!({"id":99,"node_id":"comment"}))}}))
        .route("/repos/owner/repository/issues/comments/7", get(|| async { Json(json!({"id":7,"node_id":"request-comment","url":"https://api.github.com/repos/owner/repository/issues/comments/7","html_url":"https://github.com/owner/repository/issues/1#issuecomment-7","issue_url":"https://api.github.com/repos/owner/repository/issues/1","body":"implement","created_at":"now","updated_at":"now"})) }))
        .route("/repos/owner/repository/issues/comments/8",get(move||{let remote=continuation.clone();async move{let number=if remote.lock().unwrap().authority==Authority::OtherIssue {2}else{1};Json(json!({"id":8,"node_id":"followup-comment","html_url":format!("https://github.com/owner/repository/issues/{number}#issuecomment-8"),"issue_url":format!("https://api.github.com/repos/owner/repository/issues/{number}"),"body":"continue current implementation","created_at":"now","updated_at":"now"}))}}))
        .route("/repos/owner/repository/issues/2",get(||async{Json(json!({"id":2,"node_id":"other-issue","number":2,"title":"Other issue","state":"open","html_url":"https://github.com/owner/repository/issues/2"}))}))
        .route("/repos/owner/repository", get(|| async { Json(json!({"default_branch":"main"})) }))
        .route("/repos/owner/repository/pulls", get({ let pr=pr.clone(); let remote=remote.clone(); move || { let mut pr=pr.clone(); let remote=remote.clone(); async move { let state=remote.lock().unwrap(); if let Some(body)=&state.pr_body {pr["body"]=json!(body);} Json(if state.create_pr && !state.pr_created {json!([])} else {json!([pr])}) } }}).post({ let pr=pr.clone(); let remote=remote.clone(); move || {let pr=pr.clone(); let remote=remote.clone(); async move { let mut state=remote.lock().unwrap(); state.pr_created=true; state.writes+=1; Json(pr) }}}))
        .route("/repos/owner/repository/git/ref/heads/main",get(|| async {Json(json!({"ref":"refs/heads/main","object":{"sha":"base"}}))}))
        .route("/repos/owner/repository/git/ref/heads/head",get({let remote=remote.clone(); move || {let remote=remote.clone(); async move {
            use axum::response::IntoResponse as _;
            if remote.lock().unwrap().head_created {Json(json!({"ref":"refs/heads/head","object":{"sha":"new"}})).into_response()} else {(axum::http::StatusCode::NOT_FOUND,Json(json!({"message":"missing"}))).into_response()}
        }}}))
        .route("/repos/owner/repository/git/commits/base",get(|| async {Json(json!({"sha":"base","tree":{"sha":"tree"}}))}))
        .route("/repos/owner/repository/git/commits",post({let remote=remote.clone(); move || {let remote=remote.clone(); async move {remote.lock().unwrap().writes+=1; Json(json!({"sha":"new","tree":{"sha":"tree"}}))}}}))
        .route("/repos/owner/repository/git/refs",post({let remote=remote.clone(); move || {let remote=remote.clone(); async move {let mut state=remote.lock().unwrap(); state.head_created=true; state.writes+=1; Json(json!({"ref":"refs/heads/head","object":{"sha":"new"}}))}}}))
        .route("/repos/owner/repository/pulls/9", get(move || {let mut pr=pr.clone();let remote=pr_read.clone(); async move {let remote=remote.lock().unwrap();if remote.authority==Authority::TargetClosed{pr["state"]=json!("closed");}
            if let Some(body)=&remote.pr_body{pr["body"]=json!(body);}Json(pr)}}))
        .route("/graphql", post(move||{let remote=associations.clone(); async move {let nodes=if remote.lock().unwrap().authority==Authority::AssociationRemoved{json!([])}else{json!([{"number":1,"repository":{"nameWithOwner":"owner/repository"}}])};Json(json!({"data":{"repository":{"pullRequest":{"closingIssuesReferences":{"nodes":nodes,"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}})) }}));
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
    let identity = crate::github::GitHubIdentity {
        app_id: 1,
        app_slug: "braid".into(),
        installation_id: 1,
        repository: "owner/repository".into(),
        repository_node_id: "repo".into(),
        actor_node_id: "app".into(),
        actor_login: "braid[bot]".into(),
        token_expires_at: String::new(),
        permissions: if permission {
            BTreeMap::from([
                ("issues".into(), "write".into()),
                ("contents".into(), "write".into()),
                ("pull_requests".into(), "write".into()),
            ])
        } else {
            BTreeMap::new()
        },
    };
    (
        Arc::new(GitHubClient::publication_test_client(
            client,
            identity,
            crate::config::GitHubConfig {
                app_id: 1,
                private_key_file: PathBuf::new(),
                projects_v2_enabled: false,
                repository: "owner/repository".into(),
                additional_repositories: Vec::new(),
                handle: "braid".into(),
                api_version: "2022-11-28".into(),
                webhook_secret_environment: None,
                webhook_secret_file: None,
            },
        )),
        task,
    )
}

#[tokio::test]
async fn outbox_delivery_recovery_and_rate_deadline_use_production_executor() {
    let fixture = Fixture::new(false);
    let receipt = fixture.comment();
    let remote = Arc::new(Mutex::new(Remote { limit: Some(120), ..Remote::default() }));
    let (github, task) = server(remote.clone(), true).await;
    fixture.drain(github.clone()).await;
    assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "pending");
    let next: String = fixture
        .db()
        .query_row("SELECT next_attempt_at FROM github_write_outbox", [], |row| row.get(0))
        .unwrap();
    let deadline =
        time::OffsetDateTime::parse(&next, &time::format_description::well_known::Rfc3339).unwrap();
    assert!(deadline > time::OffsetDateTime::now_utc() + time::Duration::seconds(110));
    assert_eq!(remote.lock().unwrap().writes, 0);
    task.abort();
    remote.lock().unwrap().limit = None;
    let (github, task) = server(remote.clone(), true).await;
    fixture.due();
    fixture.drain(github.clone()).await;
    assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "applied");
    assert_eq!(remote.lock().unwrap().writes, 1);
    // Simulate remote success before local acknowledgement persisted, then restart recovery.
    fixture.db().execute("UPDATE github_write_outbox SET lifecycle='sending'", []).unwrap();
    fixture.store.recover_writes().unwrap();
    fixture.drain(github).await;
    assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "applied");
    assert_eq!(remote.lock().unwrap().writes, 1);
    task.abort();
}

#[tokio::test]
async fn uncertain_comment_recovery_scans_the_remote_conversation_once() {
    let fixture = Fixture::new(false);
    let receipt = fixture.comment();
    fixture.db().execute("UPDATE github_write_outbox SET lifecycle='uncertain'", []).unwrap();
    let remote = Arc::new(Mutex::new(Remote::default()));
    let (github, http) = server(remote.clone(), true).await;
    fixture.drain(github).await;
    assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "applied");
    let remote = remote.lock().unwrap();
    assert_eq!(remote.comment_reads, 1);
    assert_eq!(remote.writes, 1);
    http.abort();
}

#[tokio::test]
async fn issue_source_ensures_pr_with_frozen_plan_and_one_native_activation() {
    let fixture = Fixture::new(true);
    assert!(
        create_comment(
            &fixture.config,
            &fixture.store,
            CommentCreateRequest {
                invocation: fixture.invocation(),
                target: "owner/repository#1".parse().unwrap(),
                profile_id: "pr-codex".into(),
                body: "forged".into(),
                request_id: None
            }
        )
        .is_err()
    );
    let receipt = ensure_pull_request(
        &fixture.config,
        &fixture.store,
        PullRequestEnsureRequest {
            invocation: fixture.invocation(),
            repository: "owner/repository".parse().unwrap(),
            comment_id: 7,
            head: Some("head".into()),
        },
    )
    .unwrap();
    assert_eq!(receipt.source.as_ref().unwrap().profile_id, "issue-codex");
    assert_eq!(receipt.source.as_ref().unwrap().role, "issue_agent");
    let remote = Arc::new(Mutex::new(Remote::default()));
    let (github, task) = server(remote.clone(), true).await;
    fixture.drain(github.clone()).await;
    let result = fixture.receipt(&receipt.request_id);
    assert_eq!(result.lifecycle, "applied", "{:?}", result.last_error);
    let plan =
        fixture.store.implementation_request_receipt(receipt.intent_id.clone()).unwrap().unwrap();
    assert_eq!(plan.head_ref, "head");
    assert_eq!(plan.stage, "activation_pending");
    for stage in ["head_ready", "pull_request_ready", "associated", "activation_pending"] {
        fixture.db().execute("UPDATE implementation_requests SET stage=?1", [stage]).unwrap();
        fixture.db().execute("UPDATE github_write_outbox SET lifecycle='sending'", []).unwrap();
        fixture.store.recover_writes().unwrap();
        fixture.drain(github.clone()).await;
        assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "applied");
    }
    let count: i64 = fixture
        .db()
        .query_row(
            "SELECT COUNT(*) FROM deliveries WHERE delivery_guid LIKE 'braid-pr-ensure-%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(remote.lock().unwrap().writes, 0);
    task.abort();
}

#[tokio::test]
async fn source_invalidation_and_missing_permissions_reject_before_mutation() {
    for invalidated in [false, true] {
        let fixture = Fixture::new(true);
        let receipt = fixture.comment();
        if invalidated {
            fixture
                .db()
                .execute("UPDATE provider_sessions SET lifecycle='reset_pending'", [])
                .unwrap();
        }
        if invalidated {
            // No environment flag is needed to reject a stale cwd binding.
            assert!(resolve_source(&fixture.config, None, &fixture.invocation()).is_err());
        }
        let remote = Arc::new(Mutex::new(Remote::default()));
        let (github, task) = server(remote.clone(), invalidated).await;
        fixture.drain(github).await;
        assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "rejected");
        assert_eq!(remote.lock().unwrap().writes, 0);
        task.abort();
    }
}

#[tokio::test]
async fn initial_ensure_bootstrap_pr_association_and_restart_do_not_duplicate_mutations() {
    let fixture = Fixture::new(true);
    let receipt = ensure_pull_request(
        &fixture.config,
        &fixture.store,
        PullRequestEnsureRequest {
            invocation: fixture.invocation(),
            repository: "owner/repository".parse().unwrap(),
            comment_id: 7,
            head: Some("head".into()),
        },
    )
    .unwrap();
    let remote = Arc::new(Mutex::new(Remote { create_pr: true, ..Remote::default() }));
    let (github, task) = server(remote.clone(), true).await;
    fixture.drain(github.clone()).await;
    let result = fixture.receipt(&receipt.request_id);
    assert_eq!(result.lifecycle, "applied", "{:?}", result.last_error);
    assert_eq!(remote.lock().unwrap().writes, 3); // commit, ref, PR
    fixture.db().execute("UPDATE github_write_outbox SET lifecycle='sending'", []).unwrap();
    fixture.store.recover_writes().unwrap();
    fixture.drain(github).await;
    assert_eq!(fixture.receipt(&receipt.request_id).lifecycle, "applied");
    assert_eq!(remote.lock().unwrap().writes, 3);
    task.abort();
}

#[tokio::test]
async fn source_role_can_publish_related_cross_surface_without_target_profile_tags() {
    for target_pr in [true, false] {
        for fence in ["valid", "invalidated", "profile_changed"] {
            let fixture = Fixture::new(true);
            if !target_pr {
                fixture
                    .db()
                    .execute("UPDATE agent_instances SET role='pr_implementation_agent'", [])
                    .unwrap();
            }
            if !target_pr {
                fixture.db().execute("UPDATE work_items SET kind='pr',number=9", []).unwrap();
            }
            assert!(
                resolve_source(&fixture.config, Some("pr-codex"), &fixture.invocation()).is_err()
            );
            let receipt = fixture.comment();
            match fence {
                "invalidated" => {
                    fixture
                        .db()
                        .execute("UPDATE provider_sessions SET lifecycle='reset_pending'", [])
                        .unwrap();
                }
                "profile_changed" => {
                    fixture.db().execute_batch("INSERT INTO profiles SELECT 'pr-codex', revision, effective_digest, provider_kind, tags FROM profiles WHERE profile_id='issue-codex'; UPDATE agent_instances SET profile_id='pr-codex';").unwrap();
                }
                _ => {}
            }
            let remote = Arc::new(Mutex::new(Remote {
                target_kind: if target_pr { TargetKind::PullRequest } else { TargetKind::Issue },
                ..Remote::default()
            }));
            let (github, task) = server(remote.clone(), true).await;
            fixture.drain(github).await;
            let result = fixture.receipt(&receipt.request_id);
            assert_eq!(
                result.lifecycle,
                if fence == "valid" { "applied" } else { "rejected" },
                "{:?}",
                result.last_error
            );
            assert_eq!(remote.lock().unwrap().writes, usize::from(fence == "valid"));
            task.abort();
        }
    }
}

mod turn_start;

mod body_edit;

mod continuous_collaboration;
mod handoff;
mod implementation_recovery;
mod observation_recovery;
mod reaction_feedback;
mod semantic_contract;
mod status_convergence;

pub(crate) async fn issue_context_server(
    body: &str,
) -> (Arc<GitHubClient>, tokio::task::JoinHandle<()>) {
    let mut remote = body_edit::BodyRemote::new(false);
    remote.snapshot.body = Some(body.into());
    body_edit::body_server(Arc::new(Mutex::new(remote))).await
}

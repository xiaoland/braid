#![allow(
    clippy::wildcard_imports,
    clippy::fn_params_excessive_bools,
    clippy::needless_pass_by_value
)]
use super::*;
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn page(nodes: Value, next: bool) -> Value {
    json!({"nodes":nodes,"pageInfo":{"hasNextPage":next,"endCursor":if next {Some("next")} else {None}}})
}
fn core(pr: bool, version: usize) -> Value {
    let mut value = json!({"id":"W1","fullDatabaseId":"1","number":1,"title":"Title","body":"Full body","state":"OPEN","updatedAt":version.to_string(),"author":null,"issueType":null,"milestone":null,"parent":null,"stateReason":null});
    if pr {
        value["isDraft"] = json!(false);
        value["merged"] = json!(false);
        value["baseRefName"] = json!("main");
        value["headRefName"] = json!("topic");
    }
    value
}
fn comment(id: &str, body: &str) -> Value {
    json!({"id":id,"fullDatabaseId":if id == "C1" {"2"} else {"3"},"author":null,"body":body,"createdAt":"2026-01-01","updatedAt":"2026-01-01","lastEditedAt":null,"isMinimized":false,"minimizedReason":null,"isPinned":false})
}
#[allow(clippy::too_many_lines)]
async fn server(
    pr: bool,
    multi: bool,
    drift: bool,
    fault: Option<&'static str>,
) -> (GitHubClient, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let operations = Arc::new(Mutex::new(Vec::new()));
    let log = operations.clone();
    let app = Router::new().route(
        "/graphql",
        post(move |Json(request): Json<Value>| {
            let log = log.clone();
            async move {
                let query = request["query"].as_str().unwrap();
                let op = query.split_whitespace().nth(1).unwrap().split('(').next().unwrap();
                let mut log = log.lock().unwrap();
                log.push(op.to_owned());
                let version =
                    usize::from(drift && log.iter().filter(|op| op.ends_with("Core")).count() > 1);
                let pr = if fault == Some("rich") { op.starts_with("Pr") } else { pr };
                let mut object = if op.ends_with("Core") { core(pr, version) } else { json!({}) };
                if op.ends_with("FirstPages") {
                    for field in if pr {
                        vec![
                            "assignees",
                            "labels",
                            "associated_issue_refs",
                            "comments",
                            "reviews",
                            "threads",
                        ]
                    } else {
                        vec![
                            "assignees",
                            "labels",
                            "linked_branches",
                            "sub_issues",
                            "blocked_by",
                            "blocking",
                            "associated_prs",
                            "comments",
                            "duplicate_events",
                        ]
                    } {
                        object[field] = page(
                            if field == "comments" {
                                let mut nodes = vec![comment("C1", "First comment")];
                                if !multi {
                                    nodes.push(comment("C2", "Complete comment body"));
                                }
                                json!(nodes)
                            } else {
                                json!([])
                            },
                            field == "comments" && multi,
                        );
                    }
                } else if !op.ends_with("Core") {
                    object["items"] = page(
                        if matches!(op, "IssueComments" | "PrComments") && request["variables"]["cursor"] == "next" {
                            json!([comment("C2", "Complete comment body")])
                        } else { json!([]) },
                        false,
                    );
                }
                if fault == Some("missing_cursor") && op.ends_with("FirstPages") {
                    object["comments"]["pageInfo"] = json!({"hasNextPage":true,"endCursor":null});
                }
                if fault == Some("unknown_union") && op.ends_with("FirstPages") && !pr {
                    object["duplicate_events"] = page(json!([{"duplicate":{"__typename":"FutureType"},"canonical":{"__typename":"FutureType"}}]),false);
                }
                if fault == Some("rich") {
                    let reference=json!({"id":if pr {"I2"} else {"W1"},"number":if pr {2} else {1},"title":"Linked","state":"OPEN","stateReason":null,"repository":{"id":"R1","nameWithOwner":"owner/repo"}});
                    if op.ends_with("FirstPages") {
                        object[if pr {"associated_issue_refs"} else {"associated_prs"}]=page(json!([reference.clone()]),false);
                        if pr {object["threads"]=page(json!([{"id":"T1","path":"src/main.rs","line":1,"startLine":null,"isResolved":false,"resolvedBy":null,"isCollapsed":false,"isOutdated":false}]),false);}
                    } else if op.ends_with("AssociatedPrs") || op.ends_with("AssociatedIssues") {
                        object["items"]=page(json!([reference]),false);
                    }
                    if !pr && op.ends_with("Core") {object["number"]=json!(2);object["id"]=json!("I2");}
                }
                let mut repository = json!({"id":"R1","nameWithOwner":"owner/repo"});
                repository[if pr { "pullRequest" } else { "issue" }] = object;
                let mut result = json!({"data":{"repository":repository,"rateLimit":{"cost":1}}});
                if op == "ReviewThreadComments" { result["data"]=json!({"node":{"items":page(json!([comment("C1","Review thread discussion")]),false)},"rateLimit":{"cost":1}}); }
                if fault == Some("partial") && op.ends_with("FirstPages") {
                    result["errors"] = json!([{"message":"incomplete"}]);
                }
                Json(result)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let config = crate::config::GitHubConfig {
        app_id: 1,
        repository: "owner/repo".into(),
        additional_repositories: vec![],
        handle: "@braid".into(),
        api_version: "2022-11-28".into(),
        private_key_file: "unused".into(),
        webhook_secret_environment: None,
        webhook_secret_file: None,
        projects_v2_enabled: false,
    };
    let identity = crate::github::GitHubIdentity {
        app_id: 1,
        app_slug: "braid".into(),
        installation_id: 1,
        repository: "owner/repo".into(),
        repository_node_id: "R1".into(),
        actor_node_id: "B1".into(),
        actor_login: "braid[bot]".into(),
        token_expires_at: String::new(),
        permissions: std::collections::BTreeMap::default(),
    };
    let client = octocrab::Octocrab::builder()
        .base_uri(format!("http://{address}"))
        .unwrap()
        .build()
        .unwrap();
    (GitHubClient::publication_test_client(client, identity, config), operations, task)
}
async fn materialize(client: &GitHubClient, pr: bool) -> Result<Value, ContextError> {
    let locator = "owner/repo#1".parse().unwrap();
    if pr {
        Ok(serde_json::to_value(materialize_pull_request(client, &locator, 100).await?).unwrap())
    } else {
        Ok(serde_json::to_value(materialize_issue(client, &locator, 100).await?).unwrap())
    }
}

#[tokio::test]
async fn single_page_bill_and_multipage_output_equivalence() {
    for pr in [false, true] {
        let (client, ops, task) = server(pr, false, false, None).await;
        let single = materialize(&client, pr).await.unwrap();
        assert_eq!(ops.lock().unwrap().len(), 5);
        assert_eq!(ops.lock().unwrap().iter().filter(|op| op.ends_with("Core")).count(), 2);
        assert_eq!(single["body"], "Full body");
        task.abort();
        let (client, ops, task) = server(pr, true, false, None).await;
        let multi = materialize(&client, pr).await.unwrap();
        for snapshot in [&single, &multi] {
            let comments =
                snapshot[if pr { "conversation" } else { "comments" }].as_array().unwrap();
            assert_eq!(comments.len(), 2);
            assert_eq!(comments[0]["node_id"], "C1");
            assert_eq!(comments[0]["body"], "First comment");
            assert_eq!(comments[1]["node_id"], "C2");
            assert_eq!(comments[1]["body"], "Complete comment body");
        }
        assert_eq!(single, multi);
        assert_eq!(ops.lock().unwrap().len(), 6);
        task.abort();
    }
}
#[tokio::test]
async fn drift_discards_and_retries_whole_snapshot_and_partial_errors_fail() {
    for pr in [false, true] {
        let (client, ops, task) = server(pr, false, true, None).await;
        let snapshot = materialize(&client, pr).await.unwrap();
        assert_eq!(snapshot["updated_at"], "1");
        assert_eq!(ops.lock().unwrap().len(), 10);
        task.abort();
        let (client, ops, task) = server(pr, false, false, Some("partial")).await;
        let error = materialize(&client, pr).await.unwrap_err();
        assert!(
            matches!(error, ContextError::GitHub(GitHubError::GraphQl(ref message)) if message == "incomplete"),
            "wrong failure: {error}"
        );
        assert_eq!(ops.lock().unwrap().len(), 3);
        task.abort();
    }
}

#[tokio::test]
async fn malformed_pagination_and_unknown_union_never_return_complete_context() {
    for fault in ["missing_cursor", "unknown_union"] {
        let (client, _, task) = server(false, false, false, Some(fault)).await;
        let error = materialize(&client, false).await.unwrap_err();
        match fault {
            "missing_cursor" => assert!(
                matches!(error, ContextError::Incomplete { connection: "comments", ref reason, .. } if reason == "hasNextPage was true without an endCursor"),
                "wrong failure: {error}"
            ),
            "unknown_union" => assert!(
                matches!(error, ContextError::GitHub(GitHubError::Octocrab(ref message)) if message.contains("unknown variant `FutureType`")),
                "wrong failure: {error}"
            ),
            _ => unreachable!(),
        }
        task.abort();
    }
}
#[tokio::test]
async fn associated_issue_and_review_thread_bodies_are_fully_materialized() {
    let (client, ops, task) = server(true, false, false, Some("rich")).await;
    let result = materialize(&client, true).await.unwrap();
    assert_eq!(result["associated_issues"][0]["body"], "Full body");
    assert_eq!(result["associated_issues"][0]["comments"][1]["body"], "Complete comment body");
    assert_eq!(result["review_threads"][0]["comments"][0]["body"], "Review thread discussion");
    assert_eq!(ops.lock().unwrap().len(), 11);
    task.abort();
}

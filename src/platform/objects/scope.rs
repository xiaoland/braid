//! A root's authoritative collaboration scope; execution remains Harness-owned.
use std::collections::BTreeSet;

use super::{CommandRequest, Item, ObjectCommand, PlatformObjects, row_id, sql_id};
use anyhow::{Context as _, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScopeBoundary {
    root_number: u64,
    pub root_open: bool,
    pub root_revision: u64,
    pub assigned_actor: Option<String>,
    registration: Option<RunPolicy>,
    pub member_numbers: BTreeSet<u64>,
}

impl ScopeBoundary {
    pub(crate) fn matches_registration(
        &self,
        root: u64,
        run_id: &str,
        client_id: &str,
        delivery_ref: &str,
    ) -> bool {
        self.root_number == root
            && self.registration.as_ref().is_some_and(|policy| {
                policy.root_number == root
                    && policy.run_id == run_id
                    && policy.client_id == client_id
                    && policy.delivery_ref == delivery_ref
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeSnapshot {
    pub root: Item,
    pub delivery_ref: Option<String>,
    pub delivery_commit: Option<String>,
    pub run_id: Option<String>,
    pub client_id: Option<String>,
    pub issues: Vec<Item>,
    pub pull_requests: Vec<Item>,
    pub prepared_commands: Vec<PendingCommand>,
    pub pending_merges: Vec<PendingMerge>,
    pub deliveries: Vec<DeliveryRef>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingCommand {
    pub request_id: String,
    pub operation: String,
    pub target_number: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingMerge {
    pub number: u64,
    pub lifecycle: String,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryRef {
    pub number: u64,
    pub head_commit: Option<String>,
    pub published: bool,
    pub merge_commit: Option<String>,
}

#[derive(Default)]
struct ScopeMembers {
    issues: Vec<u64>,
    pull_requests: Vec<u64>,
}

fn scope_members(connection: &Connection, root: u64) -> Result<ScopeMembers> {
    let mut stmt = connection.prepare(
        "WITH RECURSIVE descendants(number) AS (
            SELECT ?1
            UNION
            SELECT p.child_number FROM local_issue_parents p
            JOIN descendants d ON p.parent_number=d.number
        )
        SELECT number,0 FROM descendants
        UNION ALL
        SELECT DISTINCT l.pr_number,1 FROM local_links l
        JOIN descendants d ON l.issue_number=d.number
        ORDER BY 1",
    )?;
    let mut rows = stmt.query([sql_id(root)?])?;
    let mut members = ScopeMembers::default();
    while let Some(row) = rows.next()? {
        let number = row_id(row, 0)?;
        let is_pr: bool = row.get(1)?;
        if is_pr {
            members.pull_requests.push(number);
        } else {
            members.issues.push(number);
        }
    }
    Ok(members)
}

impl PlatformObjects {
    pub fn parent(&self, number: u64) -> Result<Option<Item>> {
        let parent = self
            .connection
            .query_row(
                "SELECT parent_number FROM local_issue_parents WHERE child_number=?1",
                [sql_id(number)?],
                |r| row_id(r, 0),
            )
            .optional()?;
        parent.map(|id| self.item(id)).transpose()
    }
    pub fn children(&self, number: u64) -> Result<Vec<Item>> {
        let mut stmt = self.connection.prepare("SELECT child_number FROM local_issue_parents WHERE parent_number=?1 ORDER BY child_number")?;
        let ids = stmt
            .query_map([sql_id(number)?], |r| row_id(r, 0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter().map(|id| self.item(id)).collect()
    }

    pub(crate) fn scope_boundary(&self, root: u64) -> Result<ScopeBoundary> {
        let tx = self.connection.unchecked_transaction()?;
        let (kind, root_open, root_revision, assigned_actor): (String, bool, u64, Option<String>) =
            tx.query_row(
                "SELECT kind,state='open',revision,desired_profile FROM local_items
                 WHERE number=?1 AND repository=?2",
                params![sql_id(root)?, self.repository],
                |row| Ok((row.get(0)?, row.get(1)?, row_id(row, 2)?, row.get(3)?)),
            )
            .optional()?
            .with_context(|| format!("local object {}#{root} does not exist", self.repository))?;
        ensure!(kind == "issue", "scope root must be an Issue");
        let registration = run_policy(&tx, root)?;
        let members = scope_members(&tx, root)?;
        tx.commit()?;
        Ok(ScopeBoundary {
            root_number: root,
            root_open,
            root_revision,
            assigned_actor,
            registration,
            member_numbers: members.issues.into_iter().chain(members.pull_requests).collect(),
        })
    }

    pub fn scope(&self, root: u64) -> Result<ScopeSnapshot> {
        let tx = self.connection.unchecked_transaction()?;
        let snapshot = self.scope_inner(root)?;
        tx.commit()?;
        Ok(snapshot)
    }
    fn scope_inner(&self, root: u64) -> Result<ScopeSnapshot> {
        let root_item = self.item(root)?;
        ensure!(root_item.kind == "issue", "scope root must be an Issue");
        let members = scope_members(&self.connection, root)?;
        let issues = members.issues.iter().map(|id| self.item(*id)).collect::<Result<Vec<_>>>()?;
        let pull_requests =
            members.pull_requests.iter().map(|id| self.item(*id)).collect::<Result<Vec<_>>>()?;
        let mut prepared_commands = Vec::new();
        let mut stmt = self.connection.prepare("SELECT request_json FROM control_receipts WHERE scope='objects' AND state='prepared' ORDER BY created_at,request_id")?;
        let requests =
            stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for raw in requests {
            let request: CommandRequest = serde_json::from_str(&raw)?;
            let target = match &request.command {
                ObjectCommand::CreateIssue { parent, .. } => *parent,
                ObjectCommand::CreatePr { issue, .. } => *issue,
                command => command.target(),
            };
            if target.is_some_and(|n| {
                members.issues.binary_search(&n).is_ok()
                    || members.pull_requests.binary_search(&n).is_ok()
            }) {
                prepared_commands.push(PendingCommand {
                    request_id: request.request_id,
                    operation: request.command.operation().into(),
                    target_number: target,
                });
            }
        }
        let mut pending_merges = Vec::new();
        let mut deliveries = Vec::new();
        for pr in &pull_requests {
            let merge: Option<(String, Option<String>, Option<String>)> = self
                .connection
                .query_row(
                    "SELECT lifecycle,error,merge_commit FROM local_merges WHERE pr_number=?1",
                    [sql_id(pr.number)?],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            if let Some((state, error, _)) = &merge
                && state != "applied"
            {
                pending_merges.push(PendingMerge {
                    number: pr.number,
                    lifecycle: state.clone(),
                    error: error.clone(),
                });
            }
            if pr.published || merge.as_ref().is_some_and(|(state, _, _)| state == "applied") {
                deliveries.push(DeliveryRef {
                    number: pr.number,
                    head_commit: pr.head_commit.clone(),
                    published: pr.published,
                    merge_commit: merge.and_then(
                        |(state, _, commit)| if state == "applied" { commit } else { None },
                    ),
                });
            }
        }
        let policy = self.run_policy(root)?;
        let delivery_commit =
            policy.as_ref().map(|p| self.delivery_commit(&p.delivery_ref)).transpose()?.flatten();
        Ok(ScopeSnapshot {
            delivery_ref: policy.as_ref().map(|p| p.delivery_ref.clone()),
            run_id: policy.as_ref().map(|p| p.run_id.clone()),
            client_id: policy.as_ref().map(|p| p.client_id.clone()),
            delivery_commit,
            root: root_item,
            issues,
            pull_requests,
            prepared_commands,
            pending_merges,
            deliveries,
        })
    }
}

impl PlatformObjects {
    pub(super) fn run_policy(&self, number: u64) -> Result<Option<RunPolicy>> {
        run_policy(&self.connection, number)
    }
    fn delivery_commit(&self, delivery_ref: &str) -> Result<Option<String>> {
        let source: String = self.connection.query_row(
            "SELECT source FROM local_space WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let git = git2::Repository::open(source)?;
        match git.find_reference(delivery_ref) {
            Ok(reference) => Ok(Some(reference.peel_to_commit()?.id().to_string())),
            Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    pub(super) fn configure_run_root(
        &mut self,
        number: u64,
        delivery_ref: &str,
        run_id: &str,
        client_id: &str,
        author: &str,
    ) -> Result<()> {
        super::nonempty(run_id, "runId")?;
        super::nonempty(client_id, "clientId")?;
        let delivery_ref = super::full_branch(delivery_ref)?;
        let tx = super::write_transaction(&mut self.connection)?;
        let item = super::read_item(&tx, &self.repository, number)?;
        ensure!(item.kind == "issue", "run root must be an Issue");
        let has_parent: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_issue_parents WHERE child_number=?1)",
            [sql_id(number)?],
            |r| r.get(0),
        )?;
        ensure!(!has_parent, "run root cannot be a child Issue");
        if let Some(existing) = run_policy(&tx, number)? {
            ensure!(
                existing.root_number == number
                    && existing.delivery_ref == delivery_ref
                    && existing.run_id == run_id
                    && existing.client_id == client_id,
                "run root already belongs to another execution identity or delivery policy"
            );
        } else {
            super::validate_revision(&tx, &self.repository, self.revision_guard)?;
            let overlap: bool = tx.query_row("WITH RECURSIVE descendants(number) AS (SELECT ?1 UNION SELECT p.child_number FROM local_issue_parents p JOIN descendants d ON p.parent_number=d.number) SELECT EXISTS(SELECT 1 FROM descendants d JOIN run_root_policies p ON p.root_number=d.number)", [sql_id(number)?], |r| r.get(0))?;
            ensure!(!overlap, "run scopes cannot overlap");
            let mut stmt = tx.prepare("WITH RECURSIVE descendants(number) AS (SELECT ?1 UNION SELECT p.child_number FROM local_issue_parents p JOIN descendants d ON p.parent_number=d.number) SELECT DISTINCT i.number,i.base_ref FROM local_links l JOIN descendants d ON l.issue_number=d.number JOIN local_items i ON i.number=l.pr_number")?;
            let prs = stmt
                .query_map([sql_id(number)?], |r| {
                    Ok((super::row_id(r, 0)?, r.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (pr, base) in prs {
                ensure!(
                    super::full_branch(&base)? == delivery_ref,
                    "existing scoped PR has another delivery base"
                );
                let crossing: bool = tx.query_row("WITH RECURSIVE descendants(number) AS (SELECT ?1 UNION SELECT p.child_number FROM local_issue_parents p JOIN descendants d ON p.parent_number=d.number) SELECT EXISTS(SELECT 1 FROM local_links WHERE pr_number=?2 AND issue_number NOT IN (SELECT number FROM descendants))", rusqlite::params![sql_id(number)?,sql_id(pr)?], |r| r.get(0))?;
                ensure!(!crossing, "existing PR links cross the run scope");
            }
            tx.execute("INSERT INTO run_root_policies(root_number,delivery_ref,run_id,client_id) VALUES(?1,?2,?3,?4)",rusqlite::params![sql_id(number)?,delivery_ref,run_id,client_id])?;
            super::bump(&tx, number)?;
            super::emit(&tx, number, "invalidate", Some("run_policy"), author, None, None)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(super) fn initialize_delivery_ref(
        &self,
        git: &git2::Repository,
        delivery_ref: &str,
    ) -> Result<()> {
        match git.find_reference(delivery_ref) {
            Ok(reference) => {
                reference.peel_to_commit()?;
            }
            Err(error) if error.code() == git2::ErrorCode::NotFound => {
                let default_branch: String = self.connection.query_row(
                    "SELECT default_branch FROM local_space WHERE singleton=1",
                    [],
                    |r| r.get(0),
                )?;
                let base = git
                    .find_reference(&super::full_branch(&default_branch)?)?
                    .peel_to_commit()?
                    .id();
                match git.reference(delivery_ref, base, false, "Braid finite run delivery") {
                    Ok(_) => {}
                    Err(error) if error.code() == git2::ErrorCode::Exists => {
                        git.find_reference(delivery_ref)?.peel_to_commit()?;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunPolicy {
    pub root_number: u64,
    pub delivery_ref: String,
    pub run_id: String,
    pub client_id: String,
}
impl RunPolicy {
    pub(crate) fn require_open_owner(
        &self,
        connection: &Connection,
        client_id: &str,
        run_id: Option<&str>,
    ) -> Result<()> {
        ensure!(
            self.client_id == client_id && run_id == Some(self.run_id.as_str()),
            "execution identity does not own this registered run scope"
        );
        let open: bool = connection.query_row(
            "SELECT state='open' FROM local_items WHERE number=?1",
            [i64::try_from(self.root_number)?],
            |row| row.get(0),
        )?;
        ensure!(open, "run root is closed; its execution scope is no longer open");
        Ok(())
    }
}

pub(crate) fn run_policy(connection: &Connection, number: u64) -> Result<Option<RunPolicy>> {
    let policy: Option<(i64, String, String, String)> = connection.query_row(
        "WITH RECURSIVE ancestors(number) AS (SELECT ?1 UNION SELECT issue_number FROM local_links WHERE pr_number=?1 UNION SELECT p.parent_number FROM local_issue_parents p JOIN ancestors a ON p.child_number=a.number) SELECT policy.root_number,policy.delivery_ref,policy.run_id,policy.client_id FROM ancestors a JOIN run_root_policies policy ON policy.root_number=a.number",
        [i64::try_from(number)?], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
    ).optional()?;
    policy
        .map(|(root, delivery_ref, run_id, client_id)| {
            Ok(RunPolicy { root_number: u64::try_from(root)?, delivery_ref, run_id, client_id })
        })
        .transpose()
}

//! A root's authoritative collaboration scope; execution remains Harness-owned.
use super::{CommandRequest, Item, ObjectCommand, PlatformObjects, row_id, sql_id};
use anyhow::{Result, ensure};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

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
    pub fn scope(&self, root: u64) -> Result<ScopeSnapshot> {
        let tx = self.connection.unchecked_transaction()?;
        let snapshot = self.scope_inner(root)?;
        tx.commit()?;
        Ok(snapshot)
    }
    fn scope_inner(&self, root: u64) -> Result<ScopeSnapshot> {
        let root_item = self.item(root)?;
        ensure!(root_item.kind == "issue", "scope root must be an Issue");
        let mut stmt = self.connection.prepare("WITH RECURSIVE descendants(number) AS (SELECT ?1 UNION SELECT p.child_number FROM local_issue_parents p JOIN descendants d ON p.parent_number=d.number) SELECT number FROM descendants ORDER BY number")?;
        let ids = stmt
            .query_map([sql_id(root)?], |r| row_id(r, 0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let issues = ids.iter().map(|id| self.item(*id)).collect::<Result<Vec<_>>>()?;
        let mut prs = std::collections::BTreeSet::new();
        for issue in &issues {
            for pr in self.linked(issue.number, false)? {
                prs.insert(pr.number);
            }
        }
        let pull_requests = prs.iter().map(|id| self.item(*id)).collect::<Result<Vec<_>>>()?;
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
            if target.is_some_and(|n| ids.contains(&n) || prs.contains(&n)) {
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
    pub(super) fn run_policy(
        &self,
        number: u64,
    ) -> Result<Option<super::super::grants::RunPolicy>> {
        super::super::grants::run_policy(&self.connection, number)
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
        source: &std::path::Path,
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
        if let Some(existing) = super::super::grants::run_policy(&tx, number)? {
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
            tx.execute("WITH RECURSIVE descendants(number) AS (SELECT ?2 UNION SELECT p.child_number FROM local_issue_parents p JOIN descendants d ON p.parent_number=d.number), covered(number) AS (SELECT number FROM descendants UNION SELECT l.pr_number FROM local_links l JOIN descendants d ON l.issue_number=d.number) UPDATE platform_grants SET epoch=epoch+1,lifecycle='revoked',updated_at=?1 WHERE item_number IN (SELECT number FROM covered) AND lifecycle='active'", rusqlite::params![super::now(),sql_id(number)?])?;
            super::bump(&tx, number)?;
            super::emit(&tx, number, "invalidate", Some("run_policy"), author, None, None)?;
        }
        tx.commit()?;
        // The registered policy survives a Git error; retry initializes only a missing ref.
        let git = git2::Repository::open(source)?;
        match git.find_reference(&delivery_ref) {
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
                match git.reference(&delivery_ref, base, false, "Braid finite run delivery") {
                    Ok(_) => {}
                    Err(error) if error.code() == git2::ErrorCode::Exists => {
                        git.find_reference(&delivery_ref)?.peel_to_commit()?;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }
}

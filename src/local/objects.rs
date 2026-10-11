//! Braid-owned collaboration objects and their atomic event outbox.
mod commands;
pub use commands::{CommandFailure, CommandRequest, ControlPolicy, ControlReceipt};
use std::{path::Path, time::Duration};

use anyhow::{Context as _, Result, bail, ensure};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Savepoint, Transaction, TransactionBehavior, params,
};
use serde::Serialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    context::{
        Actor, CanonicalContext, CommentSnapshot, IssueSnapshot, PullRequestSnapshot, WorkItemKind,
        WorkItemReference,
    },
    store::{EventKind, IngressEvent},
};

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub number: u64,
    pub repository: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub author: String,
    pub state: String,
    pub desired_profile: Option<String>,
    pub head_ref: Option<String>,
    pub base_ref: Option<String>,
    pub head_commit: Option<String>,
    pub base_commit: Option<String>,
    pub published: bool,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Comment {
    #[serde(rename = "comment_id")]
    pub id: u64,
    pub operational: bool,
    pub reply_to: Option<u64>,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug)]
pub struct PendingEvent {
    pub event_id: u64,
    pub event: IngressEvent,
}

#[derive(Clone)]
struct Writer {
    session: String,
    turn: String,
    item: String,
    profile: String,
}

pub struct LocalObjects {
    connection: Connection,
    repository: String,
    writer: Option<Writer>,
    revision_guard: Option<(u64, u64)>,
    candidate_guard: Option<(String, String)>,
}

pub fn repository_node(repository: &str) -> String {
    format!("local:{repository}")
}
pub fn item_node(repository: &str, number: u64) -> String {
    format!("local:{repository}:item:{number}")
}
fn actor(login: &str) -> Actor {
    Actor { node_id: format!("local:actor:{login}"), login: login.into() }
}
fn now() -> String {
    OffsetDateTime::now_utc().format(&Rfc3339).expect("current time is representable")
}
fn nonempty(value: &str, field: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{field} must not be empty");
    Ok(())
}

impl LocalObjects {
    pub fn open(database: &Path, repository: &str) -> Result<Self> {
        nonempty(repository, "repository")?;
        let mut connection =
            Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_WRITE).with_context(
                || format!("open local collaboration database {}", database.display()),
            )?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let version: u32 = connection.query_row(
            "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            version == crate::store::DATABASE_SCHEMA_VERSION && version >= 4,
            "local collaboration requires the current database schema; run braid migrate apply"
        );
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let foreign: i64 = tx.query_row(
            "SELECT COUNT(*) FROM repositories WHERE node_id NOT LIKE 'local:%'",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            foreign == 0,
            "database already contains GitHub collaboration authority; use a separate local database"
        );
        tx.execute(
            "INSERT OR IGNORE INTO local_space(singleton,repository) VALUES(1,?1)",
            [repository],
        )?;
        let bound: String =
            tx.query_row("SELECT repository FROM local_space WHERE singleton=1", [], |r| r.get(0))?;
        ensure!(
            bound == repository,
            "local database belongs to repository {bound}, not {repository}"
        );
        tx.commit()?;
        Ok(Self {
            connection,
            repository: repository.into(),
            writer: None,
            revision_guard: None,
            candidate_guard: None,
        })
    }

    pub fn open_read_only(database: &Path, repository: &str) -> Result<Self> {
        let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let version: u32 = connection.query_row(
            "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            version == crate::store::DATABASE_SCHEMA_VERSION,
            "database schema is not current; apply migrations before HTTP control"
        );
        let bound: String = connection.query_row(
            "SELECT repository FROM local_space WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            bound == repository,
            "local database belongs to repository {bound}, not {repository}"
        );
        Ok(Self {
            connection,
            repository: repository.into(),
            writer: None,
            revision_guard: None,
            candidate_guard: None,
        })
    }

    pub fn verify_source(&self, source: &Path) -> Result<()> {
        let source = source.canonicalize()?;
        git2::Repository::open(&source)?;
        let bound: Option<String> = self.connection.query_row(
            "SELECT source FROM local_space WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            bound.as_deref() == source.to_str(),
            "Git source does not match this local collaboration space"
        );
        Ok(())
    }

    pub fn bind_source(&self, source: &Path) -> Result<()> {
        let source = source
            .canonicalize()
            .with_context(|| format!("resolve local Git source {}", source.display()))?;
        git2::Repository::open(&source)?;
        let source = source.to_str().context("Git source path must be UTF-8")?;
        self.connection.execute(
            "UPDATE local_space SET source=?1 WHERE singleton=1 AND source IS NULL",
            [source],
        )?;
        let bound: String = self.connection.query_row(
            "SELECT source FROM local_space WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        ensure!(bound == source, "local database belongs to Git source {bound}, not {source}");
        Ok(())
    }

    pub fn bind_agent(&mut self, cwd: &Path) -> Result<String> {
        let cwd = cwd.canonicalize()?;
        let mut stmt=self.connection.prepare("SELECT s.session_id,t.turn_id,a.work_item_node_id,g.profile_id,w.path FROM provider_sessions s JOIN turns t ON t.session_id=s.session_id JOIN agent_instances g ON g.agent_id=s.agent_id JOIN assignments a ON a.assignment_id=g.assignment_id JOIN worktrees w ON w.agent_id=g.agent_id WHERE s.lifecycle='running' AND t.lifecycle IN ('starting','running') AND a.lifecycle IN ('active','finalizing') AND g.lifecycle IN ('idle','finalizing') AND w.lifecycle='active'")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut matches = Vec::new();
        for (session, turn, item, profile, path) in rows {
            let root = Path::new(&path).canonicalize()?;
            if cwd.starts_with(root) {
                matches.push(Writer { session, turn, item, profile });
            }
        }
        ensure!(
            matches.len() == 1,
            "Agent CLI caller has no unique active assignment/session/turn in this worktree"
        );
        let writer = matches.pop().expect("exactly one caller");
        let profile = writer.profile.clone();
        self.writer = Some(writer);
        Ok(profile)
    }

    pub fn item(&self, number: u64) -> Result<Item> {
        read_item(&self.connection, &self.repository, number)
    }

    pub fn list(&self) -> Result<Vec<Item>> {
        let mut statement = self
            .connection
            .prepare("SELECT number FROM local_items WHERE repository=?1 ORDER BY number")?;
        let ids = statement
            .query_map([&self.repository], |r| row_id(r, 0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter().map(|id| self.item(id)).collect()
    }

    pub fn create_issue(&mut self, title: &str, body: &str, author: &str) -> Result<Item> {
        self.create(title, body, author, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_pr(
        &mut self,
        title: &str,
        body: &str,
        author: &str,
        git_dir: &Path,
        head: &str,
        base: &str,
        issue: Option<u64>,
    ) -> Result<Item> {
        nonempty(head, "head")?;
        nonempty(base, "base")?;
        let git = git2::Repository::open(git_dir)
            .with_context(|| format!("open Git repository {}", git_dir.display()))?;
        let head_commit = git.revparse_single(head)?.peel_to_commit()?.id().to_string();
        let base_branch = full_branch(base)?;
        let base_commit = git.refname_to_id(&base_branch)?.to_string();
        git.find_commit(git2::Oid::from_str(&base_commit)?)?;
        ensure!(head_commit != base_commit, "PR head and base resolve to the same commit");
        if let Some(number) = issue {
            ensure!(
                self.item(number)?.kind == "issue",
                "associated object #{number} is not an Issue"
            );
        }
        let item = self.create(
            title,
            body,
            author,
            Some((head, base, &head_commit, &base_commit, issue)),
        )?;
        Ok(item)
    }

    fn create(
        &mut self,
        title: &str,
        body: &str,
        author: &str,
        pr: Option<(&str, &str, &str, &str, Option<u64>)>,
    ) -> Result<Item> {
        nonempty(title, "title")?;
        nonempty(author, "author")?;
        let tx = write_transaction(&mut self.connection)?;
        validate_writer(&tx, self.writer.as_ref())?;
        validate_revision(&tx, &self.repository, self.revision_guard)?;
        let at = now();
        let kind = if pr.is_some() { "pr" } else { "issue" };
        let (head, base, hc, bc, issue) = pr
            .map_or((None, None, None, None, None), |(h, b, hc, bc, i)| {
                (Some(h), Some(b), Some(hc), Some(bc), i)
            });
        tx.execute("INSERT INTO local_items(repository,kind,title,body,author,head_ref,base_ref,head_commit,base_commit,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?10)",params![self.repository,kind,title,body,author,head,base,hc,bc,at])?;
        let id = u64::try_from(tx.last_insert_rowid())?;
        if let Some(issue) = issue {
            tx.execute(
                "INSERT INTO local_links(issue_number,pr_number) VALUES(?1,?2)",
                params![sql_id(issue)?, sql_id(id)?],
            )?;
            bump(&tx, issue)?;
            emit(
                &tx,
                issue,
                "invalidate",
                Some("cross_surface"),
                author,
                None,
                self.writer.as_ref(),
            )?;
        }
        emit(&tx, id, "noop", Some("created"), author, Some(body), self.writer.as_ref())?;
        tx.commit()?;
        self.item(id)
    }

    pub fn comments(&self, number: u64) -> Result<Vec<Comment>> {
        self.item(number)?;
        let mut stmt = self.connection.prepare("SELECT comment_id,author,body,created_at,operational,reply_to FROM local_comments WHERE item_number=?1 ORDER BY comment_id")?;
        Ok(stmt
            .query_map([sql_id(number)?], |r| {
                Ok(Comment {
                    id: row_id(r, 0)?,
                    author: r.get(1)?,
                    body: r.get(2)?,
                    created_at: r.get(3)?,
                    operational: r.get(4)?,
                    reply_to: r.get::<_, Option<i64>>(5)?.map(u64::try_from).transpose().map_err(
                        |e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                5,
                                rusqlite::types::Type::Integer,
                                Box::new(e),
                            )
                        },
                    )?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn reply(
        &mut self,
        number: u64,
        body: &str,
        author: &str,
        reply_to: Option<u64>,
    ) -> Result<Comment> {
        self.item(number)?;
        nonempty(body, "comment body")?;
        nonempty(author, "author")?;
        let tx = write_transaction(&mut self.connection)?;
        validate_writer(&tx, self.writer.as_ref())?;
        validate_revision(&tx, &self.repository, self.revision_guard)?;
        if let Some(parent) = reply_to {
            let same:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM local_comments WHERE comment_id=?1 AND item_number=?2)",params![sql_id(parent)?,sql_id(number)?],|r|r.get(0))?;
            ensure!(same, "reply target #{parent} is not a comment on this object");
        }
        let at = now();
        tx.execute(
            "INSERT INTO local_comments(item_number,author,body,created_at,reply_to) VALUES(?1,?2,?3,?4,?5)",
            params![sql_id(number)?, author, body, at,reply_to.map(sql_id).transpose()?],
        )?;
        let comment_id = u64::try_from(tx.last_insert_rowid())?;
        bump(&tx, number)?;
        emit(&tx, number, "wake", Some("comment"), author, Some(body), self.writer.as_ref())?;
        tx.commit()?;
        Ok(Comment {
            id: comment_id,
            operational: false,
            reply_to,
            author: author.into(),
            body: body.into(),
            created_at: at,
        })
    }

    pub fn edit(
        &mut self,
        number: u64,
        title: Option<&str>,
        body: Option<&str>,
        author: &str,
    ) -> Result<Item> {
        let old = self.item(number)?;
        nonempty(author, "author")?;
        ensure!(title.is_some() || body.is_some(), "provide title or body");
        if let Some(title) = title {
            nonempty(title, "title")?;
        }
        if title.unwrap_or(&old.title) == old.title && body.unwrap_or(&old.body) == old.body {
            return Ok(old);
        }
        let tx = write_transaction(&mut self.connection)?;
        validate_writer(&tx, self.writer.as_ref())?;
        validate_revision(&tx, &self.repository, self.revision_guard)?;
        tx.execute("UPDATE local_items SET title=COALESCE(?1,title),body=COALESCE(?2,body) WHERE number=?3",params![title,body,sql_id(number)?])?;
        bump(&tx, number)?;
        emit(&tx, number, "invalidate", Some("edited"), author, body, self.writer.as_ref())?;
        if old.kind == "issue" {
            invalidate_links(&tx, number, author, self.writer.as_ref())?;
        }
        tx.commit()?;
        self.item(number)
    }

    pub fn assign(&mut self, number: u64, profile: Option<&str>, author: &str) -> Result<Item> {
        let old = self.item(number)?;
        nonempty(author, "author")?;
        if let Some(profile) = profile {
            nonempty(profile, "profile")?;
        }
        if old.desired_profile.as_deref() == profile {
            return Ok(old);
        }
        let tx = write_transaction(&mut self.connection)?;
        validate_writer(&tx, self.writer.as_ref())?;
        validate_revision(&tx, &self.repository, self.revision_guard)?;
        tx.execute(
            "UPDATE local_items SET desired_profile=?1 WHERE number=?2",
            params![profile, sql_id(number)?],
        )?;
        bump(&tx, number)?;
        if old.desired_profile.is_some() {
            emit(&tx, number, "unassign", Some("unassigned"), author, None, self.writer.as_ref())?;
        }
        if profile.is_some() {
            emit(&tx, number, "assign", Some("assigned"), author, None, self.writer.as_ref())?;
        }
        tx.commit()?;
        self.item(number)
    }

    pub fn set_state(&mut self, number: u64, open: bool, author: &str) -> Result<Item> {
        let old = self.item(number)?;
        nonempty(author, "author")?;
        if open && old.kind == "pr" {
            let merged:bool=self.connection.query_row("SELECT EXISTS(SELECT 1 FROM local_merges WHERE pr_number=?1 AND lifecycle='applied')",[sql_id(number)?],|r|r.get(0))?;
            ensure!(!merged, "merged PR cannot be reopened; create a new PR candidate");
        }
        let state = if open { "open" } else { "closed" };
        if old.state == state {
            return Ok(old);
        }
        let tx = write_transaction(&mut self.connection)?;
        validate_writer(&tx, self.writer.as_ref())?;
        validate_revision(&tx, &self.repository, self.revision_guard)?;
        tx.execute(
            "UPDATE local_items SET state=?1 WHERE number=?2",
            params![state, sql_id(number)?],
        )?;
        bump(&tx, number)?;
        emit(
            &tx,
            number,
            "lifecycle",
            Some(if open { "reopened" } else { "closed" }),
            author,
            None,
            self.writer.as_ref(),
        )?;
        tx.commit()?;
        self.item(number)
    }

    pub fn link(&mut self, issue: u64, pr: u64, author: &str) -> Result<()> {
        ensure!(self.item(issue)?.kind == "issue", "#{issue} is not an Issue");
        ensure!(self.item(pr)?.kind == "pr", "#{pr} is not a PR");
        nonempty(author, "author")?;
        let tx = write_transaction(&mut self.connection)?;
        validate_writer(&tx, self.writer.as_ref())?;
        validate_revision(&tx, &self.repository, self.revision_guard)?;
        if tx.execute(
            "INSERT OR IGNORE INTO local_links VALUES(?1,?2)",
            params![sql_id(issue)?, sql_id(pr)?],
        )? > 0
        {
            for id in [issue, pr] {
                bump(&tx, id)?;
                emit(
                    &tx,
                    id,
                    "invalidate",
                    Some("cross_surface"),
                    author,
                    None,
                    self.writer.as_ref(),
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn linked(&self, number: u64, issues: bool) -> Result<Vec<Item>> {
        let sql = if issues {
            "SELECT issue_number FROM local_links WHERE pr_number=?1 ORDER BY issue_number"
        } else {
            "SELECT pr_number FROM local_links WHERE issue_number=?1 ORDER BY pr_number"
        };
        let mut stmt = self.connection.prepare(sql)?;
        let ids = stmt
            .query_map([sql_id(number)?], |r| row_id(r, 0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter().map(|id| self.item(id)).collect()
    }

    fn build_issue_snapshot(&self, item: Item) -> Result<IssueSnapshot> {
        let associated_prs = self.linked(item.number, false)?.into_iter().map(reference).collect();
        Ok(IssueSnapshot {
            node_id: item_node(&self.repository, item.number),
            database_id: item.number.to_string(),
            repository_node_id: repository_node(&self.repository),
            repository: self.repository.clone(),
            number: item.number,
            title: item.title,
            body: item.body,
            state: item.state,
            state_reason: None,
            updated_at: item.updated_at,
            author: Some(actor(&item.author)),
            issue_type: None,
            assignees: item.desired_profile.as_deref().map(actor).into_iter().collect(),
            labels: vec![],
            milestone: None,
            projects: vec![],
            linked_branches: vec![],
            parent: None,
            sub_issues: vec![],
            blocked_by: vec![],
            blocking: vec![],
            duplicate_pairs: vec![],
            associated_prs,
            comments: self.comment_snapshots(item.number)?,
        })
    }

    fn comment_snapshots(&self, number: u64) -> Result<Vec<CommentSnapshot>> {
        Ok(self
            .comments(number)?
            .into_iter()
            .filter(|c| !c.operational)
            .map(|c| CommentSnapshot {
                node_id: format!("local:{}:comment:{}", self.repository, c.id),
                database_id: c.id.to_string(),
                repository: self.repository.clone(),
                work_item_number: number,
                author: Some(actor(&c.author)),
                created_at: c.created_at.clone(),
                updated_at: c.created_at,
                body: Some(c.reply_to.map_or_else(
                    || c.body.clone(),
                    |parent| format!("Reply to local comment #{parent}.\n\n{}", c.body),
                )),
                minimized: false,
                minimized_reason: None,
                pinned: false,
                deleted: false,
            })
            .collect())
    }

    pub fn context(&self, number: u64) -> Result<CanonicalContext> {
        let tx = self.connection.unchecked_transaction()?;
        let context = self.context_inner(number)?;
        tx.commit()?;
        Ok(context)
    }

    fn context_inner(&self, number: u64) -> Result<CanonicalContext> {
        let item = self.item(number)?;
        if item.kind == "issue" {
            return Ok(CanonicalContext::Issue(self.build_issue_snapshot(item)?));
        }
        let associated_issues = self
            .linked(number, true)?
            .into_iter()
            .map(|i| self.build_issue_snapshot(i))
            .collect::<Result<Vec<_>>>()?;
        Ok(CanonicalContext::PullRequest(PullRequestSnapshot {
            node_id: item_node(&self.repository, number),
            database_id: number.to_string(),
            repository_node_id: repository_node(&self.repository),
            repository: self.repository.clone(),
            number,
            title: item.title,
            body: item.body,
            state: item.state,
            draft: false,
            merged: self.connection.query_row("SELECT EXISTS(SELECT 1 FROM local_merges WHERE pr_number=?1 AND lifecycle='applied')",[sql_id(number)?],|r|r.get(0))?,
            updated_at: item.updated_at,
            author: Some(actor(&item.author)),
            base_ref: item.base_ref.context("PR lacks base")?,
            head_repository: Some(self.repository.clone()),
            head_ref: item.head_ref.context("PR lacks head")?,
            assignees: item.desired_profile.as_deref().map(actor).into_iter().collect(),
            labels: vec![],
            milestone: None,
            projects: vec![],
            associated_issues,
            conversation: self.comment_snapshots(number)?,
            reviews: vec![],
            review_threads: vec![],
        }))
    }

    pub fn pending_events(&self, limit: u32) -> Result<Vec<PendingEvent>> {
        let mut stmt=self.connection.prepare("SELECT event_id,item_number,kind,detail,actor,body,revision,item_state,created_at,origin FROM local_events WHERE consumed_at IS NULL AND item_number IN (SELECT number FROM local_items WHERE repository=?1) ORDER BY event_id LIMIT ?2")?;
        let rows = stmt
            .query_map(params![self.repository, limit], |r| {
                Ok((
                    row_id(r, 0)?,
                    row_id(r, 1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    row_id(r, 6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(|(id,number,kind,detail,author,body,revision,state,at,origin)| {
            let item=self.item(number)?;
            let detail=match detail.as_deref() {Some("closed")=>Some("closed"),Some("reopened")=>Some("reopened"),Some("merged")=>Some("merged"),Some("cross_surface")=>Some("cross_surface"),_=>None};
            let event=IngressEvent {delivery_guid:format!("local:{id}"),event_name:"local".into(),action:None,repository_node_id:repository_node(&self.repository),repository:self.repository.clone(),work_item_node_id:Some(item_node(&self.repository,number)),work_item_kind:Some(if item.kind=="issue" {"issue"} else {"pr"}),work_item_number:Some(number),work_item_state:Some(state),object_node_id:Some(format!("local:{}:event:{id}",self.repository)),object_version:Some(format!("{revision}:{at}")),object_digest:None,visible_body:body.clone(),actor_node_id:Some(actor(&author).node_id),actor_login:Some(author.clone()),kind:EventKind::from_str(&kind).context("invalid local event kind")?,detail,cross_surface_invalidation:detail==Some("cross_surface"),origin:match origin.as_str(){"agent"=>"agent","human"=>"human",_=>bail!("invalid local event origin {origin}")},reference:format!("Braid {}#{} event {id}",self.repository,number),mention_candidate:false,reaction_target:None,known:true,raw_payload:serde_json::to_vec(&serde_json::json!({"event_id":id,"number":number,"revision":revision,"actor":author,"body":body}))?};
            Ok(PendingEvent {event_id:id,event})
        }).collect()
    }

    pub fn ack_event(&self, event_id: u64) -> Result<()> {
        ensure!(self.connection.execute("UPDATE local_events SET consumed_at=COALESCE(consumed_at,?1) WHERE event_id=?2 AND item_number IN (SELECT number FROM local_items WHERE repository=?3)",params![now(),sql_id(event_id)?,self.repository])? == 1,"local event {event_id} does not exist");
        Ok(())
    }
}

fn reference(item: Item) -> WorkItemReference {
    WorkItemReference {
        node_id: item_node(&item.repository, item.number),
        repository_node_id: repository_node(&item.repository),
        repository: item.repository,
        number: item.number,
        kind: if item.kind == "issue" { WorkItemKind::Issue } else { WorkItemKind::PullRequest },
        title: item.title,
        state: item.state,
        state_reason: None,
    }
}
fn read_item(connection: &Connection, repository: &str, number: u64) -> Result<Item> {
    connection.query_row("SELECT number,repository,kind,title,body,author,state,desired_profile,head_ref,base_ref,head_commit,base_commit,revision,created_at,updated_at,published FROM local_items WHERE number=?1 AND repository=?2",params![sql_id(number)?,repository],|r|Ok(Item {number:row_id(r,0)?,repository:r.get(1)?,kind:r.get(2)?,title:r.get(3)?,body:r.get(4)?,author:r.get(5)?,state:r.get(6)?,desired_profile:r.get(7)?,head_ref:r.get(8)?,base_ref:r.get(9)?,head_commit:r.get(10)?,base_commit:r.get(11)?,published:r.get(15)?,revision:row_id(r,12)?,created_at:r.get(13)?,updated_at:r.get(14)?})).optional()?.with_context(||format!("local object {repository}#{number} does not exist"))
}
fn bump(tx: &Connection, number: u64) -> Result<()> {
    tx.execute(
        "UPDATE local_items SET revision=revision+1,updated_at=?1 WHERE number=?2",
        params![now(), sql_id(number)?],
    )?;
    Ok(())
}
fn emit(
    tx: &Connection,
    number: u64,
    kind: &str,
    detail: Option<&str>,
    author: &str,
    body: Option<&str>,
    writer: Option<&Writer>,
) -> Result<()> {
    if EventKind::from_str(kind).is_none() {
        bail!("invalid local event kind {kind}");
    }
    let origin = if writer.is_some() { "agent" } else { "human" };
    let repository: String = tx.query_row(
        "SELECT repository FROM local_items WHERE number=?1",
        [sql_id(number)?],
        |r| r.get(0),
    )?;
    let own = writer.is_some_and(|w| w.item == item_node(&repository, number));
    let kind = if own && matches!(kind, "wake" | "invalidate") && detail != Some("cross_surface") {
        "origin_echo"
    } else {
        kind
    };
    tx.execute("INSERT INTO local_events(item_number,kind,detail,actor,body,revision,item_state,created_at,origin) SELECT number,?1,?2,?3,?4,revision,state,?5,?7 FROM local_items WHERE number=?6",params![kind,detail,author,body,now(),sql_id(number)?,origin])?;
    Ok(())
}
fn invalidate_links(
    tx: &Connection,
    issue: u64,
    author: &str,
    writer: Option<&Writer>,
) -> Result<()> {
    let ids = {
        let mut stmt = tx.prepare("SELECT pr_number FROM local_links WHERE issue_number=?1")?;
        stmt.query_map([sql_id(issue)?], |r| row_id(r, 0))?.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for id in ids {
        bump(tx, id)?;
        emit(tx, id, "invalidate", Some("cross_surface"), author, None, writer)?;
    }
    Ok(())
}

fn validate_writer(tx: &Connection, writer: Option<&Writer>) -> Result<()> {
    if let Some(w) = writer {
        let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM turns t JOIN provider_sessions s ON s.session_id=t.session_id JOIN agent_instances g ON g.agent_id=s.agent_id JOIN assignments a ON a.assignment_id=g.assignment_id WHERE t.turn_id=?1 AND s.session_id=?2 AND t.lifecycle IN ('starting','running') AND s.lifecycle='running' AND g.lifecycle IN ('idle','finalizing') AND a.lifecycle IN ('active','finalizing') AND a.work_item_node_id=?3)",params![w.turn,w.session,w.item],|r|r.get(0))?;
        ensure!(valid, "Agent writer assignment/session/turn is no longer active");
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct MergeResult {
    pub number: u64,
    pub merge_commit: String,
    pub base_ref: String,
    pub recovered: bool,
}

impl LocalObjects {
    pub fn merge_status(&self, number: u64) -> Result<Option<serde_json::Value>> {
        self.item(number)?;
        Ok(self.connection.query_row("SELECT expected_base,expected_head,merge_commit,lifecycle,error,created_at,updated_at FROM local_merges WHERE pr_number=?1",[sql_id(number)?],|r|Ok(serde_json::json!({"expected_base":r.get::<_,String>(0)?,"expected_head":r.get::<_,String>(1)?,"merge_commit":r.get::<_,Option<String>>(2)?,"lifecycle":r.get::<_,String>(3)?,"error":r.get::<_,Option<String>>(4)?,"created_at":r.get::<_,String>(5)?,"updated_at":r.get::<_,String>(6)?}))).optional()?)
    }

    pub fn diff(&self, number: u64, source: &Path) -> Result<String> {
        self.verify_source(source)?;
        let item = self.item(number)?;
        ensure!(item.kind == "pr", "#{number} is not a PR");
        let git = git2::Repository::open(source)?;
        let head = git
            .find_commit(git2::Oid::from_str(
                item.head_commit.as_deref().context("PR lacks head commit")?,
            )?)?
            .tree()?;
        let base = git
            .find_commit(git2::Oid::from_str(
                item.base_commit.as_deref().context("PR lacks base commit")?,
            )?)?
            .tree()?;
        let diff = git.diff_tree_to_tree(Some(&base), Some(&head), None)?;
        let mut output = Vec::new();
        diff.print(git2::DiffFormat::Patch, |_, _, line| {
            if matches!(line.origin(), ' ' | '+' | '-') {
                output.push(line.origin() as u8);
            }
            output.extend_from_slice(line.content());
            true
        })?;
        String::from_utf8(output).context("Git diff contains non-UTF-8 content")
    }

    pub fn merge(&mut self, number: u64, source: &Path, author: &str) -> Result<MergeResult> {
        self.bind_source(source)?;
        nonempty(author, "author")?;
        let item = self.item(number)?;
        ensure!(item.kind == "pr", "#{number} is not a PR");
        let git = git2::Repository::open(source)?;
        let base_ref = full_branch(item.base_ref.as_deref().context("PR lacks base ref")?)?;
        let head_ref = item.head_ref.as_deref().context("PR lacks head ref")?;
        let expected_base =
            git2::Oid::from_str(item.base_commit.as_deref().context("PR lacks base commit")?)?;
        let expected_head =
            git2::Oid::from_str(item.head_commit.as_deref().context("PR lacks head commit")?)?;
        let receipt: Option<(String, Option<String>)> = self
            .connection
            .query_row(
                "SELECT lifecycle,merge_commit FROM local_merges WHERE pr_number=?1",
                [sql_id(number)?],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((state, Some(commit))) = &receipt {
            if state == "applied" {
                return Ok(MergeResult {
                    number,
                    merge_commit: commit.clone(),
                    base_ref,
                    recovered: true,
                });
            }
            if state == "prepared" && git.refname_to_id(&base_ref)? == git2::Oid::from_str(commit)?
            {
                self.finish_merge(number, commit, author)?;
                return Ok(MergeResult {
                    number,
                    merge_commit: commit.clone(),
                    base_ref,
                    recovered: true,
                });
            }
        }
        ensure!(
            receipt.as_ref().is_none_or(|(s, _)| s == "prepared"),
            "previous merge attempt failed or conflicted; inspect its stored error and create a new PR candidate"
        );
        ensure!(item.state == "open", "PR #{number} is closed");
        ensure!(
            git.refname_to_id(&base_ref)? == expected_base,
            "base branch moved since this PR candidate was created"
        );
        ensure!(
            item.published
                || git.revparse_single(head_ref)?.peel_to_commit()?.id() == expected_head,
            "head revision moved since this PR candidate was created"
        );
        reject_checked_out(&git, &base_ref)?;
        let merge_commit = if let Some((_, Some(commit))) = receipt {
            git2::Oid::from_str(&commit)?
        } else {
            self.prepare_merge(&git, &item, expected_base, expected_head)?
        };
        self.adopt_merge_ref(&git, &item, &base_ref, merge_commit, expected_base, expected_head)?;
        self.finish_merge(number, &merge_commit.to_string(), author)?;
        Ok(MergeResult {
            number,
            merge_commit: merge_commit.to_string(),
            base_ref,
            recovered: false,
        })
    }

    fn adopt_merge_ref(
        &mut self,
        git: &git2::Repository,
        item: &Item,
        base_ref: &str,
        merge_commit: git2::Oid,
        expected_base: git2::Oid,
        expected_head: git2::Oid,
    ) -> Result<()> {
        let number = item.number;
        // The durable intent precedes Git CAS; restart recognizes only its exact commit.
        let updated = {
            let tx = write_transaction(&mut self.connection)?;
            validate_writer(&tx, self.writer.as_ref())?;
            validate_revision(&tx, &self.repository, self.revision_guard)?;
            let current = read_item(&tx, &self.repository, number)?;
            ensure!(
                current.state == "open"
                    && current.head_commit == item.head_commit
                    && current.base_commit == item.base_commit,
                "PR candidate changed before Git adoption"
            );
            let result = if git.refname_to_id(base_ref)? == merge_commit {
                Ok(())
            } else {
                git.reference_matching(
                    base_ref,
                    merge_commit,
                    true,
                    expected_base,
                    "Braid local PR merge",
                )
                .map(|_| ())
            };
            tx.commit()?;
            result
        };
        if let Err(error) = updated {
            self.record_merge(
                number,
                &expected_base.to_string(),
                &expected_head.to_string(),
                Some(&merge_commit.to_string()),
                "failed",
                Some(&error.to_string()),
            )?;
            return Err(error).context("base ref compare-and-swap failed; merge intent preserved");
        }
        Ok(())
    }

    fn prepare_merge(
        &mut self,
        git: &git2::Repository,
        item: &Item,
        expected_base: git2::Oid,
        expected_head: git2::Oid,
    ) -> Result<git2::Oid> {
        let base = git.find_commit(expected_base)?;
        let head = git.find_commit(expected_head)?;
        let merge_base = git.find_commit(git.merge_base(expected_base, expected_head)?)?;
        let mut index = git.merge_trees(&merge_base.tree()?, &base.tree()?, &head.tree()?, None)?;
        if index.has_conflicts() {
            let mut paths = std::collections::BTreeSet::new();
            for conflict in index.conflicts()? {
                let conflict = conflict?;
                for entry in [conflict.ancestor, conflict.our, conflict.their].into_iter().flatten()
                {
                    paths.insert(String::from_utf8_lossy(&entry.path).into_owned());
                }
            }
            let error = format!(
                "Git merge tree has unresolved conflicts in: {}",
                paths.into_iter().collect::<Vec<_>>().join(", ")
            );
            self.record_merge(
                item.number,
                &expected_base.to_string(),
                &expected_head.to_string(),
                None,
                "conflict",
                Some(&error),
            )?;
            bail!("{error}; base branch was not changed");
        }
        let tree_id = index.write_tree_to(git)?;
        let tree = git.find_tree(tree_id)?;
        let signature = git2::Signature::now("Braid", "braid@localhost")?;
        let commit = git.commit(
            None,
            &signature,
            &signature,
            &format!("Merge Braid PR #{}: {}", item.number, item.title),
            &tree,
            &[&base, &head],
        )?;
        self.record_merge(
            item.number,
            &expected_base.to_string(),
            &expected_head.to_string(),
            Some(&commit.to_string()),
            "prepared",
            None,
        )?;
        let (stored_base,stored_head,stored_commit,state):(String,String,Option<String>,String)=self.connection.query_row("SELECT expected_base,expected_head,merge_commit,lifecycle FROM local_merges WHERE pr_number=?1",[sql_id(item.number)?],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        ensure!(
            stored_base == expected_base.to_string() && stored_head == expected_head.to_string(),
            "another merge intent already owns a different candidate"
        );
        ensure!(
            matches!(state.as_str(), "prepared" | "applied"),
            "canonical merge intent is {state}; inspect its persisted failure"
        );
        git2::Oid::from_str(&stored_commit.context("canonical merge intent lacks a commit")?)
            .map_err(Into::into)
    }

    fn record_merge(
        &mut self,
        number: u64,
        base: &str,
        head: &str,
        commit: Option<&str>,
        state: &str,
        error: Option<&str>,
    ) -> Result<()> {
        let tx = write_transaction(&mut self.connection)?;
        validate_writer(&tx, self.writer.as_ref())?;
        validate_revision(&tx, &self.repository, self.revision_guard)?;
        tx.execute("INSERT INTO local_merges(pr_number,expected_base,expected_head,merge_commit,lifecycle,error,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?7) ON CONFLICT(pr_number) DO UPDATE SET lifecycle=excluded.lifecycle,error=excluded.error,updated_at=excluded.updated_at WHERE local_merges.lifecycle='prepared'",params![sql_id(number)?,base,head,commit,state,error,now()])?;
        tx.commit()?;
        Ok(())
    }

    fn finish_merge(&mut self, number: u64, commit: &str, author: &str) -> Result<()> {
        let tx = write_transaction(&mut self.connection)?;
        validate_writer(&tx, self.writer.as_ref())?;
        validate_revision(&tx, &self.repository, self.revision_guard)?;
        let applied=tx.execute("UPDATE local_merges SET lifecycle='applied',error=NULL,updated_at=?1 WHERE pr_number=?2 AND merge_commit=?3 AND lifecycle='prepared'",params![now(),sql_id(number)?,commit])?;
        if applied == 0 {
            let complete:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM local_merges WHERE pr_number=?1 AND merge_commit=?2 AND lifecycle='applied')",params![sql_id(number)?,commit],|r|r.get(0))?;
            ensure!(complete, "prepared merge intent no longer matches this commit");
            tx.commit()?;
            return Ok(());
        }
        tx.execute("UPDATE local_items SET state='closed' WHERE number=?1", [sql_id(number)?])?;
        bump(&tx, number)?;
        emit(&tx, number, "lifecycle", Some("merged"), author, None, self.writer.as_ref())?;
        tx.commit()?;
        Ok(())
    }

    pub fn apply_outbox(
        &mut self,
        intent: &crate::store::PendingGitHubWrite,
    ) -> Result<(Option<String>, Option<String>)> {
        ensure!(
            intent.repository == self.repository,
            "outbox intent belongs to another repository"
        );
        let tx = write_transaction(&mut self.connection)?;
        let existing = tx
            .query_row(
                "SELECT database_id,node_id FROM local_write_receipts WHERE intent_id=?1",
                [&intent.intent_id],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .optional()?;
        if let Some(receipt) = existing {
            return Ok(receipt);
        }
        let target = intent
            .target_database_id
            .parse::<u64>()
            .context("local outbox target must be numeric")?;
        let result = match intent.operation.as_str() {
            "comment_create" => {
                read_item(&tx, &self.repository, target)?;
                tx.execute("INSERT INTO local_comments(item_number,author,body,created_at,operational) VALUES(?1,'braid',?2,?3,1)",params![sql_id(target)?,intent.content,now()])?;
                let id = tx.last_insert_rowid().to_string();
                (Some(id.clone()), Some(format!("local:{}:comment:{id}", self.repository)))
            }
            "comment_update" => {
                ensure!(tx.execute("UPDATE local_comments SET body=?1 WHERE comment_id=?2 AND operational=1 AND item_number IN (SELECT number FROM local_items WHERE repository=?3)",params![intent.content,sql_id(target)?,self.repository])?==1,"operational local comment {target} does not exist");
                (
                    Some(target.to_string()),
                    Some(format!("local:{}:comment:{target}", self.repository)),
                )
            }
            "reaction_add" => {
                let visible:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM local_comments c JOIN local_items i ON i.number=c.item_number WHERE c.id=?1 AND i.repository=?2)",params![sql_id(target)?,self.repository],|r|r.get(0))?;
                ensure!(visible, "local reaction target comment {target} does not exist");
                tx.execute(
                    "INSERT OR IGNORE INTO local_reactions(comment_id,content) VALUES(?1,?2)",
                    params![sql_id(target)?, intent.content],
                )?;
                let id: i64 = tx.query_row(
                    "SELECT reaction_id FROM local_reactions WHERE comment_id=?1 AND content=?2",
                    params![sql_id(target)?, intent.content],
                    |r| r.get(0),
                )?;
                (Some(id.to_string()), None)
            }
            "reaction_delete" => {
                let id = intent
                    .remote_database_id
                    .as_deref()
                    .context("reaction delete lacks a reaction ID")?
                    .parse::<u64>()?;
                tx.execute(
                    "DELETE FROM local_reactions WHERE reaction_id=?1 AND comment_id=?2",
                    params![sql_id(id)?, sql_id(target)?],
                )?;
                (Some(id.to_string()), None)
            }
            operation => bail!("unsupported local outbox operation {operation}"),
        };
        tx.execute(
            "INSERT INTO local_write_receipts(intent_id,database_id,node_id) VALUES(?1,?2,?3)",
            params![intent.intent_id, result.0, result.1],
        )?;
        tx.commit()?;
        Ok(result)
    }
}

fn full_branch(value: &str) -> Result<String> {
    let branch =
        if value.starts_with("refs/heads/") { value.into() } else { format!("refs/heads/{value}") };
    ensure!(git2::Reference::is_valid_name(&branch), "invalid local base branch {value}");
    Ok(branch)
}
fn reject_checked_out(git: &git2::Repository, base: &str) -> Result<()> {
    if !git.is_bare() {
        ensure!(
            git.head().ok().and_then(|h| h.name().ok().map(str::to_owned)).as_deref() != Some(base),
            "base branch {base} is checked out; switch that worktree to another branch before merging"
        );
    }
    for name in &git.worktrees()? {
        let name = name?.context("Git worktree name is not UTF-8")?;
        let worktree = git.find_worktree(name)?;
        let checkout = git2::Repository::open(worktree.path())?;
        ensure!(
            checkout.head().ok().and_then(|h| h.name().ok().map(str::to_owned)).as_deref()
                != Some(base),
            "base branch {base} is checked out in {}; switch it to another branch before merging",
            worktree.path().display()
        );
    }
    Ok(())
}

fn sql_id(value: u64) -> Result<i64> {
    i64::try_from(value).context("object ID exceeds SQLite integer range")
}
fn row_id(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(e),
        )
    })
}

impl LocalObjects {
    /// Publish a concrete commit from the caller's checkout without moving its branch.
    pub fn publish(
        &mut self,
        number: u64,
        source: &Path,
        cwd: &Path,
        author: &str,
    ) -> Result<Item> {
        self.bind_source(source)?;
        nonempty(author, "author")?;
        let item = self.item(number)?;
        ensure!(item.kind == "pr" && item.state == "open", "publish requires an open PR");
        if let Some(writer) = &self.writer {
            ensure!(
                writer.item == item_node(&self.repository, number),
                "Agent may publish only its assigned PR"
            );
        }
        let git = git2::Repository::open(source)?;
        let checkout = git2::Repository::discover(cwd)?;
        ensure!(
            git.commondir().canonicalize()? == checkout.commondir().canonicalize()?,
            "publication checkout belongs to another Git repository"
        );
        let mut status_options = git2::StatusOptions::new();
        status_options.include_untracked(true).recurse_untracked_dirs(true);
        ensure!(
            checkout.statuses(Some(&mut status_options))?.is_empty(),
            "publication checkout has uncommitted changes; commit the complete candidate first"
        );
        let commit = checkout.head()?.peel_to_commit()?.id().to_string();
        ensure!(
            commit != item.base_commit.as_deref().unwrap_or_default(),
            "candidate is identical to the frozen base"
        );
        let tx = write_transaction(&mut self.connection)?;
        validate_writer(&tx, self.writer.as_ref())?;
        validate_revision(&tx, &self.repository, self.revision_guard)?;
        if let Some((worktree, node)) = &self.candidate_guard {
            let current:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM worktrees w JOIN agent_instances g ON g.agent_id=w.agent_id JOIN assignments a ON a.assignment_id=g.assignment_id WHERE w.worktree_id=?1 AND a.work_item_node_id=?2 AND a.lifecycle IN ('active','finalizing') AND w.lifecycle='active')",params![worktree,node],|r|r.get(0))?;
            ensure!(
                current,
                "candidate worktree no longer belongs to this PR's current assignment"
            );
        }
        if item.head_commit.as_deref() == Some(&commit) && item.published {
            tx.commit()?;
            return Ok(item);
        }
        let merging: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_merges WHERE pr_number=?1)",
            [sql_id(number)?],
            |r| r.get(0),
        )?;
        ensure!(
            !merging,
            "PR has a durable merge attempt; create another candidate PR instead of replacing its evidence"
        );
        ensure!(tx.execute("UPDATE local_items SET head_commit=?1,published=1 WHERE number=?2 AND revision=?3 AND state='open'",params![commit,sql_id(number)?,sql_id(item.revision)?])?==1,"PR changed while publishing; read it again before retrying");
        bump(&tx, number)?;
        emit(
            &tx,
            number,
            "wake",
            Some("head_published"),
            author,
            Some(&commit),
            self.writer.as_ref(),
        )?;
        tx.commit()?;
        self.item(number)
    }
}

enum WriteTransaction<'a> {
    Top(Transaction<'a>),
    Nested(Savepoint<'a>),
}
impl std::ops::Deref for WriteTransaction<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        match self {
            Self::Top(t) => t,
            Self::Nested(t) => t,
        }
    }
}
impl WriteTransaction<'_> {
    fn commit(self) -> rusqlite::Result<()> {
        match self {
            Self::Top(t) => t.commit(),
            Self::Nested(t) => t.commit(),
        }
    }
}
fn write_transaction(connection: &mut Connection) -> rusqlite::Result<WriteTransaction<'_>> {
    if connection.is_autocommit() {
        connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map(WriteTransaction::Top)
    } else {
        connection.savepoint().map(WriteTransaction::Nested)
    }
}

fn validate_revision(
    connection: &Connection,
    repository: &str,
    guard: Option<(u64, u64)>,
) -> Result<()> {
    if let Some((number, expected)) = guard {
        let actual = read_item(connection, repository, number)?.revision;
        ensure!(actual == expected, "object #{number} revision is {actual}, expected {expected}");
    }
    Ok(())
}

//! Durable object commands shared by HTTP control and the local authority.
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{PlatformObjects, now};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandRequest {
    pub request_id: String,
    pub expected_revision: Option<u64>,
    pub command: ObjectCommand,
    #[serde(default = "human_actor")]
    pub actor: String,
    pub writer: Option<super::super::grants::WriterGrant>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ObjectCommand {
    CreateIssue {
        title: String,
        body: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_revision: Option<u64>,
    },
    #[serde(rename = "createPR")]
    CreatePr {
        title: String,
        body: String,
        head: String,
        base: String,
        issue: Option<u64>,
        issue_revision: Option<u64>,
    },
    ConfigureRunRoot {
        number: u64,
        delivery_ref: String,
        run_id: String,
        client_id: String,
    },
    Comment {
        number: u64,
        body: String,
        reply_to: Option<u64>,
    },
    Edit {
        number: u64,
        title: Option<String>,
        body: Option<String>,
    },
    Assign {
        number: u64,
        #[serde(alias = "profile")]
        actor: String,
    },
    Unassign {
        number: u64,
    },
    Close {
        number: u64,
    },
    Reopen {
        number: u64,
    },
    Link {
        issue: u64,
        pr: u64,
        expected_pr_revision: u64,
    },
    Merge {
        number: u64,
    },
    Publish {
        number: u64,
        candidate: CandidateLocation,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CandidateLocation {
    Source,
    Commit { commit: String },
}

impl ObjectCommand {
    pub(crate) fn target(&self) -> Option<u64> {
        match self {
            Self::CreateIssue { .. } | Self::CreatePr { .. } => None,
            Self::ConfigureRunRoot { number, .. }
            | Self::Comment { number, .. }
            | Self::Edit { number, .. }
            | Self::Assign { number, .. }
            | Self::Unassign { number }
            | Self::Close { number }
            | Self::Reopen { number }
            | Self::Merge { number }
            | Self::Publish { number, .. } => Some(*number),
            Self::Link { issue, .. } => Some(*issue),
        }
    }
    pub(crate) fn operation(&self) -> &'static str {
        match self {
            Self::CreateIssue { .. } => "createIssue",
            Self::CreatePr { .. } => "createPR",
            Self::ConfigureRunRoot { .. } => "configureRunRoot",
            Self::Comment { .. } => "comment",
            Self::Edit { .. } => "edit",
            Self::Assign { .. } => "assign",
            Self::Unassign { .. } => "unassign",
            Self::Close { .. } => "close",
            Self::Reopen { .. } => "reopen",
            Self::Link { .. } => "link",
            Self::Merge { .. } => "merge",
            Self::Publish { .. } => "publish",
        }
    }
    fn has_external_effect(&self) -> bool {
        matches!(self, Self::Merge { .. } | Self::Publish { .. } | Self::ConfigureRunRoot { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlReceipt {
    pub request_id: String,
    pub state: String,
    pub result: Option<Value>,
    pub error: Option<CommandFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct CommandFailure {
    pub code: String,
    pub message: String,
}
impl CommandFailure {
    fn conflict(message: impl Into<String>) -> Self {
        Self { code: "conflict".into(), message: message.into() }
    }
    fn rejected(error: impl std::fmt::Display) -> Self {
        Self { code: "rejected".into(), message: error.to_string() }
    }
}
impl From<anyhow::Error> for CommandFailure {
    fn from(e: anyhow::Error) -> Self {
        Self::rejected(format!("{e:#}"))
    }
}

#[derive(Clone)]
pub struct ControlPolicy {
    pub source: PathBuf,
}

impl PlatformObjects {
    pub fn control_receipt(&self, request_id: &str) -> Result<Option<ControlReceipt>> {
        let row=self.connection.query_row("SELECT state,response_json,error FROM control_receipts WHERE scope='objects' AND request_id=?1",[request_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?))).optional()?;
        row.map(|(state, response, error)| {
            if let Some(response) = response {
                return serde_json::from_str(&response).context("decode durable control receipt");
            }
            Ok(ControlReceipt {
                request_id: request_id.into(),
                state,
                result: None,
                error: error.map(CommandFailure::rejected),
            })
        })
        .transpose()
    }

    pub fn apply_request(
        &mut self,
        request: &CommandRequest,
        policy: &ControlPolicy,
    ) -> Result<ControlReceipt, CommandFailure> {
        ensure_request_id(&request.request_id)?;
        self.bind_source(&policy.source)?;
        let payload = serde_json::to_string(request).map_err(CommandFailure::rejected)?;
        let digest = hex::encode(Sha256::digest(payload.as_bytes()));
        self.connection.execute_batch("BEGIN IMMEDIATE").map_err(CommandFailure::rejected)?;
        let prepared = self.prepare_control_request(request, &payload, &digest);
        match prepared {
            Ok(Some(receipt)) => {
                self.connection.execute_batch("COMMIT").map_err(CommandFailure::rejected)?;
                if matches!(receipt.state.as_str(), "prepared" | "blocked")
                    && matches!(request.command, ObjectCommand::ConfigureRunRoot { .. })
                {
                    return self.apply_external_request(request, policy);
                }
                return Ok(receipt);
            }
            Ok(None) => {}
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                return self.persist_rejection(request, &payload, &digest, error);
            }
        }
        self.writer = request
            .writer
            .as_ref()
            .map(|grant| {
                Ok::<_, CommandFailure>(super::Writer {
                    grant: grant.clone(),
                    item: super::super::grants::item_node(&self.connection, grant)?,
                    actor: request.actor.clone(),
                })
            })
            .transpose()?;
        if request.command.has_external_effect() {
            self.connection.execute_batch("COMMIT").map_err(CommandFailure::rejected)?;
            return self.apply_external_request(request, policy);
        }
        let result = self.apply_object_command(&request.command, policy, &request.actor);
        self.writer = None;
        match result {
            Ok(result) => {
                let receipt = ControlReceipt {
                    request_id: request.request_id.clone(),
                    state: "applied".into(),
                    result: Some(result),
                    error: None,
                };
                self.save_control_receipt(&receipt)?;
                self.connection.execute_batch("COMMIT").map_err(CommandFailure::rejected)?;
                Ok(receipt)
            }
            Err(error) => {
                self.connection.execute_batch("ROLLBACK").map_err(CommandFailure::rejected)?;
                self.persist_rejection(request, &payload, &digest, error.into())
            }
        }
    }

    fn prepare_control_request(
        &self,
        request: &CommandRequest,
        payload: &str,
        digest: &str,
    ) -> Result<Option<ControlReceipt>, CommandFailure> {
        let existing=self.connection.query_row("SELECT payload_sha256 FROM control_receipts WHERE scope='objects' AND request_id=?1",[&request.request_id],|r|r.get::<_,String>(0)).optional().map_err(CommandFailure::rejected)?;
        if let Some(existing) = existing {
            if existing != digest {
                return Err(CommandFailure::conflict(
                    "requestId was already used for a different command",
                ));
            }
            return self
                .control_receipt(&request.request_id)?
                .ok_or_else(|| CommandFailure::rejected("control receipt disappeared"))
                .map(Some);
        }
        self.check_request_revision(request)?;
        super::nonempty(&request.actor, "actor")?;
        if request.writer.is_some()
            && matches!(request.command, ObjectCommand::ConfigureRunRoot { .. })
        {
            return Err(CommandFailure::rejected("run scope policy is operator-only"));
        }
        if let Some(grant) = &request.writer {
            let target = match &request.command {
                ObjectCommand::CreatePr { issue, .. } => *issue,
                ObjectCommand::CreateIssue { parent, .. } => *parent,
                _ => request.command.target(),
            };
            if matches!(
                &request.command,
                ObjectCommand::CreateIssue { parent: None, .. }
                    | ObjectCommand::CreatePr { issue: None, .. }
            ) {
                return Err(CommandFailure::rejected(
                    "Agent creation requires an in-scope parent Issue",
                ));
            }
            if let ObjectCommand::Link { pr, .. } = &request.command {
                super::super::grants::validate(
                    &self.connection,
                    grant,
                    &request.actor,
                    &super::item_node(&self.repository, *pr),
                )?;
            }
            let item = target.map_or_else(
                || super::super::grants::item_node(&self.connection, grant),
                |number| Ok(super::item_node(&self.repository, number)),
            )?;
            super::super::grants::validate(&self.connection, grant, &request.actor, &item)?;
        }
        self.connection.execute("INSERT INTO control_receipts(scope,request_id,payload_sha256,request_json,state,created_at,updated_at) VALUES('objects',?1,?2,?3,'prepared',?4,?4)",params![request.request_id,digest,payload,now()]).map_err(CommandFailure::rejected)?;
        Ok(None)
    }

    fn check_request_revision(&self, request: &CommandRequest) -> Result<(), CommandFailure> {
        if let Some(number) = request.command.target() {
            let expected = request.expected_revision.ok_or_else(|| {
                CommandFailure::conflict(
                    "expectedRevision is required for an existing object command",
                )
            })?;
            self.expect_revision(number, expected)?;
        } else if request.expected_revision.is_some() {
            return Err(CommandFailure::rejected("creation does not accept expectedRevision"));
        }
        match &request.command {
            ObjectCommand::CreateIssue { parent: Some(parent), parent_revision, .. } => {
                let expected = parent_revision.ok_or_else(|| {
                    CommandFailure::conflict(
                        "parentRevision is required when creating a child Issue",
                    )
                })?;
                self.expect_revision(*parent, expected)?;
                let item = self.item(*parent)?;
                if item.kind != "issue" || item.state != "open" {
                    return Err(CommandFailure::rejected("parent must be an open Issue"));
                }
            }
            ObjectCommand::CreateIssue { parent: None, parent_revision: Some(_), .. } => {
                return Err(CommandFailure::rejected("parentRevision requires a parent Issue"));
            }
            ObjectCommand::Link { pr, expected_pr_revision, .. } => {
                self.expect_revision(*pr, *expected_pr_revision)?;
            }
            ObjectCommand::CreatePr { issue: Some(issue), issue_revision, .. } => {
                let expected = issue_revision.ok_or_else(|| {
                    CommandFailure::conflict(
                        "issueRevision is required when creating an associated PR",
                    )
                })?;
                self.expect_revision(*issue, expected)?;
            }
            ObjectCommand::CreatePr { issue: None, issue_revision: Some(_), .. } => {
                return Err(CommandFailure::rejected("issueRevision requires an associated issue"));
            }
            _ => {}
        }
        Ok(())
    }

    fn expect_revision(&self, number: u64, expected: u64) -> Result<(), CommandFailure> {
        let actual = self.item(number)?.revision;
        if actual != expected {
            return Err(CommandFailure::conflict(format!(
                "object #{number} revision is {actual}, expected {expected}"
            )));
        }
        Ok(())
    }

    fn apply_object_command(
        &mut self,
        command: &ObjectCommand,
        policy: &ControlPolicy,
        author: &str,
    ) -> Result<Value> {
        Ok(match command {
            ObjectCommand::CreateIssue { title, body, parent, .. } => {
                serde_json::to_value(self.create_child_issue(title, body, author, *parent)?)?
            }
            ObjectCommand::CreatePr { title, body, head, base, issue, .. } => serde_json::to_value(
                self.create_pr(title, body, author, &policy.source, head, base, *issue)?,
            )?,
            ObjectCommand::Comment { number, body, reply_to } => {
                serde_json::to_value(self.reply(*number, body, author, *reply_to)?)?
            }
            ObjectCommand::Edit { number, title, body } => serde_json::to_value(self.edit(
                *number,
                title.as_deref(),
                body.as_deref(),
                author,
            )?)?,
            ObjectCommand::Assign { number, actor } => {
                serde_json::to_value(self.assign(*number, Some(actor), author)?)?
            }
            ObjectCommand::Unassign { number } => {
                serde_json::to_value(self.assign(*number, None, author)?)?
            }
            ObjectCommand::Close { number } => {
                serde_json::to_value(self.set_state(*number, false, author)?)?
            }
            ObjectCommand::Reopen { number } => {
                serde_json::to_value(self.set_state(*number, true, author)?)?
            }
            ObjectCommand::Link { issue, pr, .. } => {
                self.link(*issue, *pr, author)?;
                serde_json::json!({"issue":issue,"pr":pr})
            }
            ObjectCommand::Merge { .. }
            | ObjectCommand::Publish { .. }
            | ObjectCommand::ConfigureRunRoot { .. } => {
                anyhow::bail!("external command requires its durable preparation boundary")
            }
        })
    }

    fn apply_external_request(
        &mut self,
        request: &CommandRequest,
        policy: &ControlPolicy,
    ) -> Result<ControlReceipt, CommandFailure> {
        self.revision_guard = request.command.target().zip(request.expected_revision);
        let result = match &request.command {
            ObjectCommand::ConfigureRunRoot { number, delivery_ref, run_id, client_id } => self.configure_run_root(*number, delivery_ref, run_id, client_id, &request.actor, &policy.source).map(|()| serde_json::json!({"root":number,"deliveryRef":delivery_ref,"runId":run_id,"clientId":client_id})),
            ObjectCommand::Merge { number } => self
                .merge(*number, &policy.source, &request.actor)
                .and_then(|r| serde_json::to_value(r).map_err(Into::into)),
            ObjectCommand::Publish { number, candidate } => {
                let commit = match candidate {
                    CandidateLocation::Source => source_candidate(&policy.source),
                    CandidateLocation::Commit { commit } => Ok(commit.clone()),
                };
                commit
                    .and_then(|commit| {
                        self.publish(*number, &policy.source, &commit, &request.actor)
                    })
                    .and_then(|r| serde_json::to_value(r).map_err(Into::into))
            }
            _ => unreachable!("only external commands reach this boundary"),
        };
        self.revision_guard = None;
        self.writer = None;
        let receipt = match result {
            Ok(result) => ControlReceipt {
                request_id: request.request_id.clone(),
                state: "applied".into(),
                result: Some(result),
                error: None,
            },
            Err(error) => ControlReceipt {
                request_id: request.request_id.clone(),
                state: "blocked".into(),
                result: None,
                error: Some(error.into()),
            },
        };
        self.save_control_receipt(&receipt)?;
        Ok(receipt)
    }

    fn save_control_receipt(&self, receipt: &ControlReceipt) -> Result<(), CommandFailure> {
        let response = serde_json::to_string(receipt).map_err(CommandFailure::rejected)?;
        let error = receipt.error.as_ref().map(|e| e.message.as_str());
        self.connection.execute("UPDATE control_receipts SET state=?1,response_json=?2,error=?3,updated_at=?4 WHERE scope='objects' AND request_id=?5",params![receipt.state,response,error,now(),receipt.request_id]).map_err(CommandFailure::rejected)?;
        Ok(())
    }

    fn persist_rejection(
        &self,
        request: &CommandRequest,
        payload: &str,
        digest: &str,
        error: CommandFailure,
    ) -> Result<ControlReceipt, CommandFailure> {
        let receipt = ControlReceipt {
            request_id: request.request_id.clone(),
            state: "rejected".into(),
            result: None,
            error: Some(error),
        };
        self.connection.execute("INSERT OR IGNORE INTO control_receipts(scope,request_id,payload_sha256,request_json,state,response_json,error,created_at,updated_at) VALUES('objects',?1,?2,?3,'rejected',?4,?5,?6,?6)",params![request.request_id,digest,payload,serde_json::to_string(&receipt).map_err(CommandFailure::rejected)?,receipt.error.as_ref().map(|e|e.message.as_str()),now()]).map_err(CommandFailure::rejected)?;
        let actual:String=self.connection.query_row("SELECT payload_sha256 FROM control_receipts WHERE scope='objects' AND request_id=?1",[&request.request_id],|r|r.get(0)).map_err(CommandFailure::rejected)?;
        if actual != digest {
            return Err(CommandFailure::conflict(
                "requestId was already used for a different command",
            ));
        }
        self.control_receipt(&request.request_id)?
            .context("rejected control receipt disappeared")
            .map_err(Into::into)
    }

    pub fn control_item(&self, number: u64) -> Result<Value> {
        let transaction = self.connection.unchecked_transaction()?;
        let item = serde_json::json!({"item":self.item(number)?,"comments":self.comments(number)?,"merge":self.merge_status(number)?,"parent":self.parent(number)?,"subIssues":self.children(number)?});
        transaction.commit()?;
        Ok(item)
    }

    pub fn control_snapshot(&self) -> Result<Value> {
        let transaction = self.connection.unchecked_transaction()?;
        let items=self.list()?.into_iter().map(|i|Ok(serde_json::json!({"item":i,"comments":self.comments(i.number)?,"merge":self.merge_status(i.number)?,"parent":self.parent(i.number)?,"subIssues":self.children(i.number)?}))).collect::<Result<Vec<_>>>()?;
        let snapshot =
            serde_json::json!({"source":"braid_platform","observedAt":now(),"items":items});
        transaction.commit()?;
        Ok(snapshot)
    }
}

fn ensure_request_id(id: &str) -> Result<(), CommandFailure> {
    if id.is_empty()
        || id.len() > 128
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(CommandFailure::rejected(
            "requestId must be 1–128 ASCII letters, digits, '-', '_' or '.'",
        ));
    }
    Ok(())
}
fn human_actor() -> String {
    "human".into()
}

fn source_candidate(source: &std::path::Path) -> Result<String> {
    let git = git2::Repository::open(source)?;
    let mut options = git2::StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true);
    anyhow::ensure!(
        git.statuses(Some(&mut options))?.is_empty(),
        "source checkout has uncommitted changes; commit the complete candidate first"
    );
    Ok(git.head()?.peel_to_commit()?.id().to_string())
}

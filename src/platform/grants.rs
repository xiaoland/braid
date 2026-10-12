use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriterGrant {
    pub grant_id: String,
    pub epoch: u64,
    pub token: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantRequest {
    pub request_id: String,
    pub client_id: String,
    pub item_number: u64,
    pub actor: String,
    pub expected_epoch: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub pending: bool,
}
// Serde's skip_serializing_if callback requires a borrowed field.
#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn is_false(value: &bool) -> bool {
    !*value
}
fn digest(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}
pub fn item_node(connection: &Connection, grant: &WriterGrant) -> Result<String> {
    Ok(connection.query_row("SELECT 'local:'||i.repository||':item:'||i.number FROM platform_grants g JOIN local_items i ON i.number=g.item_number WHERE g.grant_id=?1",[&grant.grant_id],|r|r.get(0))?)
}
pub fn validate(
    connection: &Connection,
    grant: &WriterGrant,
    actor: &str,
    item: &str,
) -> Result<()> {
    let valid: bool = connection.query_row(
        "WITH RECURSIVE owned(number) AS (
            SELECT i.number FROM platform_grants g JOIN local_items i ON i.number=g.item_number
            WHERE g.grant_id=?1 AND g.epoch=?2 AND g.token_sha256=?3
            AND NOT EXISTS (
                WITH RECURSIVE ancestors(number) AS (
                    SELECT i.number UNION SELECT issue_number FROM local_links WHERE pr_number=i.number
                    UNION SELECT p.parent_number FROM local_issue_parents p JOIN ancestors a ON p.child_number=a.number
                ) SELECT 1 FROM ancestors a JOIN run_root_policies policy ON policy.root_number=a.number
                  JOIN local_items root ON root.number=a.number WHERE root.state!='open' OR policy.client_id!=g.client_id
            )
            AND g.lifecycle='active' AND g.actor=?4 AND i.desired_profile=g.actor AND i.state='open'
            UNION SELECT p.child_number FROM local_issue_parents p JOIN owned o ON p.parent_number=o.number
        ), covered(number) AS (
            SELECT number FROM owned
            UNION SELECT l.pr_number FROM local_links l JOIN owned o ON l.issue_number=o.number
            UNION SELECT l.issue_number FROM local_links l JOIN platform_grants g ON l.pr_number=g.item_number
              JOIN owned o ON o.number=g.item_number WHERE g.grant_id=?1
        ) SELECT EXISTS(SELECT 1 FROM covered c JOIN local_items target ON target.number=c.number
          WHERE 'local:'||target.repository||':item:'||target.number=?5)",
        params![grant.grant_id,i64::try_from(grant.epoch)?,digest(&grant.token),actor,item],
        |r| r.get(0),
    )?;
    ensure!(valid, "writer grant is stale, revoked, or does not cover this actor/item");
    Ok(())
}
// Keep authorization, epoch transition and receipt in one transaction.
#[allow(clippy::too_many_lines)]
pub fn change(
    database: &std::path::Path,
    request: &GrantRequest,
    revoke: bool,
) -> Result<serde_json::Value> {
    ensure!(
        !request.request_id.is_empty()
            && !request.client_id.is_empty()
            && !request.actor.is_empty(),
        "grant request identity must not be empty"
    );
    ensure!(!request.pending || revoke, "pending is valid only for revoke");
    let mut connection = Connection::open(database)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let payload = serde_json::to_string(&(request, revoke))?;
    let hash = digest(&payload);
    let receipt:Option<(String,String)>=tx.query_row("SELECT payload_sha256,response_json FROM control_receipts WHERE scope='grants' AND request_id=?1",[&request.request_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((old, response)) = receipt {
        ensure!(old == hash, "requestId already names a different grant command");
        return Ok(serde_json::from_str(&response)?);
    }
    let number = i64::try_from(request.item_number)?;
    let (assigned, state): (Option<String>, String) = tx.query_row(
        "SELECT desired_profile,state FROM local_items WHERE number=?1",
        [number],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    ensure!(
        revoke
            || (state == "open" && assigned.as_ref().is_some_and(|actor| actor == &request.actor)),
        "writer grant requires an open object assigned to its actor"
    );
    if let Some(policy) = run_policy(&tx, request.item_number)? {
        ensure!(
            policy.client_id == request.client_id
                && request.run_id.as_deref() == Some(policy.run_id.as_str()),
            "writer identity does not own this registered run scope"
        );
        if !revoke {
            let open: bool = tx.query_row(
                "SELECT state='open' FROM local_items WHERE number=?1",
                [i64::try_from(policy.root_number)?],
                |r| r.get(0),
            )?;
            ensure!(open, "run root is closed; its writer scope is revoked");
        }
    }
    let current: Option<i64> = tx
        .query_row(
            "SELECT epoch FROM platform_grants WHERE client_id=?1 AND item_number=?2 AND actor=?3",
            params![request.client_id, number, request.actor],
            |r| r.get(0),
        )
        .optional()?;
    let already_revoked = if revoke {
        tx.query_row("SELECT lifecycle='revoked' AND (epoch>?4 OR (?5 AND epoch=?4)) FROM platform_grants WHERE client_id=?1 AND item_number=?2 AND actor=?3",params![request.client_id,number,request.actor,i64::try_from(request.expected_epoch)?,request.pending],|r|r.get::<_,bool>(0)).optional()?.unwrap_or(false)
    } else {
        false
    };
    if already_revoked {
        let result = serde_json::json!({"epoch":current,"lifecycle":"revoked"});
        tx.execute("INSERT INTO control_receipts(scope,request_id,payload_sha256,request_json,state,response_json,created_at,updated_at) VALUES('grants',?1,?2,?3,'applied',?4,datetime('now'),datetime('now'))",params![request.request_id,hash,payload,result.to_string()])?;
        tx.commit()?;
        return Ok(result);
    }
    let current_epoch = current.map(u64::try_from).transpose()?;
    let epoch = if revoke && request.pending {
        let minted_epoch = request
            .expected_epoch
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("grant epoch exhausted"))?;
        match current_epoch {
            None => minted_epoch,
            Some(actual) if actual == minted_epoch => {
                actual.checked_add(1).ok_or_else(|| anyhow::anyhow!("grant epoch exhausted"))?
            }
            _ => anyhow::bail!(
                "pending writer grant epoch changed; expected absent or minted {minted_epoch} current {current_epoch:?}"
            ),
        }
    } else {
        ensure!(
            current_epoch.unwrap_or(0) == request.expected_epoch,
            "writer grant epoch changed; expected {} current {}",
            request.expected_epoch,
            current_epoch.unwrap_or(0)
        );
        ensure!(!revoke || current.is_some(), "cannot revoke absent writer grant");
        request
            .expected_epoch
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("grant epoch exhausted"))?
    };
    let grant_id = uuid::Uuid::now_v7().to_string();
    let token = hex::encode(rand::random::<[u8; 32]>());
    let state = if revoke { "revoked" } else { "active" };
    let now =
        time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?;
    tx.execute("INSERT INTO platform_grants(client_id,item_number,actor,grant_id,epoch,token_sha256,lifecycle,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8) ON CONFLICT(client_id,item_number,actor) DO UPDATE SET grant_id=excluded.grant_id,epoch=excluded.epoch,token_sha256=excluded.token_sha256,lifecycle=excluded.lifecycle,updated_at=excluded.updated_at",params![request.client_id,number,request.actor,grant_id,i64::try_from(epoch)?,digest(&token),state,now])?;
    let result = if revoke {
        serde_json::json!({"epoch":epoch,"lifecycle":state})
    } else {
        serde_json::json!({"grantId":grant_id,"epoch":epoch,"token":token,"lifecycle":state})
    };
    tx.execute("INSERT INTO control_receipts(scope,request_id,payload_sha256,request_json,state,response_json,created_at,updated_at) VALUES('grants',?1,?2,?3,'applied',?4,?5,?5)",params![request.request_id,hash,payload,result.to_string(),now])?;
    tx.commit()?;
    Ok(result)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunPolicy {
    pub root_number: u64,
    pub delivery_ref: String,
    pub run_id: String,
    pub client_id: String,
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

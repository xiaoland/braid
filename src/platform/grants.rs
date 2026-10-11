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
    let valid:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM platform_grants g JOIN local_items i ON i.number=g.item_number WHERE g.grant_id=?1 AND g.epoch=?2 AND g.token_sha256=?3 AND g.lifecycle='active' AND g.actor=?4 AND i.desired_profile=g.actor AND i.state='open' AND ('local:'||i.repository||':item:'||i.number=?5 OR EXISTS(SELECT 1 FROM local_links l JOIN local_items target ON target.number=CASE WHEN l.issue_number=i.number THEN l.pr_number ELSE l.issue_number END WHERE (l.issue_number=i.number OR l.pr_number=i.number) AND 'local:'||target.repository||':item:'||target.number=?5)))",params![grant.grant_id,i64::try_from(grant.epoch)?,digest(&grant.token),actor,item],|r|r.get(0))?;
    ensure!(valid, "writer grant is stale, revoked, or does not cover this actor/item");
    Ok(())
}
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
    let current: Option<i64> = tx
        .query_row(
            "SELECT epoch FROM platform_grants WHERE client_id=?1 AND item_number=?2 AND actor=?3",
            params![request.client_id, number, request.actor],
            |r| r.get(0),
        )
        .optional()?;
    let already_revoked = if revoke {
        tx.query_row("SELECT lifecycle='revoked' AND epoch>?4 FROM platform_grants WHERE client_id=?1 AND item_number=?2 AND actor=?3",params![request.client_id,number,request.actor,i64::try_from(request.expected_epoch)?],|r|r.get::<_,bool>(0)).optional()?.unwrap_or(false)
    } else {
        false
    };
    if already_revoked {
        let result = serde_json::json!({"epoch":current,"lifecycle":"revoked"});
        tx.execute("INSERT INTO control_receipts(scope,request_id,payload_sha256,request_json,state,response_json,created_at,updated_at) VALUES('grants',?1,?2,?3,'applied',?4,datetime('now'),datetime('now'))",params![request.request_id,hash,payload,result.to_string()])?;
        tx.commit()?;
        return Ok(result);
    }
    ensure!(
        u64::try_from(current.unwrap_or(0))? == request.expected_epoch,
        "writer grant epoch changed; expected {} current {}",
        request.expected_epoch,
        current.unwrap_or(0)
    );
    ensure!(!revoke || current.is_some(), "cannot revoke absent writer grant");
    let epoch = request
        .expected_epoch
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("grant epoch exhausted"))?;
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

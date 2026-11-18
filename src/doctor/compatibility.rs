//! Read-only candidate-config preflight; never repairs an execution state.
use super::{Check, CheckState};
use crate::{
    config::Config,
    group::provider::{issue_system_prompt, pr_system_prompt},
};
use anyhow::{Context as _, Result, bail};
use rusqlite::Connection;
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;

struct Stored {
    repository: String,
    kind: String,
    number: i64,
    generation: i64,
    assignment: String,
    agent: String,
    profile: String,
    digest: String,
    session: Option<String>,
    instructions: Option<String>,
    head: Option<String>,
}
pub(super) fn check(config: &Config) -> Check {
    match inspect(config) {
        Ok(messages) if messages.is_empty()=>Check{name:"persisted Session compatibility".into(),state:CheckState::Pass,detail:"current candidate materials match retained eligible Sessions; this is not execution-stop or tool-auth proof".into()},
        Ok(messages)=>Check{name:"persisted Session compatibility".into(),state:CheckState::Fail,detail:messages.join("; ")},
        Err(error)=>Check{name:"persisted Session compatibility".into(),state:CheckState::Fail,detail:error.to_string()},
    }
}
fn inspect(config: &Config) -> Result<Vec<String>> {
    if !config.runtime.database().exists() {
        return Ok(Vec::new());
    }
    let db = Connection::open_with_flags(
        config.runtime.database(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    let mut query=db.prepare("SELECT r.name_with_owner,w.kind,w.number,a.generation,a.lifecycle,g.lifecycle,g.profile_id,p.effective_digest,s.lifecycle,s.instruction_revision,t.head_ref FROM assignments a JOIN work_items w ON w.node_id=a.work_item_node_id JOIN repositories r ON r.node_id=w.repository_node_id JOIN agent_instances g ON g.assignment_id=a.assignment_id JOIN profiles p ON p.profile_id=g.profile_id AND p.revision=g.profile_revision LEFT JOIN provider_sessions s ON s.agent_id=g.agent_id AND s.rowid=(SELECT max(newest.rowid) FROM provider_sessions newest WHERE newest.agent_id=g.agent_id) LEFT JOIN worktrees t ON t.agent_id=g.agent_id WHERE a.generation=(SELECT max(latest.generation) FROM assignments latest WHERE latest.work_item_node_id=w.node_id) AND (a.lifecycle IN ('active','materializing','finalizing') OR (a.lifecycle='blocked' AND lower(w.state)='open')) ORDER BY r.name_with_owner,w.number")?;
    let rows = query
        .query_map([], |r| {
            Ok(Stored {
                repository: r.get(0)?,
                kind: r.get(1)?,
                number: r.get(2)?,
                generation: r.get(3)?,
                assignment: r.get(4)?,
                agent: r.get(5)?,
                profile: r.get(6)?,
                digest: r.get(7)?,
                session: r.get(8)?,
                instructions: r.get(9)?,
                head: r.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut materials = HashMap::new();
    let mut messages = Vec::new();
    for stored in rows {
        let number =
            u64::try_from(stored.number).context("retained Work Item number is invalid")?;
        let target =
            format!("{}#{} generation {}", stored.repository, stored.number, stored.generation);
        let profile = if config.is_builtin() {
            config.profile(&stored.profile)?
        } else {
            match stored.kind.as_str() {
                "issue" => config.issue_profile_for_repository(&stored.repository)?,
                "pr" => config.pr_profile_for_repository(&stored.repository)?,
                other => bail!("unsupported retained Work Item kind {other}"),
            }
        };
        if config.profile_repository(profile) != stored.repository || !profile.has_tag(&stored.kind)
        {
            messages.push(format!(
                "{target}: retained Profile no longer belongs to this repository and role"
            ));
            continue;
        }
        let digest = match materials.entry(profile.id.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => entry.insert(
                config
                    .profile_materials(profile)
                    .with_context(|| format!("cannot inspect candidate Profile {}", profile.id))?
                    .digest,
            ),
        };
        if profile.id != stored.profile || *digest != stored.digest {
            messages.push(format!("{target}: candidate Profile {} does not match retained {}; switching would prevent Session reuse; keep compatible material or explicitly reactivate a new generation",profile.id,stored.profile));
            continue;
        }
        if stored.assignment == "blocked"
            || stored.agent == "blocked"
            || stored.session.as_deref() == Some("blocked")
        {
            messages.push(format!("{target}: materials match but execution is persistently blocked; configuration rollback is not recovery; explicit verified recovery is required (assignment reactivate creates a new generation)"));
            continue;
        }
        if stored.session.as_deref() == Some("unknown") {
            messages.push(format!("{target}: execution outcome is unknown; matching materials are not proof of stop or permission to resume"));
            continue;
        }
        // Built-in finite runs add root/run-specific instructions which an
        // offline config alone cannot reconstruct. Its runtime owns that gate.
        if config.is_builtin() {
            continue;
        }
        let Some(expected) = stored.instructions else { continue };
        let prompt = if stored.kind == "issue" {
            issue_system_prompt(config, profile, number)
        } else {
            pr_system_prompt(
                config,
                profile,
                number,
                stored.head.as_deref().context("retained PR Session has no worktree head")?,
            )
        };
        if hex::encode(Sha256::digest(prompt.as_bytes())) != expected {
            messages.push(format!("{target}: candidate instructions do not match retained Session; no implicit history reuse is safe"));
        }
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::tests::Fixture;
    fn compatible() -> Fixture {
        let f = Fixture::new(true);
        let profile = f.config.profile("issue-codex").unwrap();
        f.store
            .register_profile(
                crate::group::provider::materialized_profile(&f.config, profile).unwrap(),
            )
            .unwrap();
        let revision =
            hex::encode(Sha256::digest(issue_system_prompt(&f.config, profile, 1).as_bytes()));
        f.db().execute("UPDATE provider_sessions SET instruction_revision=?1", [revision]).unwrap();
        f
    }
    #[test]
    fn candidate_mismatch_is_reported_without_poisoning_retained_generation() {
        let f = compatible();
        assert_eq!(check(&f.config).state, CheckState::Pass);
        let mut candidate = f.config.clone();
        candidate.profiles.iter_mut().find(|p| p.id == "issue-codex").unwrap().model =
            Some("changed-model".into());
        assert_eq!(check(&candidate).state, CheckState::Fail);
        assert_eq!(
            f.db()
                .query_row(
                    "SELECT lifecycle FROM assignments WHERE assignment_id='assignment'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "active"
        );
        assert_eq!(check(&f.config).state, CheckState::Pass);
    }
    #[test]
    fn matching_material_is_not_a_reason_to_clear_a_persistent_execution_block() {
        let f = compatible();
        f.store
            .block_provider_session(
                "provider".into(),
                "explicit stop/compatibility recovery is required".into(),
            )
            .unwrap();
        let report = check(&f.config);
        assert_eq!(report.state, CheckState::Fail);
        assert!(report.detail.contains("rollback is not recovery"));
        assert_eq!(
            f.db()
                .query_row(
                    "SELECT lifecycle FROM assignments WHERE assignment_id='assignment'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "blocked"
        );
    }
    #[test]
    fn builtin_run_instructions_are_not_guessed_from_an_ordinary_config() {
        let mut f = Fixture::new(true);
        f.config.github = None;
        f.config.builtin = Some(crate::config::BuiltinConfig {
            repository: "owner/repository".into(),
            endpoint: "http://127.0.0.1:3000".parse().unwrap(),
        });
        let profile = f.config.profile("issue-codex").unwrap();
        let record = crate::group::provider::materialized_profile(&f.config, profile).unwrap();
        let revision = i64::try_from(record.revision).unwrap();
        f.store.register_profile(record).unwrap();
        f.db().execute("UPDATE agent_instances SET profile_revision=?1", [revision]).unwrap();
        f.db().execute("UPDATE provider_sessions SET instruction_revision='finite-run-specific-instructions'",[]).unwrap();
        assert_eq!(check(&f.config).state, CheckState::Pass);
    }
}

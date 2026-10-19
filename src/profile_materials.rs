use std::{
    collections::BTreeMap,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::config::{Config, Profile};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct EffectiveMaterials {
    pub digest: String,
    pub revision: u64,
    pub source: Option<String>,
    pub files: BTreeMap<String, String>,
    pub skills: Vec<SkillIdentity>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SkillIdentity {
    pub name: String,
    pub digest: String,
}

struct Collected {
    identity: EffectiveMaterials,
    native_files: BTreeMap<PathBuf, Vec<u8>>,
    skill_files: BTreeMap<PathBuf, Vec<u8>>,
}

struct CollectedSkills {
    files: BTreeMap<PathBuf, Vec<u8>>,
    identities: Vec<SkillIdentity>,
}

struct SkillSource {
    name: String,
    files: BTreeMap<PathBuf, Vec<u8>>,
}

pub(crate) fn collect(config: &Config, profile: &Profile) -> Result<EffectiveMaterials> {
    Ok(collect_all(config, profile)?.identity)
}

pub(crate) fn snapshot(
    config: &Config,
    profile: &Profile,
    workspace: &Path,
    home_key: &str,
    expected_digest: &str,
) -> Result<PathBuf> {
    let collected = collect_all(config, profile)?;
    if collected.identity.digest != expected_digest {
        bail!(
            "Profile materials changed before Pi spawn: expected {expected_digest}, found {}",
            collected.identity.digest
        );
    }
    let parsed_key = uuid::Uuid::parse_str(home_key).context("Pi native home key is not a UUID")?;
    let home_key = parsed_key.to_string();
    let destination = workspace.join(".braid/pi-homes").join(&home_key);
    let manifest = serde_json::to_vec(&collected.identity)?;
    let skill_root = destination.join("braid-skills");
    if destination.exists() {
        let existing = fs::read(destination.join(".braid-materials.json")).with_context(|| {
            format!("existing Pi native home {} has no identity record", destination.display())
        })?;
        if existing != manifest {
            bail!(
                "existing Pi native home {} has different profile material identity; refusing to overwrite it",
                destination.display()
            );
        }
        validate_snapshot_entries(&destination, &collected)?;
        for (relative, bytes) in &collected.native_files {
            let expected = materialized_native_file(relative, bytes, &skill_root)?;
            let snapshot_path = destination.join(relative);
            let matches = if relative == Path::new("settings.json") {
                let actual = fs::read(&snapshot_path).with_context(|| {
                    format!("cannot read Pi settings snapshot {}", snapshot_path.display())
                })?;
                let actual = settings_semantics(&actual, profile).with_context(|| {
                    format!("cannot interpret Pi settings snapshot {}", snapshot_path.display())
                })?;
                let expected = settings_semantics(&expected, profile)
                    .context("cannot interpret collected Pi settings material")?;
                actual == expected
            } else {
                fs::read(&snapshot_path).with_context(|| {
                    format!("cannot read Pi material snapshot {}", snapshot_path.display())
                })? == expected
            };
            if !matches {
                bail!(
                    "Pi native material snapshot changed at {}; refusing to reuse it",
                    snapshot_path.display()
                );
            }
        }
        for (relative, bytes) in &collected.skill_files {
            let path = skill_root.join(relative);
            let actual = fs::read(&path)
                .with_context(|| format!("cannot read Pi skill snapshot {}", path.display()))?;
            if actual.as_slice() != bytes.as_slice() {
                bail!("Pi skill snapshot changed at {}; refusing to reuse it", path.display());
            }
        }
        return Ok(destination);
    }

    fs::create_dir_all(&destination)?;
    for (relative, bytes) in &collected.native_files {
        let expected = materialized_native_file(relative, bytes, &skill_root)?;
        write_new(&destination.join(relative), &expected)?;
    }
    for (relative, bytes) in &collected.skill_files {
        write_new(&skill_root.join(relative), bytes)?;
    }
    for skill in &collected.identity.skills {
        let entry = skill_root.join(&skill.name).join("SKILL.md");
        if !entry.is_file() {
            bail!("materialized skill {} has no SKILL.md entrypoint", skill.name);
        }
    }
    write_new(&destination.join(".braid-materials.json"), &manifest)?;
    Ok(destination)
}

fn validate_snapshot_entries(root: &Path, collected: &Collected) -> Result<()> {
    let mut expected = std::collections::BTreeSet::new();
    expected.extend(collected.native_files.keys().cloned());
    expected
        .extend(collected.skill_files.keys().map(|path| PathBuf::from("braid-skills").join(path)));
    let mut actual = std::collections::BTreeSet::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let controlled_file = name == "models.json" || name == "settings.json";
        let controlled_tree = name == "agents" || name == "braid-skills";
        if !controlled_file && !controlled_tree {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            bail!("Pi native material snapshot contains symlink {}", path.display());
        }
        if controlled_file && kind.is_file() {
            actual.insert(path.strip_prefix(root)?.to_path_buf());
        } else if controlled_tree && kind.is_dir() {
            visit_tree(root, &path, &mut actual)?;
        } else {
            bail!("Pi native material snapshot has an invalid entry at {}", path.display());
        }
    }
    if actual != expected {
        bail!("Pi native home snapshot file set changed; refusing to reuse it");
    }
    Ok(())
}

fn visit_tree(
    root: &Path,
    current: &Path,
    actual: &mut std::collections::BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            bail!("Pi native home snapshot contains symlink {}", path.display());
        }
        if kind.is_dir() {
            visit_tree(root, &path, actual)?;
        } else if kind.is_file() {
            actual.insert(path.strip_prefix(root)?.to_path_buf());
        } else {
            bail!("Pi native home snapshot contains unsupported entry {}", path.display());
        }
    }
    Ok(())
}

pub(crate) fn bind_session_home(workspace: &Path, session: &str, home_key: &str) -> Result<()> {
    let parsed_key = uuid::Uuid::parse_str(home_key).context("Pi native home key is not a UUID")?;
    let index = workspace.join(".braid/pi-homes/index");
    fs::create_dir_all(&index)?;
    let key_path = index.join(sha(session.as_bytes()));
    let key = parsed_key.to_string();
    if key_path.exists() {
        let existing = fs::read_to_string(&key_path)?;
        if existing.trim() != key {
            bail!("Pi session {session} is already bound to a different native home");
        }
        return Ok(());
    }
    write_new(&key_path, key.as_bytes())
}

pub(crate) fn session_home_key(workspace: &Path, session: &str) -> Result<String> {
    let key_path = workspace.join(".braid/pi-homes/index").join(sha(session.as_bytes()));
    let key = fs::read_to_string(&key_path).with_context(|| {
        format!("Pi session {session} has no native-home identity record; refusing to guess its prior profile materials")
    })?;
    let parsed =
        uuid::Uuid::parse_str(key.trim()).context("Pi native home index has an invalid UUID")?;
    Ok(parsed.to_string())
}

fn materialized_native_file(relative: &Path, bytes: &[u8], skill_root: &Path) -> Result<Vec<u8>> {
    if relative.parent() != Some(Path::new("agents")) {
        return Ok(bytes.to_vec());
    }
    let mut text = String::from_utf8(bytes.to_vec())?;
    text = text.replace("@BRAID_PROFILE_SKILLS_ROOT@", &skill_root.to_string_lossy());
    Ok(text.into_bytes())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().context("material path has no parent")?)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("cannot create immutable Pi material {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn collect_all(config: &Config, profile: &Profile) -> Result<Collected> {
    let runtime = config.runtime_for(profile)?;
    let llm = config.llm_provider_for(profile)?;
    let skills = collect_profile_skills(profile)?;
    let skill_files = skills.files;
    let skill_identities = skills.identities;
    let native_source = if profile.adapter_type != "pi" {
        None
    } else if let Some(template) = &profile.native_template {
        Some(template)
    } else {
        runtime.home.as_ref().filter(|home| home.exists())
    };
    let mut native_files = BTreeMap::new();
    let mut material_identities = BTreeMap::new();
    if let Some(source) = native_source {
        let root = source.canonicalize().with_context(|| {
            format!("cannot resolve Pi native material source {}", source.display())
        })?;
        for relative in [PathBuf::from("models.json"), PathBuf::from("settings.json")] {
            let path = root.join(&relative);
            if path.exists() {
                if fs::symlink_metadata(&path)?.file_type().is_symlink() {
                    bail!("Pi native template cannot contain symlinks: {}", path.display());
                }
                let bytes =
                    fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
                validate_json_material(
                    &relative,
                    &bytes,
                    profile,
                    llm.api_key_environment.as_deref(),
                    profile.native_template.is_some(),
                )?;
                let identity_bytes = if relative == Path::new("settings.json") {
                    settings_semantics(&bytes, profile)?
                } else {
                    bytes.clone()
                };
                material_identities.insert(relative.display().to_string(), sha(&identity_bytes));
                native_files.insert(relative, bytes);
            }
        }
        let agents = root.join("agents");
        if agents.exists() {
            if fs::symlink_metadata(&agents)?.file_type().is_symlink() || !agents.is_dir() {
                bail!("Pi agents material must be a directory: {}", agents.display());
            }
            for entry in fs::read_dir(&agents)? {
                let entry = entry?;
                let path = entry.path();
                let kind = entry.file_type()?;
                if kind.is_symlink()
                    || !kind.is_file()
                    || path.extension().is_none_or(|ext| ext != "md")
                {
                    bail!(
                        "Pi native template supports only regular agents/*.md files: {}",
                        path.display()
                    );
                }
                let relative = PathBuf::from("agents").join(entry.file_name());
                let bytes = fs::read(&path)?;
                let text = std::str::from_utf8(&bytes)
                    .with_context(|| format!("Pi agent file is not UTF-8: {}", path.display()))?;
                let bytes = normalize_agent_skills(text, profile, &skill_identities)?.into_bytes();
                material_identities.insert(relative.display().to_string(), sha(&bytes));
                native_files.insert(relative, bytes);
            }
        }
        if profile.native_template.is_some() {
            for entry in fs::read_dir(&root)? {
                let entry = entry?;
                let name = entry.file_name();
                if entry.file_type()?.is_symlink() {
                    bail!("Pi native template cannot contain symlinks: {}", entry.path().display());
                }
                if name != "models.json" && name != "settings.json" && name != "agents" {
                    bail!("unsupported Pi native template entry {}", entry.path().display());
                }
            }
        }
    }

    let mut semantic = serde_json::to_value(profile)?;
    semantic["skills"] = serde_json::Value::Null;
    semantic["native_template"] = serde_json::Value::Null;
    let runtime_manifest = runtime.native_runtime.as_ref().map(fs::read).transpose()?;
    let manifest_hash = runtime_manifest.as_deref().map(sha);
    let runtime_api_url = runtime.api_url.as_deref().map(sanitized_runtime_api_url).transpose()?;
    let digest_input = serde_json::json!({
        "profile": semantic,
        "adapter": {
            "type": runtime.adapter_type,
            "version": runtime.version,
            "native_runtime": manifest_hash,
            "api_url": runtime_api_url,
            "stable_schema_sha256": runtime.stable_schema_sha256,
            "experimental_schema_sha256": runtime.experimental_schema_sha256,
        },
        "provider_route": {
            "provider": llm.id,
            "protocol": llm.protocol,
            "api_key_environment": llm.api_key_environment,
            "model": profile.model,
            "reasoning": profile.reasoning
        },
        "native_files": material_identities,
        "skills": skill_identities,
    });
    let digest = sha(&serde_json::to_vec(&digest_input)?);
    let revision = u64::from_str_radix(&digest[..15], 16)?.max(1);
    Ok(Collected {
        identity: EffectiveMaterials {
            digest,
            revision,
            source: native_source.map(|_| {
                if profile.native_template.is_some() { "profile_template" } else { "runtime_home" }
                    .to_owned()
            }),
            files: material_identities,
            skills: skill_identities,
        },
        native_files,
        skill_files,
    })
}

fn collect_profile_skills(profile: &Profile) -> Result<CollectedSkills> {
    let mut skill_files = BTreeMap::new();
    let mut skill_identities = Vec::new();
    for declared in &profile.skills {
        for SkillSource { name, files } in collect_skill_source(declared)? {
            if skill_identities.iter().any(|skill: &SkillIdentity| skill.name == name) {
                bail!("duplicate Pi skill name {name:?} across Profile.skills sources");
            }
            let fingerprint = tree_digest(&files)?;
            skill_identities.push(SkillIdentity { name: name.clone(), digest: fingerprint });
            for (relative, bytes) in files {
                let path = PathBuf::from(&name).join(relative);
                if skill_files.insert(path.clone(), bytes).is_some() {
                    bail!("duplicate Pi skill material path {}", path.display());
                }
            }
        }
    }
    Ok(CollectedSkills { files: skill_files, identities: skill_identities })
}

fn read_tree(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    fn visit(root: &Path, current: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) -> Result<()> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                bail!("Pi skill trees cannot contain symlinks: {}", path.display());
            }
            if kind.is_dir() {
                visit(root, &path, files)?;
            } else if kind.is_file() {
                let relative = path.strip_prefix(root)?.to_path_buf();
                files.insert(relative, fs::read(&path)?);
            } else {
                bail!("unsupported Pi skill material: {}", path.display());
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files)?;
    Ok(files)
}

fn collect_skill_source(path: &Path) -> Result<Vec<SkillSource>> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        bail!("Pi skill source cannot be a symlink: {}", path.display());
    }
    let resolved = if path.is_file() {
        if path.file_name().is_none_or(|name| name != "SKILL.md") {
            bail!("Pi skill file must be named SKILL.md: {}", path.display());
        }
        path.parent().context("Pi skill file has no parent")?.canonicalize()?
    } else {
        path.canonicalize()
            .with_context(|| format!("cannot resolve Pi skill source {}", path.display()))?
    };
    if resolved.join("SKILL.md").is_file() {
        let name = pi_skill_name(&resolved)?;
        return Ok(vec![SkillSource { name, files: read_tree(&resolved)? }]);
    }
    let mut entries: Vec<_> = fs::read_dir(&resolved)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let mut skills = Vec::new();
    for entry in entries {
        if entry.file_type()?.is_symlink() {
            bail!("Pi skill catalogs cannot contain symlinks: {}", entry.path().display());
        }
        if entry.file_type()?.is_dir() && entry.path().join("SKILL.md").is_file() {
            let name = pi_skill_name(&entry.path())?;
            skills.push(SkillSource { name, files: read_tree(&entry.path())? });
        }
    }
    if skills.is_empty() {
        bail!(
            "Pi skill source must be a SKILL.md file, a skill directory, or a catalog of skill directories: {}",
            resolved.display()
        );
    }
    Ok(skills)
}

fn pi_skill_name(directory: &Path) -> Result<String> {
    let entry = directory.join("SKILL.md");
    if fs::symlink_metadata(&entry)?.file_type().is_symlink() {
        bail!("Pi skill entrypoint cannot be a symlink: {}", entry.display());
    }
    let contents = fs::read_to_string(&entry)
        .with_context(|| format!("cannot read Pi skill entrypoint {}", entry.display()))?;
    let contents = normalize_text_newlines(&contents);
    let (frontmatter, _) = contents
        .strip_prefix("---\n")
        .and_then(|text| text.split_once("\n---"))
        .with_context(|| format!("Pi skill {} must have frontmatter", entry.display()))?;
    let field = |field: &str| {
        frontmatter.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == field).then(|| value.trim().trim_matches(['"', '\'']).to_owned())
        })
    };
    let name = field("name").filter(|name| !name.is_empty()).unwrap_or_else(|| {
        directory.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_owned()
    });
    let description = field("description").unwrap_or_default();
    let valid = name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !valid || description.trim().is_empty() {
        bail!(
            "Pi skill {} must have a valid Agent Skill name and non-empty description",
            entry.display()
        );
    }
    Ok(name)
}

fn sanitized_runtime_api_url(value: &str) -> Result<String> {
    let mut url = url::Url::parse(value).context("runtime api_url must be a valid URL")?;
    url.set_username("")
        .map_err(|()| anyhow::anyhow!("runtime api_url cannot sanitize userinfo"))?;
    url.set_password(None)
        .map_err(|()| anyhow::anyhow!("runtime api_url cannot sanitize userinfo"))?;
    if url.query().is_some() {
        let empty_query = url.query().is_some_and(str::is_empty);
        let query_count = url.query_pairs().count();
        let safe_pairs: Vec<_> = url
            .query_pairs()
            .filter(|(key, _)| !sensitive_query_name(key))
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        if safe_pairs.len() != query_count || empty_query {
            url.set_query(None);
            if !safe_pairs.is_empty() {
                url.query_pairs_mut().extend_pairs(safe_pairs);
            }
        }
    }
    Ok(url.to_string())
}

fn sensitive_header_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "authorization",
        "proxy-authorization",
        "api-key",
        "api_key",
        "apikey",
        "x-api-key",
        "x-auth-token",
        "ocp-apim-subscription-key",
        "x-goog-api-key",
        "auth-token",
        "cookie",
        "set-cookie",
    ]
    .iter()
    .any(|exact| name == *exact)
}

fn sensitive_query_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "api-key",
        "api_key",
        "apikey",
        "auth",
        "authorization",
        "token",
        "secret",
        "password",
        "credential",
        "signature",
        "key",
        "access-token",
        "access_token",
    ]
    .iter()
    .any(|exact| name == *exact)
}

fn supported_header_value(header: &str, value: &str, api_key_environment: Option<&str>) -> bool {
    if value.starts_with('!') {
        return false;
    }
    let sensitive = sensitive_header_name(header);
    if !value.contains('$') {
        return !sensitive;
    }
    let matches_profile_key = |reference: &str| {
        environment_reference_name(reference) == api_key_environment
            && api_key_environment.is_some()
    };
    matches_profile_key(value)
        || (header.eq_ignore_ascii_case("authorization")
            && value.strip_prefix("Bearer ").is_some_and(matches_profile_key))
}

fn environment_reference_name(value: &str) -> Option<&str> {
    let name = value
        .strip_prefix("${")
        .and_then(|name| name.strip_suffix('}'))
        .or_else(|| value.strip_prefix('$'))?;
    valid_environment_reference(name).then_some(name)
}

fn tree_digest(files: &BTreeMap<PathBuf, Vec<u8>>) -> Result<String> {
    let normalized: BTreeMap<_, _> = files.iter().map(|(path, bytes)| (path, sha(bytes))).collect();
    Ok(sha(&serde_json::to_vec(&normalized)?))
}

fn validate_json_material(
    path: &Path,
    bytes: &[u8],
    profile: &Profile,
    api_key_environment: Option<&str>,
    reject_root_model: bool,
) -> Result<()> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).with_context(|| format!("invalid Pi {}", path.display()))?;
    reject_ambient_authority(&value, path, api_key_environment, reject_root_model)?;
    if path == Path::new("models.json")
        && let Some(model) = &profile.model
    {
        let providers = value
            .get("providers")
            .and_then(serde_json::Value::as_object)
            .context("Pi models.json must contain providers when selected as profile material")?;
        let descriptor = providers.get(&profile.provider).with_context(|| {
            format!("Pi models.json does not define selected provider {:?}", profile.provider)
        })?;
        let models = descriptor
            .get("models")
            .and_then(serde_json::Value::as_array)
            .context("selected Pi provider descriptor has no models array")?;
        if !models
            .iter()
            .any(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some(model))
        {
            bail!(
                "Pi models.json does not define selected root model {:?} for provider {:?}",
                model,
                profile.provider
            );
        }
    }
    Ok(())
}

fn reject_ambient_authority(
    value: &serde_json::Value,
    path: &Path,
    api_key_environment: Option<&str>,
    reject_root_model: bool,
) -> Result<()> {
    fn visit(
        value: &serde_json::Value,
        path: &Path,
        api_key_environment: Option<&str>,
        reject_root_model: bool,
        root: bool,
    ) -> Result<()> {
        match value {
            serde_json::Value::Object(object) => {
                for (key, value) in object {
                    let forbidden = if let Some("models.json") =
                        path.file_name().and_then(|name| name.to_str())
                    {
                        key == "apiKey"
                            && value.as_str().is_none_or(|key| {
                                !valid_environment_reference(key)
                                    || api_key_environment
                                        .is_none_or(|environment| key != format!("${environment}"))
                            })
                            || (key == "headers"
                                && value.as_object().is_none_or(|headers| {
                                    headers.iter().any(|(header, value)| {
                                        value.as_str().is_none_or(|value| {
                                            !supported_header_value(
                                                header,
                                                value,
                                                api_key_environment,
                                            )
                                        })
                                    })
                                }))
                            || (key == "baseUrl"
                                && value.as_str().is_some_and(|base_url| {
                                    url::Url::parse(base_url).map_or(true, |url| {
                                        !url.username().is_empty()
                                            || url.password().is_some()
                                            || url
                                                .query_pairs()
                                                .any(|(name, _)| sensitive_query_name(&name))
                                    })
                                }))
                    } else {
                        let empty = value.as_array().is_some_and(Vec::is_empty)
                            || value.as_str().is_some_and(str::is_empty);
                        (root
                            && reject_root_model
                            && ["defaultModel", "defaultProvider", "defaultThinkingLevel"]
                                .contains(&key.as_str()))
                            || (["extensions", "packages", "skills", "skillsDir"]
                                .contains(&key.as_str())
                                && !empty)
                            || (key == "apiKey"
                                && path.file_name().is_none_or(|name| name != "models.json"))
                    };
                    if forbidden {
                        bail!(
                            "Pi {} contains unsupported or conflicting key {:?}",
                            path.display(),
                            key
                        );
                    }
                    visit(value, path, api_key_environment, reject_root_model, false)?;
                }
            }
            serde_json::Value::Array(array) => {
                for value in array {
                    visit(value, path, api_key_environment, reject_root_model, false)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    visit(value, path, api_key_environment, reject_root_model, true)
}

fn settings_semantics(bytes: &[u8], profile: &Profile) -> Result<Vec<u8>> {
    let mut value: serde_json::Value =
        serde_json::from_slice(bytes).context("Pi settings snapshot must remain valid JSON")?;
    reject_ambient_authority(&value, Path::new("settings.json"), None, false)?;
    let settings = value.as_object_mut().context("Pi settings.json must contain an object")?;
    let mut overridden_preferences = vec!["defaultProvider"];
    if profile.model.is_some() {
        overridden_preferences.push("defaultModel");
    }
    if profile.reasoning.is_some() {
        overridden_preferences.push("defaultThinkingLevel");
    }
    for preference in overridden_preferences {
        settings.remove(preference);
    }
    for empty_selection in ["packages", "extensions", "skills"] {
        if settings
            .get(empty_selection)
            .is_some_and(|value| value.as_array().is_some_and(Vec::is_empty))
        {
            settings.remove(empty_selection);
        }
    }
    Ok(serde_json::to_vec(&value)?)
}

fn valid_environment_reference(reference: &str) -> bool {
    let Some(name) = reference.strip_prefix('$') else {
        return false;
    };
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn normalize_agent_skills(
    contents: &str,
    profile: &Profile,
    skills: &[SkillIdentity],
) -> Result<String> {
    let contents = normalize_text_newlines(contents);
    let Some((frontmatter, body)) =
        contents.strip_prefix("---\n").and_then(|text| text.split_once("\n---"))
    else {
        return Ok(contents);
    };
    let mut normalized = Vec::new();
    let lines: Vec<_> = frontmatter.lines().collect();
    for (line_index, line) in lines.iter().enumerate() {
        if !line.trim_start().starts_with("skillPath:") {
            let trimmed = line.trim_start();
            if let Some((key, raw)) = trimmed.split_once(':')
                && ["extensions", "extensionPath", "extensionPaths"].contains(&key.trim())
            {
                let value = raw.trim().trim_matches(['"', '\'']);
                if !value.is_empty() && value != "[]" && value != "null" {
                    bail!(
                        "Pi role {} has non-empty {key}; only locked runtime extension entrypoints are supported",
                        profile.id
                    );
                }
                if value.is_empty() {
                    for next in lines.iter().skip(line_index + 1) {
                        if !next.starts_with(char::is_whitespace) {
                            break;
                        }
                        if next.trim_start().starts_with('-')
                            && !next.trim_start().trim_start_matches('-').trim().is_empty()
                        {
                            bail!(
                                "Pi role {} has non-empty {key}; only locked runtime extension entrypoints are supported",
                                profile.id
                            );
                        }
                    }
                }
            }
            if trimmed.starts_with("skills:") {
                let value = trimmed
                    .trim_start_matches("skills:")
                    .trim()
                    .trim_matches(['"', '\''])
                    .trim_matches(['[', ']']);
                for name in value.split(',').map(str::trim).filter(|name| !name.is_empty()) {
                    if !skills.iter().any(|skill| skill.name == name) {
                        bail!(
                            "Pi role {:?} requests undeclared Profile skill {:?}",
                            profile.id,
                            name
                        );
                    }
                }
            }
            normalized.push((*line).to_owned());
            continue;
        }
        let raw =
            line.trim_start().trim_start_matches("skillPath:").trim().trim_matches(['"', '\'']);
        if raw == "@SKILLS@" {
            normalized.push("skillPath: @BRAID_PROFILE_SKILLS_ROOT@".to_owned());
            continue;
        }
        let path = Path::new(raw).canonicalize().with_context(|| format!("Pi role skillPath must resolve to a declared Profile.skills directory or use @SKILLS@: {raw}"))?;
        let declared = profile.skills.iter().any(|skill| {
            let root = if skill.is_file() { skill.parent() } else { Some(skill.as_path()) };
            root.and_then(|root| root.canonicalize().ok()).is_some_and(|root| {
                path == root
                    || (path.parent() == Some(root.as_path()) && path.join("SKILL.md").is_file())
            })
        });
        if !declared {
            bail!(
                "Pi role skillPath {} is outside the Profile.skills material snapshot",
                path.display()
            );
        }
        normalized.push("skillPath: @BRAID_PROFILE_SKILLS_ROOT@".to_owned());
    }
    Ok(format!("---\n{}\n---{}", normalized.join("\n"), body))
}

fn normalize_text_newlines(contents: &str) -> String {
    contents.strip_prefix('\u{feff}').unwrap_or(contents).replace("\r\n", "\n").replace('\r', "\n")
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

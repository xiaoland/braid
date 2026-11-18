//! One-shot conversion of Pi-native `auth.json` credentials into Braid's
//! provider credential files. The resulting Braid-schema file is the only
//! credential input; Pi's auth.json remains an adapter-owned runtime format.
use std::{
    fmt::Write as _,
    fs::OpenOptions,
    io::Write as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use clap::Args;

use crate::config::{OAuthCredentialFile, validate_token};
use crate::home::UserHome;

#[derive(Debug, Args)]
pub(crate) struct ProviderImportAuth {
    /// LLM provider id whose credential is imported.
    #[arg(value_name = "PROVIDER_ID")]
    provider_id: String,
    /// Pi-native auth.json to convert from.
    #[arg(long, value_name = "PATH")]
    from: PathBuf,
    #[arg(long, value_name = "DIR", env = "BRAID_USER_HOME", default_value = "~/.braid")]
    user_home: PathBuf,
    /// Overwrite an existing Braid credential file.
    #[arg(long)]
    force: bool,
}

pub fn import_auth(arguments: &ProviderImportAuth) -> Result<()> {
    validate_token("provider id", &arguments.provider_id)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let user_home = UserHome::resolve(Some(&arguments.user_home))?;
    let from = arguments.from.canonicalize().context("cannot resolve Pi auth.json source")?;
    let (rendered, kind_label) = convert_credential(&from, &arguments.provider_id)?;

    let target_dir = user_home.secrets_dir();
    std::fs::create_dir_all(&target_dir)
        .with_context(|| format!("cannot create secrets directory {}", target_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = std::fs::metadata(&target_dir)?.permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&target_dir, permissions)?;
    }
    let target = target_dir.join(format!("{}.toml", arguments.provider_id));
    if target.exists() && !arguments.force {
        bail!(
            "Braid credential file {} already exists; pass --force to overwrite it",
            target.display()
        );
    }
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(&target)
        .with_context(|| format!("cannot write Braid credential file {}", target.display()))?;
    file.write_all(rendered.as_bytes())?;
    file.sync_all()?;

    println!(
        "Imported {:?} {} credential from {}\nWrote Braid credential file: {}\nReference it as: credential = {{ kind = \"{}\", file = \"{}\" }}",
        arguments.provider_id,
        kind_label,
        from.display(),
        target.display(),
        kind_label,
        target.display()
    );
    Ok(())
}

fn convert_credential(from: &Path, provider_id: &str) -> Result<(String, &'static str)> {
    let bytes = std::fs::read(from)
        .with_context(|| format!("cannot read Pi auth.json {}", from.display()))?;
    let document: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("cannot parse Pi auth.json {}", from.display()))?;
    let credentials = document
        .as_object()
        .with_context(|| format!("Pi auth.json {} must contain a provider map", from.display()))?;
    let entry = credentials.get(provider_id).ok_or_else(|| {
        let available = credentials.keys().cloned().collect::<Vec<_>>().join(", ");
        anyhow::anyhow!(
            "Pi auth.json {} has no credential for {provider_id:?} (available: {})",
            from.display(),
            if available.is_empty() { "none".to_owned() } else { available }
        )
    })?;
    let kind = entry.get("type").and_then(serde_json::Value::as_str).ok_or_else(|| {
        anyhow::anyhow!("credential {provider_id:?} in {} has no type", from.display())
    })?;

    match kind {
        "oauth" => convert_oauth(entry, from, provider_id),
        "api_key" => convert_api_key(entry, from, provider_id),
        other => {
            bail!("unsupported credential type {other:?} for {provider_id:?} in {}", from.display())
        }
    }
}

fn convert_oauth(
    entry: &serde_json::Value,
    from: &Path,
    provider_id: &str,
) -> Result<(String, &'static str)> {
    let access = required_string(entry, "access")?;
    let refresh = required_string(entry, "refresh")?;
    let expires = entry.get("expires").and_then(serde_json::Value::as_u64).ok_or_else(|| {
        anyhow::anyhow!(
            "credential {provider_id:?} in {} requires numeric expires (epoch milliseconds)",
            from.display()
        )
    })?;
    let account = entry.get("account").and_then(serde_json::Value::as_str);
    for field in entry.as_object().expect("credential object").keys() {
        if !matches!(field.as_str(), "type" | "access" | "refresh" | "expires" | "account") {
            eprintln!(
                "warning: dropping provider-specific OAuth field {field:?} from {provider_id:?}; Braid keeps only access/refresh/expires/account"
            );
        }
    }
    let oauth =
        OAuthCredentialFile { access, refresh, expires, account: account.map(str::to_owned) };
    let mut text = format!(
        "access = {:?}\nrefresh = {:?}\nexpires = {}\n",
        oauth.access, oauth.refresh, oauth.expires
    );
    if let Some(account) = &oauth.account {
        writeln!(text, "account = {account:?}").expect("write to string cannot fail");
    }
    Ok((text, "oauth"))
}

fn convert_api_key(
    entry: &serde_json::Value,
    from: &Path,
    provider_id: &str,
) -> Result<(String, &'static str)> {
    let key = entry.get("key").and_then(serde_json::Value::as_str).ok_or_else(|| {
        anyhow::anyhow!("api_key credential {provider_id:?} in {} has no key", from.display())
    })?;
    if key.trim().is_empty() {
        bail!("api_key credential {provider_id:?} in {} is empty", from.display());
    }
    Ok((format!("provider_api_key = {key:?}\n"), "api_key"))
}

fn required_string(entry: &serde_json::Value, field: &str) -> Result<String> {
    let value = entry
        .get(field)
        .and_then(serde_json::Value::as_str)
        .with_context(|| format!("OAuth credential requires string field {field:?}"))?;
    if value.trim().is_empty() {
        bail!("OAuth credential field {field:?} must not be empty");
    }
    Ok(value.to_owned())
}

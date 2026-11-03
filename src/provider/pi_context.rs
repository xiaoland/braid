use std::{collections::BTreeMap, fs, io::Write as _, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ProviderError;

/// Staging input only. Native history, rather than a second consumed flag,
/// decides whether this Context still needs to be sent.
#[derive(Serialize, Deserialize)]
pub(super) struct PendingContext {
    pub(super) native_session_id: String,
    pub(super) context: String,
}

pub(super) fn save(session: &str, pending: &PendingContext) -> Result<(), ProviderError> {
    let path = Path::new(session).with_extension("braid-context");
    let temporary = path.with_extension(format!("braid-context-{}", uuid::Uuid::now_v7()));
    let bytes = serde_json::to_vec(pending)
        .map_err(|error| ProviderError::Protocol(format!("encode Pi startup Context: {error}")))?;
    let result = (|| -> std::io::Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;
        if let Some(parent) = path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(ProviderError::Protocol(format!(
            "persist Pi startup Context {}: {error}",
            path.display()
        )));
    }
    Ok(())
}

pub(super) fn load(session: &str) -> Result<Option<PendingContext>, ProviderError> {
    let path = Path::new(session).with_extension("braid-context");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ProviderError::Protocol(format!(
                "read Pi startup Context {}: {error}",
                path.display()
            )));
        }
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        ProviderError::Protocol(format!("decode Pi startup Context {}: {error}", path.display()))
    })
}

pub(super) fn validate_history(session: &str) -> Result<(), ProviderError> {
    let history = fs::read_to_string(session).map_err(|error| {
        ProviderError::Protocol(format!("read Pi native history {session}: {error}"))
    })?;
    for (index, line) in history.lines().enumerate() {
        if !line.is_empty() {
            serde_json::from_str::<Value>(line).map_err(|error| {
                ProviderError::Protocol(format!(
                    "Pi native history {session}, line {}: {error}",
                    index + 1
                ))
            })?;
        }
    }
    Ok(())
}

/// Follow the active native branch, including ancestors before compaction.
/// A quoted Context in tool output or a different branch is not consumption.
pub(super) fn consumed(pending: &PendingContext, response: &Value) -> Result<bool, ProviderError> {
    let invalid = |reason: &str| ProviderError::Protocol(format!("Pi get_entries: {reason}"));
    let data = response.get("data").ok_or_else(|| invalid("missing data"))?;
    let entries =
        data.get("entries").and_then(Value::as_array).ok_or_else(|| invalid("missing entries"))?;
    let mut indexed = BTreeMap::new();
    for entry in entries {
        let id =
            entry.get("id").and_then(Value::as_str).ok_or_else(|| invalid("entry missing id"))?;
        if indexed.insert(id, entry).is_some() {
            return Err(invalid("duplicate entry id"));
        }
    }
    let prefix = format!("{}\n\n", pending.context);
    let mut cursor = data.get("leafId").ok_or_else(|| invalid("missing leafId"))?;
    let mut visited = 0;
    let mut found = false;
    let mut has_user_input = false;
    while !cursor.is_null() {
        let id = cursor.as_str().ok_or_else(|| invalid("invalid branch id"))?;
        let entry = indexed.get(id).ok_or_else(|| invalid("branch entry missing"))?;
        visited += 1;
        if visited > entries.len() {
            return Err(invalid("cyclic branch"));
        }
        if entry.get("type").and_then(Value::as_str) == Some("message") {
            let message = &entry["message"];
            has_user_input |= message["role"].as_str() == Some("user");
            if message["role"].as_str() == Some("user")
                && message["content"].as_array().is_some_and(|content| {
                    content.iter().any(|item| {
                        item["type"].as_str() == Some("text")
                            && item["text"].as_str().is_some_and(|text| text.starts_with(&prefix))
                    })
                })
            {
                found = true;
            }
        }
        cursor = entry.get("parentId").ok_or_else(|| invalid("entry missing parentId"))?;
    }
    if has_user_input && !found {
        return Err(invalid(
            "startup Context is absent from a populated active branch; cannot safely reinject",
        ));
    }
    Ok(found)
}

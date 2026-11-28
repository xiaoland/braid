#![allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn required_string(
    value: &Value,
    field: &str,
    context: &str,
) -> Result<String, ProviderError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ProviderError::Protocol(format!("{context} omitted {field}")))
}

pub(crate) fn path_text(path: &Path) -> Result<&str, ProviderError> {
    path.to_str().ok_or_else(|| {
        ProviderError::Protocol(format!("provider workspace is not UTF-8: {}", path.display()))
    })
}

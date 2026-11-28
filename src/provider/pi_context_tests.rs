use super::*;
use serde_json::json;

#[test]
fn persisted_startup_context_is_reopened_and_consumption_follows_the_active_native_branch() {
    let root = std::env::temp_dir().join(format!("braid-pi-context-{}", uuid::Uuid::now_v7()));
    fs::create_dir_all(&root).unwrap();
    let session = root.join("session.jsonl");
    let session = session.to_str().unwrap();
    assert!(load(session).unwrap().is_none());
    save(
        session,
        &PendingContext {
            native_session_id: "native-session".into(),
            context: "Current agreement".into(),
        },
    )
    .unwrap();
    // Read the actual file independently of the decoder, then reopen through
    // the production resume reader. No fixture pre-seeds a consumed marker.
    let stored: Value = serde_json::from_slice(
        &fs::read(Path::new(session).with_extension("braid-context")).unwrap(),
    )
    .unwrap();
    assert_eq!(stored, json!({"native_session_id":"native-session","context":"Current agreement"}));
    let pending = load(session).unwrap().unwrap();
    assert_eq!(pending.native_session_id, "native-session");
    assert_eq!(pending.context, "Current agreement");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(Path::new(session).with_extension("braid-context"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let entries = json!([
        {"id":"user","parentId":null,"type":"message","message":{"role":"user","content":[{"type":"text","text":"Current agreement\n\nEvent reference"}]}},
        {"id":"compaction","parentId":"user","type":"compaction"},
        {"id":"tool","parentId":null,"type":"message","message":{"role":"toolResult","content":[{"type":"text","text":"Current agreement\n\nquoted"}]}},
        {"id":"different","parentId":null,"type":"message","message":{"role":"user","content":[{"type":"text","text":"Different input"}]}}
    ]);
    assert!(
        consumed(&pending, &json!({"data":{"entries":entries,"leafId":"compaction"}})).unwrap()
    );
    assert!(!consumed(&pending, &json!({"data":{"entries":entries,"leafId":"tool"}})).unwrap());
    assert!(!consumed(&pending, &json!({"data":{"entries":[],"leafId":null}})).unwrap());
    let error =
        consumed(&pending, &json!({"data":{"entries":entries,"leafId":"different"}})).unwrap_err();
    assert!(
        matches!(error, ProviderError::Protocol(reason) if reason.contains("startup Context is absent from a populated active branch"))
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn incomplete_or_cyclic_native_history_cannot_prove_context_consumption() {
    let pending = PendingContext { native_session_id: "native".into(), context: "Context".into() };
    for (entries, expected) in [
        (json!([{"id":"leaf","parentId":"missing","type":"compaction"}]), "branch entry missing"),
        (json!([{"id":"leaf","parentId":"leaf","type":"compaction"}]), "cyclic branch"),
        (
            json!([{"id":"leaf","parentId":null},{"id":"leaf","parentId":null}]),
            "duplicate entry id",
        ),
    ] {
        let error =
            consumed(&pending, &json!({"data":{"entries":entries,"leafId":"leaf"}})).unwrap_err();
        assert!(
            matches!(error, ProviderError::Protocol(reason) if reason == format!("Pi get_entries: {expected}"))
        );
    }
}

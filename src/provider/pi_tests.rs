use super::*;

#[test]
fn oauth_material_is_written_in_the_native_format_with_private_permissions() {
    let root = std::env::temp_dir().join(format!("braid-pi-auth-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    let config: PiConfig =
        toml::from_str("executable = '/unused/pi'\nprovider = 'test-llm'\n").unwrap();
    for account in [None, Some("operator@example.invalid".to_owned())] {
        let oauth = crate::config::OAuthCredentialFile {
            access: "PRIVATE_ACCESS".into(),
            refresh: "PRIVATE_REFRESH".into(),
            expires: 4_102_444_800_000,
            account: account.clone(),
        };
        let auth_path = root.join("auth.json");
        std::fs::write(&auth_path, "old contents").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&auth_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        write_pi_auth_json(&root, &config, &oauth).unwrap();
        let actual: Value = serde_json::from_slice(&std::fs::read(&auth_path).unwrap()).unwrap();
        let mut expected = json!({"test-llm":{"type":"oauth","access":"PRIVATE_ACCESS","refresh":"PRIVATE_REFRESH","expires":4_102_444_800_000_u64}});
        if account.is_some() {
            expected["test-llm"]["account"] = json!("operator@example.invalid");
        }
        assert_eq!(actual, expected);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&auth_path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pi_settles_after_retry_and_keeps_terminal_hook_failure_even_when_aborted() {
    let mut last_stop = None;
    let mut last_error = None;
    let mut hook_errors = Vec::new();
    let mut event = |frame: Value| {
        parse_pi_event(&frame, "thread", "turn", &mut last_stop, &mut last_error, &mut hook_errors)
    };
    for frame in [
        json!({"type":"message_end","message":{"role":"assistant","stopReason":"error","errorMessage":"transient"}}),
        json!({"type":"agent_end","willRetry":true}),
        json!({"type":"agent_start"}),
        json!({"type":"message_end","message":{"role":"assistant","stopReason":"stop"}}),
        json!({"type":"message_end","message":{"role":"toolResult"}}),
        json!({"type":"agent_end","willRetry":false}),
    ] {
        assert!(event(frame).is_none(), "intermediate frames must leave the turn open");
    }
    assert!(matches!(event(json!({"type":"agent_settled"})),
        Some(ProviderNotification::TurnCompleted { thread_id, turn_id, status, error: None })
            if thread_id == "thread" && turn_id == "turn" && status == "completed"));

    assert!(event(json!({"type":"agent_start"})).is_none());
    assert!(matches!(event(json!({"type":"agent_settled"})),
        Some(ProviderNotification::TurnCompleted { status, error: Some(_) , .. }) if status == "failed"));
    for interrupted in [json!({"aborted":true}), json!({"cancelled":true})] {
        assert!(event(json!({"type":"agent_start"})).is_none());
        let mut frame = interrupted;
        frame["type"] = "agent_settled".into();
        assert!(
            matches!(event(frame), Some(ProviderNotification::TurnCompleted { status, error: None, .. })
            if status == "interrupted")
        );
    }
    assert!(event(json!({"type":"agent_start"})).is_none());
    assert!(event(json!({
        "type":"extension_error","event":"agent_before_settle","extensionPath":"background.ts","error":"cannot preserve result"
    })).is_none());
    assert!(matches!(event(json!({"type":"agent_settled","aborted":true})),
        Some(ProviderNotification::TurnCompleted { status, error: Some(reason), .. })
            if status == "failed" && reason.contains("background.ts") && reason.contains("cannot preserve result")));
}

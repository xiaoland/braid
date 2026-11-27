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

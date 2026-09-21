use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_manager(dir: &std::path::Path) -> OAuthManager {
        OAuthManager {
            client: reqwest::Client::new(),
            config: AuthConfig::default(),
            credentials_path: dir.join("auth.json"),
        }
    }

    fn make_credentials(hours_from_now: i64) -> OAuthCredentials {
        OAuthCredentials {
            access_token: "test-access-token".to_string(),
            refresh_token: Some("test-refresh-token".to_string()),
            expires_at: Utc::now() + chrono::Duration::hours(hours_from_now),
            token_type: "Bearer".to_string(),
        }
    }

    #[tokio::test]
    async fn test_save_and_load_credentials() {
        let tmp = TempDir::new().unwrap();
        let manager = test_manager(tmp.path());
        let creds = make_credentials(1);

        manager.save_credentials(&creds).unwrap();
        let loaded = manager.load_credentials().unwrap();

        assert_eq!(loaded.access_token, "test-access-token");
        assert_eq!(loaded.refresh_token, Some("test-refresh-token".to_string()));
        assert_eq!(loaded.token_type, "Bearer");
        // Allow 1 second tolerance for serialization round-trip
        let diff = (loaded.expires_at - creds.expires_at).num_seconds().abs();
        assert!(diff <= 1, "expires_at mismatch: diff={diff}s");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_save_credentials_restricts_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let manager = test_manager(tmp.path());
        let creds = make_credentials(1);

        manager.save_credentials(&creds).unwrap();

        let mode = std::fs::metadata(&manager.credentials_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "credentials file must be owner-only readable/writable"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_save_credentials_tightens_preexisting_loose_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let manager = test_manager(tmp.path());

        // Simulate a credentials file written by an older build before this
        // hardening existed: it already sits on disk, world-readable.
        std::fs::write(&manager.credentials_path, "{}").unwrap();
        std::fs::set_permissions(&manager.credentials_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let creds = make_credentials(1);
        manager.save_credentials(&creds).unwrap();

        let mode = std::fs::metadata(&manager.credentials_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "an update to a pre-existing, over-permissive credentials file must still end up owner-only"
        );
    }

    #[tokio::test]
    async fn test_has_credentials_false_when_empty() {
        let tmp = TempDir::new().unwrap();
        let manager = test_manager(tmp.path());

        assert!(!manager.has_credentials());
    }

    #[tokio::test]
    async fn test_logout_deletes_credentials() {
        let tmp = TempDir::new().unwrap();
        let manager = test_manager(tmp.path());
        let creds = make_credentials(1);

        manager.save_credentials(&creds).unwrap();
        assert!(manager.has_credentials());

        manager.logout().unwrap();
        assert!(!manager.has_credentials());
        assert!(!manager.credentials_path.exists());
    }

    #[tokio::test]
    async fn test_get_token_returns_valid_token() {
        let tmp = TempDir::new().unwrap();
        let manager = test_manager(tmp.path());
        let creds = make_credentials(1);

        manager.save_credentials(&creds).unwrap();

        let token = manager.get_token().await.unwrap();
        assert_eq!(token, "test-access-token");
    }

    #[tokio::test]
    async fn test_get_token_refreshes_expired() {
        let tmp = TempDir::new().unwrap();
        let mock_server = MockServer::start().await;

        let manager = OAuthManager {
            client: reqwest::Client::new(),
            config: AuthConfig {
                auth_url: mock_server.uri(),
                token_url: format!("{}/token", mock_server.uri()),
                client_id: "test".to_string(),
            },
            credentials_path: tmp.path().join("auth.json"),
        };

        // Save expired credentials
        let expired_creds = make_credentials(-1);
        manager.save_credentials(&expired_creds).unwrap();

        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "new-token",
                "refresh_token": "new-refresh",
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .mount(&mock_server)
            .await;

        let token = manager.get_token().await.unwrap();
        assert_eq!(token, "new-token");

        // Verify new credentials were persisted
        let reloaded = manager.load_credentials().unwrap();
        assert_eq!(reloaded.access_token, "new-token");
        assert_eq!(reloaded.refresh_token, Some("new-refresh".to_string()));
    }
}

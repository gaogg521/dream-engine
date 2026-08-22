use dream_engine_agent::session::SessionManager;
use dream_engine_config::config::SessionConfig;
use tempfile::tempdir;

use super::session_catalog;

#[test]
fn session_catalog_does_not_truncate_entries_to_ten() {
    let directory = tempdir().expect("temporary session directory should be created");
    let manager = SessionManager::new(directory.path().to_path_buf(), 20);
    for index in 0..15 {
        manager
            .create(
                "openai",
                "test-model",
                "/workspace",
                Some(&format!("session-{index:02}")),
            )
            .expect("test session should be created");
    }
    let config = SessionConfig {
        enabled: true,
        directory: directory.path().to_string_lossy().into_owned(),
        max_sessions: 20,
    };

    let sessions = session_catalog(&config, None).expect("session catalog should load");

    assert_eq!(sessions.len(), 15);
}

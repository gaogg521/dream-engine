use super::*;

#[cfg(unix)]
#[test]
fn restrict_to_owner_sets_mode_0600() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("secret.json");
    std::fs::write(&path, "{}").unwrap();

    // Start from an intentionally over-permissive mode, as a pre-existing
    // file written before this hardening existed would have.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    restrict_to_owner(&path).unwrap();

    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[cfg(unix)]
#[test]
fn restrict_to_owner_tightens_preexisting_loose_permissions() {
    use std::os::unix::fs::PermissionsExt;

    // Simulates an old file written before this fix shipped: it already
    // exists with group/world-readable permissions, and a later write to
    // it (e.g. a token refresh) must still end up owner-only.
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("old-creds.json");
    std::fs::write(&path, "stale").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();

    std::fs::write(&path, "updated").unwrap();
    restrict_to_owner(&path).unwrap();

    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn restrict_to_owner_ok_on_existing_file_any_platform() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("secret.json");
    std::fs::write(&path, "{}").unwrap();

    assert!(restrict_to_owner(&path).is_ok());
}

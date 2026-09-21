// Centralized file-permission hardening for locally persisted secrets
// (provider API keys, OAuth access/refresh tokens).
//
// Call this after every write to a file that may hold such secrets —
// including a write that only updates an existing file — so that a file
// left over-permissive by a previous version (or created before this
// hardening existed) gets its permissions tightened the next time it is
// touched, not only when it is freshly created.

use std::path::Path;

/// Restrict a file to owner-only read/write.
///
/// - Unix: sets mode `0o600` via [`std::os::unix::fs::PermissionsExt`].
/// - Non-Unix (Windows): no-op. NTFS already scopes a user's profile
///   directory (where `%APPDATA%` lives) to that user by default, so the
///   same cross-machine "other local accounts can read it" risk this
///   targets does not apply the same way; adding ACL-level hardening here
///   was judged not worth the complexity for this fix. See AGENTS.md's
///   Cross-Platform section for the project's general stance on platform
///   differences.
pub fn restrict_to_owner(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let metadata = std::fs::metadata(path)?;
        let mut perms = metadata.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
#[path = "fs_perms_test.rs"]
mod fs_perms_test;

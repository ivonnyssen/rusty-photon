use std::path::Path;

use tracing::debug;

use crate::error::Result;

/// Set file permissions to owner-only read/write (0600).
///
/// On Unix systems, this restricts access to the file owner.
/// On non-Unix systems, this is a no-op with a debug log.
pub fn set_restricted_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)?;
        debug!("Set permissions 0600 on {}", path.display());
    }
    #[cfg(not(unix))]
    {
        debug!(
            "Skipping permission restriction on non-Unix platform: {}",
            path.display()
        );
    }
    Ok(())
}

/// Create `path` (truncating any previous content), restrict it while it
/// is still empty, then write `contents` — secret bytes never exist on
/// disk under the umask-default mode.
pub fn write_restricted(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    set_restricted_permissions(path)?;
    file.write_all(contents)?;
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn write_restricted_writes_the_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.key");
        write_restricted(&path, b"KEY-BYTES").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"KEY-BYTES");
        #[cfg(unix)]
        {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "mode {mode:o}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn set_restricted_permissions_sets_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("secret.key");
        std::fs::write(&file_path, "secret").unwrap();

        set_restricted_permissions(&file_path).unwrap();

        let meta = std::fs::metadata(&file_path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600 but got {mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn set_restricted_permissions_on_nonexistent_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("does-not-exist");
        let result = set_restricted_permissions(&file_path);
        assert!(result.is_err());
    }
}

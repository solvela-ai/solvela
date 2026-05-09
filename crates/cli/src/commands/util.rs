//! Shared CLI helpers reused across multiple subcommands.
//!
//! This module exists so commands like `wallet init` and `mcp install` can
//! share a single atomic-write implementation that gets the file-permission
//! bits right (chmod the tempfile FD *before* the rename — never leave a
//! key-bearing file readable on disk between syscalls).

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

/// Atomically write `value` as pretty JSON to `path`:
/// 1. Write to a temp file in the same directory (same filesystem → rename is atomic).
/// 2. Chmod 0600 on the tempfile FD before the rename so the file is never
///    visible on disk with wider permissions.
/// 3. Rename temp file over the target path.
/// 4. Re-apply 0600 after rename in case of cross-filesystem fallback (belt-and-braces).
pub(crate) fn write_atomic_json(path: &Path, value: &Value) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("no parent directory for {}", path.display()))?;

    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create directory {}", parent.display()))?;

    let json = serde_json::to_string_pretty(value).context("failed to serialize JSON")?;

    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .context("failed to create temp file for atomic write")?;
    tmp.write_all(json.as_bytes())
        .context("failed to write to temp file")?;
    tmp.write_all(b"\n")
        .context("failed to write newline to temp file")?;
    tmp.flush().context("failed to flush temp file")?;

    // Set 0600 on the tempfile FD BEFORE the rename — closes the race window
    // where `fs::write` followed by `fs::set_permissions` would briefly leave
    // a key-bearing file world-readable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .context("failed to set permissions on temp file")?;
    }

    tmp.persist(path)
        .with_context(|| format!("failed to persist temp file to {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to set permissions on {}", path.display()))?;
    }

    Ok(())
}

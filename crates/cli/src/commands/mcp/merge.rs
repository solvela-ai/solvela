use std::fs;
use std::io::Write as IoWrite;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Map, Value};

/// Merge `entry` into the `mcpServers.solvela` key of the JSON file at `path`.
///
/// Rules:
/// - If the file doesn't exist: write `{ "mcpServers": { "solvela": entry } }`.
/// - If the file exists but has no `mcpServers.solvela` key: inject and write.
/// - If the file exists and `mcpServers.solvela` equals `entry` already: no-op (returns `false`).
/// - If the file exists and `mcpServers.solvela` differs: return `Err` unless `force` is true.
///
/// Returns `true` when the file was written, `false` when it was already up to date.
pub fn merge_entry(path: &Path, entry: &Value, force: bool) -> Result<bool> {
    let mut root: Value = if path.exists() {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("invalid JSON in {}", path.display()))?
    } else {
        Value::Object(Map::new())
    };

    // Ensure mcpServers key exists.
    if root.get("mcpServers").is_none() {
        root.as_object_mut()
            .context("config root is not a JSON object")?
            .insert("mcpServers".to_owned(), Value::Object(Map::new()));
    }

    let mcp_servers = root["mcpServers"]
        .as_object_mut()
        .context("mcpServers is not a JSON object")?;

    if let Some(existing) = mcp_servers.get("solvela") {
        if existing == entry {
            tracing::info!("no change to {}", path.display());
            return Ok(false);
        }
        if !force {
            anyhow::bail!(
                "A Solvela MCP entry already exists at {}.\n\
                 Use --force to overwrite, or --diff to see what would change.",
                path.display()
            );
        }
        tracing::info!(
            "overwriting existing solvela entry (--force) at {}",
            path.display()
        );
    }

    mcp_servers.insert("solvela".to_owned(), entry.clone());

    write_atomic(path, &root)?;
    Ok(true)
}

/// Remove the `mcpServers.solvela` key from the JSON file at `path`.
///
/// Returns `true` when the file was modified, `false` when `mcpServers` is absent
/// (nothing to do). Returns `Err` when `mcpServers` is present but is not a JSON
/// object (malformed config — mirrors install's error behavior for consistency).
pub fn remove_entry(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut root: Value = serde_json::from_str(&raw)
        .with_context(|| format!("invalid JSON in {}", path.display()))?;

    match root.get_mut("mcpServers") {
        None => return Ok(false),
        Some(v) => match v.as_object_mut() {
            Some(mcp_servers) => {
                if mcp_servers.remove("solvela").is_none() {
                    return Ok(false);
                }
            }
            None => {
                anyhow::bail!(
                    "mcpServers is not a JSON object in {}; cannot uninstall safely",
                    path.display()
                );
            }
        },
    }

    write_atomic(path, &root)?;
    Ok(true)
}

/// Read the current `mcpServers.solvela` entry from a config file, if present.
pub fn read_entry(path: &Path) -> Result<Option<Value>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let root: Value = serde_json::from_str(&raw)
        .with_context(|| format!("invalid JSON in {}", path.display()))?;
    Ok(root
        .get("mcpServers")
        .and_then(|v| v.get("solvela"))
        .cloned())
}

/// Atomically write `value` as pretty JSON to `path`:
/// 1. Write to a temp file in the same directory (same filesystem → rename is atomic).
/// 2. Rename temp file over the target path.
/// 3. On Unix, chmod 0600.
pub fn write_atomic(path: &Path, value: &Value) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("no parent directory for {}", path.display()))?;

    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create directory {}", parent.display()))?;

    let json = serde_json::to_string_pretty(value).context("failed to serialize JSON")?;

    // Write to temp file in same directory for atomic rename.
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .context("failed to create temp file for atomic write")?;
    tmp.write_all(json.as_bytes())
        .context("failed to write to temp file")?;
    tmp.write_all(b"\n")
        .context("failed to write newline to temp file")?;
    tmp.flush().context("failed to flush temp file")?;

    // Set 0600 permissions on the tempfile FD BEFORE the atomic rename so the
    // file is never visible on disk with wider permissions (no race window).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .context("failed to set permissions on temp file")?;
    }

    tmp.persist(path)
        .with_context(|| format!("failed to persist temp file to {}", path.display()))?;

    // Belt-and-braces: re-apply after rename in case of cross-filesystem fallback.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to set permissions on {}", path.display()))?;
    }

    tracing::info!("wrote config to {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn entry() -> Value {
        json!({
            "command": "npx",
            "args": ["-y", "@solvela/mcp-server"],
            "env": { "SOLVELA_API_URL": "https://api.solvela.ai" }
        })
    }

    fn other_entry() -> Value {
        json!({
            "command": "node",
            "args": ["other"],
            "env": {}
        })
    }

    #[test]
    fn test_merge_creates_file_when_missing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        let wrote = merge_entry(&path, &entry(), false).unwrap();
        assert!(wrote, "should have written");
        assert!(path.exists());

        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["solvela"], entry());
    }

    #[test]
    fn test_merge_preserves_other_mcp_servers() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        // Pre-populate with another server entry
        let existing = json!({
            "mcpServers": {
                "other-server": { "command": "node", "args": [] }
            }
        });
        fs::write(&path, serde_json::to_string_pretty(&existing).unwrap()).unwrap();

        merge_entry(&path, &entry(), false).unwrap();

        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["solvela"], entry(), "solvela entry written");
        assert!(
            v["mcpServers"]["other-server"].is_object(),
            "other-server preserved"
        );
    }

    #[test]
    fn test_merge_no_op_on_identical_entry() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        merge_entry(&path, &entry(), false).unwrap();
        let wrote = merge_entry(&path, &entry(), false).unwrap();
        assert!(!wrote, "identical entry should be a no-op");
    }

    #[test]
    fn test_merge_errors_on_different_entry_without_force() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        merge_entry(&path, &entry(), false).unwrap();
        let result = merge_entry(&path, &other_entry(), false);
        assert!(result.is_err(), "should error on conflict without --force");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("--force"), "error should mention --force");
    }

    #[test]
    fn test_merge_overwrites_with_force() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        merge_entry(&path, &entry(), false).unwrap();
        let wrote = merge_entry(&path, &other_entry(), true).unwrap();
        assert!(wrote);

        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["solvela"], other_entry());
    }

    #[test]
    fn test_remove_entry_removes_solvela() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        merge_entry(&path, &entry(), false).unwrap();
        let removed = remove_entry(&path).unwrap();
        assert!(removed);

        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            v["mcpServers"]["solvela"].is_null(),
            "solvela key should be removed"
        );
    }

    #[test]
    fn test_remove_entry_preserves_other_servers() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        let existing = json!({
            "mcpServers": {
                "other-server": { "command": "node", "args": [] },
                "solvela": entry()
            }
        });
        fs::write(&path, serde_json::to_string_pretty(&existing).unwrap()).unwrap();

        remove_entry(&path).unwrap();

        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v["mcpServers"]["solvela"].is_null(), "solvela removed");
        assert!(
            v["mcpServers"]["other-server"].is_object(),
            "other-server preserved"
        );
    }

    #[test]
    fn test_remove_entry_no_op_when_absent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        let existing = json!({ "mcpServers": {} });
        fs::write(&path, serde_json::to_string_pretty(&existing).unwrap()).unwrap();

        let removed = remove_entry(&path).unwrap();
        assert!(!removed);
    }

    #[test]
    fn test_remove_entry_no_op_when_file_missing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.json");
        let removed = remove_entry(&path).unwrap();
        assert!(!removed);
    }

    #[test]
    fn test_remove_entry_errors_on_malformed_mcp_servers() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        // mcpServers is present but not an object (malformed)
        let malformed = json!({ "mcpServers": "not-an-object" });
        fs::write(&path, serde_json::to_string_pretty(&malformed).unwrap()).unwrap();

        let result = remove_entry(&path);
        assert!(result.is_err(), "should error on malformed mcpServers");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("mcpServers is not a JSON object"),
            "error should describe the problem, got: {msg}"
        );
    }

    #[test]
    fn test_remove_entry_no_op_when_mcp_servers_absent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        // File exists but has no mcpServers key at all
        let existing = json!({ "other": "data" });
        fs::write(&path, serde_json::to_string_pretty(&existing).unwrap()).unwrap();

        let removed = remove_entry(&path).unwrap();
        assert!(!removed, "should be a no-op when mcpServers is absent");
    }

    #[test]
    fn test_write_atomic_produces_valid_json() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("out.json");
        let val = json!({ "hello": "world" });
        write_atomic(&path, &val).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["hello"].as_str().unwrap(), "world");
    }

    #[test]
    fn test_uninstall_restores_pre_install_state() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        let pre_install = json!({
            "mcpServers": {
                "other-server": { "command": "node", "args": [] }
            }
        });
        fs::write(&path, serde_json::to_string_pretty(&pre_install).unwrap()).unwrap();

        // Install
        merge_entry(&path, &entry(), false).unwrap();

        // Uninstall
        remove_entry(&path).unwrap();

        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v["mcpServers"]["solvela"].is_null(), "solvela removed");
        assert!(
            v["mcpServers"]["other-server"].is_object(),
            "other-server still there"
        );
    }

    #[test]
    fn test_idempotency() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        merge_entry(&path, &entry(), false).unwrap();
        let first = fs::read_to_string(&path).unwrap();

        // Second install with same entry — no-op
        merge_entry(&path, &entry(), false).unwrap();
        let second = fs::read_to_string(&path).unwrap();

        assert_eq!(
            first, second,
            "file should be identical after idempotent install"
        );
    }
}

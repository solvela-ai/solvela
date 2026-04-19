use std::io::ErrorKind;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;

const OPENCLAW_INSTALL_HINT: &str = "openclaw CLI not found on PATH.\n\
     Install it with: npm install -g @openclaw/cli\n\
     Then re-run: solvela mcp install --host=openclaw";

/// Install (or update) the Solvela MCP entry in OpenClaw by shelling out to the
/// `openclaw mcp set solvela '<json>'` command.
///
/// This is the canonical write path per OpenClaw docs; we do NOT edit
/// `~/.openclaw/openclaw.json` directly.
pub fn openclaw_set(entry: &Value) -> Result<()> {
    let json = serde_json::to_string(entry).context("failed to serialize openclaw entry")?;

    tracing::info!("running: openclaw mcp set solvela ...");

    let output = Command::new("openclaw")
        .args(["mcp", "set", "solvela", &json])
        .output()
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                anyhow::anyhow!("{}", OPENCLAW_INSTALL_HINT)
            } else {
                anyhow::anyhow!("failed to run openclaw: {}", e)
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "openclaw mcp set failed (exit {}):\n{}",
            output.status,
            stderr.trim()
        );
    }

    Ok(())
}

/// Uninstall the Solvela MCP entry from OpenClaw by shelling out to
/// `openclaw mcp unset solvela`.
pub fn openclaw_unset() -> Result<()> {
    tracing::info!("running: openclaw mcp unset solvela");

    let output = Command::new("openclaw")
        .args(["mcp", "unset", "solvela"])
        .output()
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                anyhow::anyhow!("{}", OPENCLAW_INSTALL_HINT)
            } else {
                anyhow::anyhow!("failed to run openclaw: {}", e)
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "openclaw mcp unset failed (exit {}):\n{}",
            output.status,
            stderr.trim()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    /// Create a fake `openclaw` script in a tempdir that exits with the given code.
    fn fake_openclaw_dir(exit_code: i32) -> TempDir {
        let tmp = TempDir::new().expect("tempdir");
        let bin = tmp.path().join("openclaw");
        let script = format!("#!/bin/sh\nexit {exit_code}\n");
        fs::write(&bin, script).expect("write script");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod");
        tmp
    }

    fn prepend_path(dir: &TempDir) -> String {
        let new_path = dir.path().to_str().unwrap().to_owned();
        let old = env::var("PATH").unwrap_or_default();
        env::set_var("PATH", format!("{new_path}:{old}"));
        old
    }

    fn restore_path(old: &str) {
        env::set_var("PATH", old);
    }

    #[tokio::test]
    async fn test_openclaw_set_not_on_path() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let tmp = TempDir::new().unwrap();
        let old = env::var("PATH").unwrap_or_default();
        env::set_var("PATH", tmp.path().to_str().unwrap());

        let entry = json!({ "command": "npx" });
        let result = openclaw_set(&entry);

        restore_path(&old);
        drop(tmp);

        assert!(result.is_err(), "should error when openclaw not on PATH");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("openclaw CLI not found"),
            "error should mention install instructions, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_openclaw_set_success() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let tmp = fake_openclaw_dir(0);
        let old = prepend_path(&tmp);

        let entry = json!({ "command": "npx", "args": ["-y", "@solvela/mcp-server"] });
        let result = openclaw_set(&entry);

        restore_path(&old);
        assert!(result.is_ok(), "should succeed with exit 0: {:?}", result);
    }

    #[tokio::test]
    async fn test_openclaw_set_non_zero_exit() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let tmp = fake_openclaw_dir(1);
        let old = prepend_path(&tmp);

        let entry = json!({ "command": "npx" });
        let result = openclaw_set(&entry);

        restore_path(&old);
        assert!(result.is_err(), "should error on non-zero exit");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("openclaw mcp set failed"), "got: {msg}");
    }

    #[tokio::test]
    async fn test_openclaw_unset_not_on_path() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let tmp = TempDir::new().unwrap();
        let old = env::var("PATH").unwrap_or_default();
        env::set_var("PATH", tmp.path().to_str().unwrap());

        let result = openclaw_unset();
        restore_path(&old);
        drop(tmp);

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("openclaw CLI not found"), "got: {msg}");
    }

    #[tokio::test]
    async fn test_openclaw_unset_success() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let tmp = fake_openclaw_dir(0);
        let old = prepend_path(&tmp);

        let result = openclaw_unset();

        restore_path(&old);
        assert!(result.is_ok(), "should succeed: {:?}", result);
    }
}

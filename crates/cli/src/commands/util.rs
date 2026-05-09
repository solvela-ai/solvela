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

/// Validate a gateway URL: must parse as `http://...` or `https://...`.
///
/// Used by clap as `value_parser` on every CLI argument that accepts a
/// gateway URL — both the global `--api-url` (which is the trust anchor
/// for the entire payment flow) and the `mcp install --gateway-url`
/// sub-argument. Without this, a user (or a poisoned `SOLVELA_API_URL`
/// env var) could point the CLI at `file:///etc/passwd` or any other
/// scheme reqwest happens to support.
pub(crate) fn validate_gateway_url(s: &str) -> Result<String, String> {
    let parsed = url::Url::parse(s).map_err(|e| format!("invalid URL '{}': {}", s, e))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!(
            "gateway URL must use http or https scheme, got '{}'",
            parsed.scheme()
        ));
    }
    Ok(s.to_owned())
}

/// Hard upper bound on how many Solana slots into the future an escrow's
/// expiry may be set, as seen from the CLI's perspective. Solana slots are
/// ~400 ms each, so 10_000 slots ≈ 66 minutes — comfortably above any
/// legitimate `max_timeout_seconds` the gateway should advertise (the
/// gateway-side ceiling is 300 s ≈ 750 slots) while still rejecting
/// malicious or misconfigured responses that would push the expiry far
/// enough out to be effectively never.
const MAX_ESCROW_EXPIRY_SLOTS_AHEAD: u64 = 10_000;

/// Compute the Solana slot at which an escrow PDA created now should expire.
///
/// The intermediate `(seconds * 1000) / 400` arithmetic operates on `u64`
/// and a hostile or buggy gateway returning a `max_timeout_seconds` near
/// `u64::MAX / 1000` would silently wrap in release mode, producing a tiny
/// or zero `timeout_slots` and a near-`current_slot` `expiry_slot`. That
/// makes the deposited PDA refundable almost immediately, opening a
/// reverse-the-payment race against the gateway's claim.
///
/// Saturating arithmetic plus the `MAX_ESCROW_EXPIRY_SLOTS_AHEAD` cap
/// closes both the wrap-to-zero case and the unbounded-future case.
pub(crate) fn escrow_expiry_slot(current_slot: u64, max_timeout_seconds: u64) -> u64 {
    let timeout_slots = max_timeout_seconds
        .saturating_mul(1000)
        .saturating_div(400)
        .min(MAX_ESCROW_EXPIRY_SLOTS_AHEAD);
    current_slot.saturating_add(timeout_slots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escrow_expiry_typical_300s() {
        // 300 s × 2.5 slots/s = 750 slots ahead.
        assert_eq!(escrow_expiry_slot(1_000_000, 300), 1_000_750);
    }

    #[test]
    fn escrow_expiry_clamps_huge_values() {
        // u64::MAX seconds would wrap without saturation; with the cap it
        // tops out at MAX_ESCROW_EXPIRY_SLOTS_AHEAD slots ahead.
        assert_eq!(
            escrow_expiry_slot(1_000_000, u64::MAX),
            1_000_000 + MAX_ESCROW_EXPIRY_SLOTS_AHEAD,
        );
    }

    #[test]
    fn escrow_expiry_zero_seconds() {
        assert_eq!(escrow_expiry_slot(1_000_000, 0), 1_000_000);
    }

    #[test]
    fn escrow_expiry_saturates_current_slot_overflow() {
        // Even with a near-u64::MAX current_slot, saturating_add prevents
        // wrap-to-zero (which would also produce an instantly-refundable PDA).
        assert_eq!(escrow_expiry_slot(u64::MAX - 100, 300), u64::MAX);
    }
}

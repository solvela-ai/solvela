//! Shared CLI argument structs and wallet loading for `SolvelaClient` binaries.
//!
//! Provides reusable [`clap::Args`] groups (`WalletArgs`, `GatewayArgs`, `RpcArgs`)
//! and helper functions for wallet file I/O so that the proxy and CLI binaries
//! share identical wallet-loading logic.

use std::path::PathBuf;

use clap::Args;

use solvela_client::Wallet;

/// CLI arguments for wallet configuration.
#[derive(Debug, Clone, Args)]
pub struct WalletArgs {
    /// Environment variable containing a base58-encoded keypair.
    #[arg(long, default_value = "SOLVELA_WALLET_KEY")]
    pub wallet_env: String,

    /// Path to wallet keypair file (Solana CLI JSON byte-array format).
    #[arg(long, default_value = "~/.solvela/wallet.json")]
    pub wallet_file: String,
}

/// CLI arguments for gateway connection.
#[derive(Debug, Clone, Args)]
pub struct GatewayArgs {
    /// Gateway URL to forward requests to.
    #[arg(short = 'g', long, default_value = "https://api.solvela.ai")]
    pub gateway: String,
}

/// CLI arguments for Solana RPC connection.
#[derive(Debug, Clone, Args)]
pub struct RpcArgs {
    /// Solana RPC URL.
    #[arg(long, default_value = "https://api.mainnet-beta.solana.com")]
    pub rpc_url: String,
}

/// Expand a leading `~` in a path to the user's home directory.
///
/// If the path does not start with `~/`, it is returned as-is.
#[must_use]
pub fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        match dirs::home_dir() {
            Some(home) => home.join(rest),
            None => PathBuf::from(path),
        }
    } else {
        PathBuf::from(path)
    }
}

/// Load a wallet from an environment variable (priority) or file (fallback).
///
/// The env var is expected to contain a base58-encoded 64-byte keypair.
/// The file is expected to be in Solana CLI JSON byte-array format (`[174, 47, ...]`).
///
/// # Errors
///
/// Returns a human-readable error string if neither source yields a valid wallet.
pub fn load_wallet(args: &WalletArgs) -> Result<Wallet, String> {
    // Try env var first
    if let Ok(val) = std::env::var(&args.wallet_env) {
        if !val.is_empty() {
            return Wallet::from_keypair_b58(&val)
                .map_err(|e| format!("invalid keypair in {}: {e}", args.wallet_env));
        }
    }

    // Expand ~ to home directory
    let expanded = expand_home(&args.wallet_file);

    // Try wallet file
    if expanded.exists() {
        // Warn if wallet file has insecure permissions (private key exposure risk)
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(meta) = expanded.metadata() {
                if meta.mode() & 0o077 != 0 {
                    tracing::warn!(
                        path = %expanded.display(),
                        "wallet file has insecure permissions (should be 0600)"
                    );
                }
            }
        }

        let contents = std::fs::read_to_string(&expanded)
            .map_err(|e| format!("failed to read {}: {e}", expanded.display()))?;

        // Parse Solana CLI format: JSON array of u8 values [174, 47, ...]
        let bytes: Vec<u8> = serde_json::from_str(&contents)
            .map_err(|e| format!("invalid wallet file format in {}: {e}", expanded.display()))?;

        return Wallet::from_keypair_bytes(&bytes)
            .map_err(|e| format!("invalid keypair in {}: {e}", expanded.display()));
    }

    Err(format!(
        "no wallet found: set {} env var or create {}",
        args.wallet_env,
        expanded.display()
    ))
}

/// Save raw keypair bytes to a file in Solana CLI JSON byte-array format.
///
/// The file is written with `0o600` permissions (owner read/write only).
/// Refuses to overwrite an existing file unless `force` is `true`.
///
/// # Errors
///
/// Returns a human-readable error string if the file cannot be written or
/// already exists without `force`.
pub fn save_wallet(path: &str, keypair_bytes: &[u8], force: bool) -> Result<PathBuf, String> {
    let expanded = expand_home(path);

    if expanded.exists() && !force {
        return Err(format!(
            "wallet file already exists at {} (use --force to overwrite)",
            expanded.display()
        ));
    }

    // Fail closed on platforms where we cannot enforce owner-only ACLs at
    // file-create time. The previous behavior (write + tracing::warn!) left
    // the keypair on disk with whatever default ACL the platform applied —
    // potentially world-readable on shared hosts. See issue #322 problem 2.
    #[cfg(not(unix))]
    {
        let _ = keypair_bytes;
        return Err(format!(
            "refusing to write wallet file at {}: owner-only ACL enforcement \
             is not implemented on this platform. Set the SOLVELA_WALLET_KEY \
             env var with a base58-encoded keypair instead, or run on a Unix host.",
            expanded.display()
        ));
    }

    // Ensure parent directory exists
    #[cfg(unix)]
    if let Some(parent) = expanded.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;
    }

    // Serialize as JSON byte array (Solana CLI format) and write with
    // owner-only permissions. Reached only on Unix — the non-Unix branch
    // above returned Err.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let json = serde_json::to_string(&keypair_bytes)
            .map_err(|e| format!("failed to serialize keypair: {e}"))?;
        // Restrict permissions at create time so there's no window in which
        // the file is world-readable before chmod.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&expanded)
            .map_err(|e| format!("failed to create {}: {e}", expanded.display()))?;
        file.write_all(json.as_bytes())
            .map_err(|e| format!("failed to write {}: {e}", expanded.display()))?;
        Ok(expanded)
    }
}

#[cfg(test)]
mod tests {
    use solana_sdk::signer::Signer;

    use super::*;

    #[test]
    fn test_expand_home_with_tilde() {
        let result = expand_home("~/some/path");
        // Should NOT start with ~ (it should be expanded)
        assert!(
            !result.to_string_lossy().starts_with('~'),
            "path was not expanded: {result:?}"
        );
        assert!(
            result.to_string_lossy().ends_with("some/path"),
            "path suffix missing: {result:?}"
        );
    }

    #[test]
    fn test_expand_home_without_tilde() {
        let result = expand_home("/absolute/path");
        assert_eq!(result, PathBuf::from("/absolute/path"));

        let relative = expand_home("relative/path");
        assert_eq!(relative, PathBuf::from("relative/path"));
    }

    #[test]
    fn test_load_wallet_from_env() {
        // Generate a fresh keypair and encode as base58
        let kp = solana_sdk::signer::keypair::Keypair::new();
        let b58 = bs58::encode(kp.to_bytes()).into_string();
        let expected_addr = kp.pubkey().to_string();

        // Use a unique env var name to avoid conflicts with parallel tests
        let env_var = "SOLVELA_TEST_WALLET_LOAD_ENV_7291";
        std::env::set_var(env_var, &b58);

        let args = WalletArgs {
            wallet_env: env_var.to_string(),
            wallet_file: "/nonexistent/path.json".to_string(),
        };

        let wallet = load_wallet(&args).expect("should load from env");
        assert_eq!(wallet.address(), expected_addr);

        // Clean up
        std::env::remove_var(env_var);
    }

    #[test]
    fn test_load_wallet_no_source() {
        // Ensure env var is not set
        let env_var = "SOLVELA_TEST_WALLET_NOSOURCE_4821";
        std::env::remove_var(env_var);

        let args = WalletArgs {
            wallet_env: env_var.to_string(),
            wallet_file: "/nonexistent/wallet_file_that_does_not_exist.json".to_string(),
        };

        let result = load_wallet(&args);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("no wallet found"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn test_save_wallet_creates_file() {
        let dir = std::env::temp_dir().join("solvela_test_save_wallet");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let kp = solana_sdk::signer::keypair::Keypair::new();
        let bytes = kp.to_bytes();
        let file_path = dir.join("test_wallet.json");
        let path_str = file_path.to_string_lossy().to_string();

        let result = save_wallet(&path_str, &bytes, false);
        assert!(result.is_ok(), "save_wallet failed: {result:?}");

        let saved_path = result.unwrap();
        assert!(saved_path.exists());

        // Verify the saved content is valid Solana CLI format
        let contents = std::fs::read_to_string(&saved_path).unwrap();
        let parsed: Vec<u8> = serde_json::from_str(&contents).unwrap();
        assert_eq!(parsed.len(), 64);
        assert_eq!(&parsed[..], &bytes[..]);

        // Verify permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = saved_path.metadata().unwrap();
            assert_eq!(meta.mode() & 0o777, 0o600, "permissions should be 0600");
        }

        // Clean up
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_wallet_refuses_overwrite() {
        let dir = std::env::temp_dir().join("solvela_test_save_overwrite");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let kp = solana_sdk::signer::keypair::Keypair::new();
        let bytes = kp.to_bytes();
        let file_path = dir.join("existing_wallet.json");
        let path_str = file_path.to_string_lossy().to_string();

        // Create the file first
        save_wallet(&path_str, &bytes, false).unwrap();

        // Attempt to overwrite without force — should fail
        let result = save_wallet(&path_str, &bytes, false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("already exists"), "unexpected error: {err}");

        // With force — should succeed
        let result = save_wallet(&path_str, &bytes, true);
        assert!(result.is_ok(), "force overwrite failed: {result:?}");

        // Clean up
        let _ = std::fs::remove_dir_all(&dir);
    }
}

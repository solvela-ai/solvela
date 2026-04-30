//! Shared CLI utilities — small helpers used by more than one command module.

use std::path::PathBuf;

/// Default Solana RPC URL when no env var is set.
const DEFAULT_RPC_URL: &str = "https://api.devnet.solana.com";

/// Resolve the configured Solana RPC URL.
///
/// Priority: `SOLVELA_SOLANA_RPC_URL` → `SOLANA_RPC_URL` →
/// `RCR_SOLANA_RPC_URL` (legacy) → devnet default.
pub fn resolve_rpc_url() -> String {
    std::env::var("SOLVELA_SOLANA_RPC_URL")
        .or_else(|_| std::env::var("SOLANA_RPC_URL"))
        .or_else(|_| std::env::var("RCR_SOLANA_RPC_URL"))
        .unwrap_or_else(|_| DEFAULT_RPC_URL.to_string())
}

/// Resolve the on-disk wallet file path.
///
/// Priority: `SOLVELA_WALLET_PATH` → `RCR_WALLET_PATH` (legacy) →
/// `~/.solvela/wallet.json`.
pub fn wallet_file_path() -> PathBuf {
    if let Ok(p) = std::env::var("SOLVELA_WALLET_PATH") {
        return PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("RCR_WALLET_PATH") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    home.join(".solvela").join("wallet.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_rpc_url_returns_devnet_default_when_no_env() {
        // Guard: temporarily clear any env vars that would affect the result.
        // Because tests may run in parallel, we only assert the fallback when
        // none of the recognised vars are set in this process.
        let solvela = std::env::var("SOLVELA_SOLANA_RPC_URL").ok();
        let solana = std::env::var("SOLANA_RPC_URL").ok();
        let rcr = std::env::var("RCR_SOLANA_RPC_URL").ok();
        if solvela.is_none() && solana.is_none() && rcr.is_none() {
            assert_eq!(resolve_rpc_url(), "https://api.devnet.solana.com");
        }
    }

    #[test]
    fn wallet_file_path_ends_with_expected_suffix() {
        // When no override env vars are set, the path ends with the standard
        // relative suffix.  We don't test the full absolute path because HOME
        // varies per machine.
        let solvela_wallet = std::env::var("SOLVELA_WALLET_PATH").ok();
        let rcr_wallet = std::env::var("RCR_WALLET_PATH").ok();
        if solvela_wallet.is_none() && rcr_wallet.is_none() {
            let p = wallet_file_path();
            assert!(
                p.to_string_lossy().ends_with(".solvela/wallet.json"),
                "unexpected path: {p:?}"
            );
        }
    }
}

use std::time::Duration;

/// Errors from wallet operations (key management, balance checks).
#[derive(Debug, thiserror::Error)]
pub enum WalletError {
    #[error("invalid mnemonic: {0}")]
    InvalidMnemonic(String),

    #[error("invalid keypair: {0}")]
    InvalidKeypair(String),

    #[error("environment variable not set: {0}")]
    EnvNotSet(String),

    #[error("RPC error: {0}")]
    RpcError(String),
}

/// Errors from transaction signing.
#[derive(Debug, thiserror::Error)]
pub enum SignerError {
    #[error("RPC error: {0}")]
    RpcError(String),

    #[error("no associated token account for {0}")]
    NoAssociatedTokenAccount(String),

    #[error("transaction build error: {0}")]
    TransactionBuild(String),
}

/// Top-level errors from the `SolvelaClient`.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("wallet error: {0}")]
    Wallet(#[from] WalletError),

    #[error("insufficient USDC balance: have {have} atomic, need {need} atomic")]
    InsufficientBalance { have: u64, need: u64 },

    #[error("payment signing failed: {0}")]
    Signing(#[from] SignerError),

    #[error("gateway error ({status}): {message}")]
    Gateway { status: u16, message: String },

    #[error("payment rejected by gateway: {0}")]
    PaymentRejected(String),

    #[error("no compatible payment scheme in 402 response")]
    NoCompatibleScheme,

    #[error("model not found: {0}")]
    ModelNotFound(String),

    #[error("request timeout after {0:?}")]
    Timeout(Duration),

    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("stream interrupted: {0}")]
    StreamError(String),

    #[error("failed to parse response: {0}")]
    ParseError(String),

    #[error("payment recipient mismatch: expected {expected}, got {actual}")]
    RecipientMismatch { expected: String, actual: String },

    #[error("payment amount {amount} exceeds maximum allowed {max}")]
    AmountExceedsMax { amount: u64, max: u64 },

    #[error("cumulative payments {spent} atomic exceed budget {cap} atomic for this request")]
    BudgetExceeded { spent: u64, cap: u64 },

    #[error("client configuration error: {0}")]
    Config(String),

    #[error("balance check failed: {0}")]
    BalanceError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wallet_error_display() {
        let err = WalletError::InvalidMnemonic("bad phrase".to_string());
        assert_eq!(err.to_string(), "invalid mnemonic: bad phrase");
    }

    #[test]
    fn test_wallet_error_invalid_keypair() {
        let err = WalletError::InvalidKeypair("not base58".to_string());
        assert_eq!(err.to_string(), "invalid keypair: not base58");
    }

    #[test]
    fn test_wallet_error_env_not_set() {
        let err = WalletError::EnvNotSet("MY_VAR".to_string());
        assert_eq!(err.to_string(), "environment variable not set: MY_VAR");
    }

    #[test]
    fn test_signer_error_display() {
        let err = SignerError::RpcError("connection refused".to_string());
        assert_eq!(err.to_string(), "RPC error: connection refused");
    }

    #[test]
    fn test_signer_error_no_ata() {
        let err = SignerError::NoAssociatedTokenAccount("abc123".to_string());
        assert!(err.to_string().contains("abc123"));
    }

    #[test]
    fn test_client_error_insufficient_balance() {
        let err = ClientError::InsufficientBalance {
            have: 1_000_000,
            need: 5_000_000,
        };
        assert!(err.to_string().contains("1000000"));
        assert!(err.to_string().contains("5000000"));
    }

    #[test]
    fn test_client_error_gateway() {
        let err = ClientError::Gateway {
            status: 500,
            message: "internal error".to_string(),
        };
        assert!(err.to_string().contains("500"));
        assert!(err.to_string().contains("internal error"));
    }

    #[test]
    fn test_client_error_timeout() {
        let err = ClientError::Timeout(Duration::from_secs(30));
        assert!(err.to_string().contains("30"));
    }

    #[test]
    fn test_client_error_no_compatible_scheme() {
        let err = ClientError::NoCompatibleScheme;
        assert!(err.to_string().contains("no compatible payment scheme"));
    }

    #[test]
    fn test_client_error_from_wallet() {
        let wallet_err = WalletError::InvalidMnemonic("bad".to_string());
        let client_err: ClientError = wallet_err.into();
        assert!(matches!(client_err, ClientError::Wallet(_)));
    }

    #[test]
    fn test_client_error_from_signer() {
        let signer_err = SignerError::RpcError("timeout".to_string());
        let client_err: ClientError = signer_err.into();
        assert!(matches!(client_err, ClientError::Signing(_)));
    }

    #[test]
    fn test_client_error_recipient_mismatch() {
        let err = ClientError::RecipientMismatch {
            expected: "expected_addr".to_string(),
            actual: "actual_addr".to_string(),
        };
        assert!(err.to_string().contains("expected_addr"));
        assert!(err.to_string().contains("actual_addr"));
    }

    #[test]
    fn test_client_error_amount_exceeds_max() {
        let err = ClientError::AmountExceedsMax {
            amount: 1_000_000,
            max: 500_000,
        };
        assert!(err.to_string().contains("1000000"));
        assert!(err.to_string().contains("500000"));
    }
}

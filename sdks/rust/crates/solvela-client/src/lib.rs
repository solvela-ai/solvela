pub mod balance;
pub(crate) mod cache;
pub mod client;
pub mod config;
pub mod error;
pub(crate) mod quality;
pub(crate) mod rpc_error;
#[allow(dead_code)] // SessionInfo::escalated and cleanup_expired are reserved for future use
pub(crate) mod session;
pub(crate) mod signer;
pub mod wallet;

pub use balance::BalanceMonitor;
pub use client::SolvelaClient;
pub use config::{ClientBuilder, ClientConfig, DEFAULT_MAX_PAYMENT_AMOUNT_ATOMIC};
pub use error::{ClientError, SignerError, WalletError};
// NOTE: `signer::EXACT_GOLDEN_VECTOR_B64` is intentionally NOT re-exported here.
// It is a `pub(crate)` test-pinning artifact (the `exact`-scheme golden vector),
// not a stable public API: nothing outside this crate imports it (the x402
// gateway test duplicates the literal; the cross-SDK drift guards read it from
// source text). Re-exporting it would semver-imply a stability guarantee it does
// not carry.
pub use wallet::Wallet;

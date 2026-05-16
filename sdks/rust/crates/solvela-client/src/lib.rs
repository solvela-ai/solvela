pub mod balance;
pub(crate) mod cache;
pub mod client;
pub mod config;
pub mod error;
pub(crate) mod quality;
#[allow(dead_code)] // SessionInfo::escalated and cleanup_expired are reserved for future use
pub(crate) mod session;
pub(crate) mod signer;
pub mod wallet;

pub use balance::BalanceMonitor;
pub use client::SolvelaClient;
pub use config::{ClientBuilder, ClientConfig, DEFAULT_MAX_PAYMENT_AMOUNT_ATOMIC};
pub use error::{ClientError, SignerError, WalletError};
pub use wallet::Wallet;

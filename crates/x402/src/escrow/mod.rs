//! EscrowVerifier — verifies on-chain escrow deposits and fires claim transactions.
//!
//! The EscrowVerifier handles scheme="escrow" payments where agents deposit
//! to a PDA vault rather than sending a direct SPL transfer. After the gateway
//! proxies the request to the LLM provider, the EscrowClaimer fires a
//! fire-and-forget claim transaction to collect the actual cost from the vault.

#[cfg(feature = "postgres")]
pub mod claim_processor;
#[cfg(feature = "postgres")]
pub mod claim_queue;

mod claimer;
mod pda;
mod verifier;

pub use claimer::{do_claim_with_params, EscrowClaimer};
pub use verifier::EscrowVerifier;

pub mod canonical;
pub mod chat;
pub mod constants;
pub mod cost;
pub mod model;
pub mod payment;
pub mod settlement;
pub mod streaming;
pub mod tools;
pub mod vision;

// Flat re-exports so consumers write:
//   use solvela_protocol::{ChatRequest, PaymentRequired, CostBreakdown};
// Canonical x402 v2 wire-compat items are re-exported EXPLICITLY (not glob)
// so the canonical surface that crosses the crate boundary stays auditable;
// supporting types (CanonicalAccept, CanonicalResource, CanonicalProof, …)
// remain reachable via `solvela_protocol::canonical::`.
pub use canonical::{
    CanonicalPaymentEnvelope, CanonicalPaymentError, CanonicalPaymentRequired,
    CANONICAL_PAYMENT_REQUIRED_HEADER, CANONICAL_X402_VERSION,
};
pub use chat::*;
pub use constants::*;
pub use cost::*;
pub use model::*;
pub use payment::*;
pub use settlement::*;
pub use streaming::*;
pub use tools::*;
pub use vision::*;

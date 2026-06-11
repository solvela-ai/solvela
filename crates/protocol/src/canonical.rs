//! Canonical x402 v2 wire-compatibility types.
//!
//! Solvela's native 402 body and `PAYMENT-SIGNATURE` payload predate the
//! canonical x402 v2 wire format and use a snake_case dialect that the
//! published SDKs (`@solvela/sdk`, `solvela-sdk` PyPI, Go, `solvela-client`)
//! parse byte-for-byte. Those shapes are frozen — see [`crate::payment`].
//!
//! This module adds the CANONICAL x402 v2 shapes alongside them, pinned to
//! what the `pay` CLI (solana-foundation/pay, backed by the
//! solana-foundation/pay-kit `solana-x402` crate) actually parses and emits:
//!
//! - **Outbound challenge** — [`CanonicalPaymentRequired`], serialized as
//!   camelCase JSON, base64-encoded into the `PAYMENT-REQUIRED` response
//!   header ([`CANONICAL_PAYMENT_REQUIRED_HEADER`]) on the same 402 response
//!   that carries the unchanged legacy body. The pay client checks this
//!   header BEFORE any body parsing and treats its presence as x402 v2.
//! - **Inbound payment** — [`CanonicalPaymentEnvelope`], the v2
//!   `PaymentSignatureEnvelope` the pay client base64-encodes into the
//!   `PAYMENT-SIGNATURE` request header:
//!   `{ x402Version: 2, accepted, resource, payload: { transaction } }`.
//!   [`CanonicalPaymentEnvelope::into_payment_payload`] maps it (fail-closed)
//!   into the legacy [`PaymentPayload`] so the existing verification pipeline
//!   runs unchanged.
//!
//! Money-path rules enforced here:
//! - Only scheme `"exact"` crosses the canonical surface — escrow is never
//!   advertised on it and is rejected inbound ([`CanonicalPaymentError`]).
//! - Only a signed `transaction` proof is accepted (the gateway controls the
//!   broadcast; a client-broadcast `signature` proof is a different trust
//!   model and is rejected).
//! - `extra.feePayer` is deliberately NEVER advertised: the gateway's exact
//!   verifier requires `signatures[0]` to verify against `account_keys[0]`
//!   and broadcasts without countersigning, so a server-fee-payer transaction
//!   (empty fee-payer signature slot) would always be rejected. With the
//!   field absent the pay client self-pays the SOL fee and submits a
//!   fully-signed transaction the existing verifier accepts.
//!
//! Unlike the legacy types, the inbound canonical types do NOT use
//! `deny_unknown_fields`: the x402 v2 spec (§5.1.2) lets clients echo and
//! append envelope fields, and the mapping below copies ONLY the validated,
//! known fields into [`PaymentPayload`] — unknown keys are dropped at the
//! boundary and can never propagate into gateway logic.

use serde::{Deserialize, Serialize};

use crate::constants::X402_VERSION;
use crate::payment::{
    PayloadData, PaymentAccept, PaymentPayload, PaymentRequired, Resource, SolanaPayload,
};

/// Canonical x402 v2 challenge response header (lowercase for
/// `HeaderName::from_static`; HTTP header names are case-insensitive and the
/// pay client matches ignore-case). The pay-kit constant is
/// `PAYMENT_REQUIRED_HEADER = "PAYMENT-REQUIRED"`.
pub const CANONICAL_PAYMENT_REQUIRED_HEADER: &str = "payment-required";

/// The canonical x402 protocol version this surface speaks (`x402Version`).
pub const CANONICAL_X402_VERSION: u64 = 2;

/// USDC token decimals, advertised in `accepts[].extra.decimals` so canonical
/// clients build `TransferChecked` with the decimals the verifier expects.
/// Solvela prices everything in 6-decimal atomic USDC units.
const USDC_DECIMALS: u8 = 6;

/// Maximum characters of a client-supplied scheme echoed into an error
/// message (GHSA-cgqx-mg48-949v posture: never reflect unbounded
/// attacker-controlled bytes).
const MAX_ECHOED_SCHEME_CHARS: usize = 32;

/// Why a canonical inbound payment envelope was rejected.
///
/// These messages may be logged server-side; the route handlers return their
/// own fixed client-facing strings, so nothing here is reflected verbatim to
/// clients beyond the (length-capped) scheme.
#[derive(Debug, thiserror::Error)]
pub enum CanonicalPaymentError {
    #[error("unsupported x402Version {0}; this gateway accepts canonical x402 version 2")]
    UnsupportedVersion(u64),

    #[error("canonical payment envelope is missing the `accepted` object")]
    MissingAccepted,

    #[error(
        "canonical payment envelope is missing the `resource` object; \
         echo the resource from the PAYMENT-REQUIRED challenge"
    )]
    MissingResource,

    #[error("unsupported scheme '{0}' on the canonical x402 surface; only 'exact' is accepted")]
    UnsupportedScheme(String),

    #[error(
        "unsupported payment proof: only a signed `transaction` is accepted \
         (the gateway broadcasts after delivery; client-broadcast `signature` \
         proofs are not supported)"
    )]
    UnsupportedProof,
}

/// Canonical v2 resource metadata (`resource` on both envelopes).
/// Mirrors pay-kit `ResourceInfo`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalResource {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// One canonical `accepts[]` entry / the echoed `accepted` object.
///
/// Field names are the v2 wire contract the pay client parses:
/// `scheme`, `network` (CAIP-2), `amount` (atomic-unit string), `asset`
/// (mint address), `payTo` (recipient wallet), `maxTimeoutSeconds`, `extra`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalAccept {
    pub scheme: String,
    pub network: String,
    pub amount: String,
    pub asset: String,
    pub pay_to: String,
    pub max_timeout_seconds: u64,
    /// Scheme-specific extras (`decimals`, never `feePayer` — see module
    /// docs). Echoed back by clients; ignored on the inbound mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

/// Canonical v2 challenge envelope carried (base64-encoded) in the
/// `PAYMENT-REQUIRED` response header. Mirrors pay-kit
/// `PaymentRequiredEnvelope`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalPaymentRequired {
    pub x402_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<CanonicalResource>,
    pub accepts: Vec<CanonicalAccept>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl CanonicalPaymentRequired {
    /// Project the legacy 402 body onto the canonical v2 challenge.
    ///
    /// Copies the already-quoted atomic amount strings verbatim — no amount
    /// math happens here. Only `exact` entries cross the canonical surface;
    /// returns `None` when the legacy challenge offers no `exact` scheme, in
    /// which case the caller must NOT emit a canonical header (fail closed —
    /// never advertise a challenge canonical clients cannot satisfy).
    pub fn from_payment_required(legacy: &PaymentRequired) -> Option<Self> {
        let accepts: Vec<CanonicalAccept> = legacy
            .accepts
            .iter()
            .filter(|accept| accept.scheme == "exact")
            .map(|accept| CanonicalAccept {
                scheme: accept.scheme.clone(),
                network: accept.network.clone(),
                amount: accept.amount.clone(),
                asset: accept.asset.clone(),
                pay_to: accept.pay_to.clone(),
                max_timeout_seconds: accept.max_timeout_seconds,
                // decimals only — NEVER feePayer (see module docs).
                extra: Some(serde_json::json!({ "decimals": USDC_DECIMALS })),
            })
            .collect();

        if accepts.is_empty() {
            return None;
        }

        Some(Self {
            x402_version: CANONICAL_X402_VERSION,
            resource: Some(CanonicalResource {
                url: legacy.resource.url.clone(),
                description: None,
                mime_type: None,
            }),
            accepts,
            error: Some(legacy.error.clone()),
        })
    }
}

/// Canonical payment proof — mirrors pay-kit `PaymentProof` (untagged).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CanonicalProof {
    /// Client sends signed transaction bytes for the gateway to broadcast.
    Transaction {
        /// Base64-encoded serialized signed transaction.
        transaction: String,
    },
    /// Client broadcast itself and sends the confirmed signature. NOT
    /// accepted by the mapping below (the gateway must control broadcast).
    Signature {
        /// Base58-encoded transaction signature.
        signature: String,
    },
}

/// Canonical v2 payment envelope carried (base64-encoded) in the
/// `PAYMENT-SIGNATURE` request header. Mirrors pay-kit
/// `PaymentSignatureEnvelope`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalPaymentEnvelope {
    /// Top-level scheme — only set by v1 clients; ignored (v2 carries the
    /// scheme inside `accepted`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    /// Top-level network — only set by v1 clients; ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    pub x402_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<CanonicalAccept>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<CanonicalResource>,
    pub payload: CanonicalProof,
    /// Echoed v2 extensions — ignored (Solvela advertises none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<serde_json::Value>,
}

impl CanonicalPaymentEnvelope {
    /// Map a canonical v2 payment envelope into the legacy [`PaymentPayload`]
    /// the existing verification pipeline consumes. Fail-closed: every
    /// rejection is an explicit error, never a default.
    ///
    /// The legacy `resource.method` has no canonical equivalent; it is set to
    /// `"POST"` because every payment-gated Solvela endpoint is POST-only and
    /// the route handlers independently require POST.
    pub fn into_payment_payload(self) -> Result<PaymentPayload, CanonicalPaymentError> {
        if self.x402_version != CANONICAL_X402_VERSION {
            return Err(CanonicalPaymentError::UnsupportedVersion(self.x402_version));
        }

        let accepted = self
            .accepted
            .ok_or(CanonicalPaymentError::MissingAccepted)?;
        if accepted.scheme != "exact" {
            let scheme: String = accepted
                .scheme
                .chars()
                .take(MAX_ECHOED_SCHEME_CHARS)
                .collect();
            return Err(CanonicalPaymentError::UnsupportedScheme(scheme));
        }

        let resource = self
            .resource
            .ok_or(CanonicalPaymentError::MissingResource)?;

        let transaction = match self.payload {
            CanonicalProof::Transaction { transaction } => transaction,
            CanonicalProof::Signature { .. } => {
                return Err(CanonicalPaymentError::UnsupportedProof);
            }
        };

        Ok(PaymentPayload {
            x402_version: X402_VERSION,
            resource: Resource {
                url: resource.url,
                method: "POST".to_string(),
            },
            accepted: PaymentAccept {
                scheme: accepted.scheme,
                network: accepted.network,
                amount: accepted.amount,
                asset: accepted.asset,
                pay_to: accepted.pay_to,
                max_timeout_seconds: accepted.max_timeout_seconds,
                escrow_program_id: None,
            },
            payload: PayloadData::Direct(SolanaPayload { transaction }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{MAX_TIMEOUT_SECONDS, SOLANA_NETWORK, USDC_MINT};
    use crate::cost::CostBreakdown;

    fn legacy_payment_required(accepts: Vec<PaymentAccept>) -> PaymentRequired {
        PaymentRequired {
            x402_version: X402_VERSION,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepts,
            cost_breakdown: CostBreakdown {
                provider_cost: "0.002500".to_string(),
                platform_fee: "0.000125".to_string(),
                total: "0.002625".to_string(),
                currency: "USDC".to_string(),
                fee_percent: 5,
            },
            error: "Payment required".to_string(),
        }
    }

    fn exact_accept() -> PaymentAccept {
        PaymentAccept {
            scheme: "exact".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: "2625".to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: "6cvgmdrsVxyiuPzqMCSBnS7fAmA5Mk2VG4BcfVhC8jdC".to_string(),
            max_timeout_seconds: MAX_TIMEOUT_SECONDS,
            escrow_program_id: None,
        }
    }

    fn escrow_accept() -> PaymentAccept {
        PaymentAccept {
            scheme: "escrow".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: "2625".to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: "6cvgmdrsVxyiuPzqMCSBnS7fAmA5Mk2VG4BcfVhC8jdC".to_string(),
            max_timeout_seconds: MAX_TIMEOUT_SECONDS,
            escrow_program_id: Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string()),
        }
    }

    /// Golden vector (outbound): the serialized canonical challenge must use
    /// exactly the field names/casing the pay client's
    /// `PaymentRequiredEnvelope`/`PaymentRequirements` parser consumes
    /// (pay-kit rust/crates/x402 — `x402Version`, `accepts[].payTo`,
    /// `maxTimeoutSeconds`, CAIP-2 `network`, mint `asset`, atomic-string
    /// `amount`).
    #[test]
    fn canonical_challenge_serializes_to_pay_client_shape() {
        let canonical =
            CanonicalPaymentRequired::from_payment_required(&legacy_payment_required(vec![
                exact_accept(),
            ]))
            .expect("exact accept must produce a canonical challenge");

        let value = serde_json::to_value(&canonical).unwrap();
        let expected = serde_json::json!({
            "x402Version": 2,
            "resource": {
                "url": "/v1/chat/completions"
            },
            "accepts": [{
                "scheme": "exact",
                "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
                "amount": "2625",
                "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                "payTo": "6cvgmdrsVxyiuPzqMCSBnS7fAmA5Mk2VG4BcfVhC8jdC",
                "maxTimeoutSeconds": 300,
                "extra": { "decimals": 6 }
            }],
            "error": "Payment required"
        });
        assert_eq!(value, expected, "canonical challenge wire shape drifted");
    }

    /// The canonical challenge never advertises escrow, and never a feePayer.
    #[test]
    fn canonical_challenge_excludes_escrow_and_fee_payer() {
        let canonical =
            CanonicalPaymentRequired::from_payment_required(&legacy_payment_required(vec![
                exact_accept(),
                escrow_accept(),
            ]))
            .expect("exact accept present");

        assert_eq!(canonical.accepts.len(), 1);
        assert_eq!(canonical.accepts[0].scheme, "exact");
        let extra = canonical.accepts[0].extra.as_ref().unwrap();
        assert!(
            extra.get("feePayer").is_none(),
            "feePayer must never be advertised (verifier requires a fully-signed tx)"
        );
    }

    /// No exact entry → no canonical challenge (fail closed: do not advertise
    /// a surface canonical clients cannot pay on).
    #[test]
    fn canonical_challenge_none_without_exact_scheme() {
        let canonical =
            CanonicalPaymentRequired::from_payment_required(&legacy_payment_required(vec![
                escrow_accept(),
            ]));
        assert!(canonical.is_none());
    }

    /// Golden vector (inbound): a `PAYMENT-SIGNATURE` envelope shaped exactly
    /// like the pay client's `build_payment_header` output — accepted object
    /// taken verbatim from the pay client's own V2 test fixture
    /// (solana-foundation/pay x402.rs::parse_v2_payment_required_header_…),
    /// including the `extra.feePayer` echo — must map into the legacy
    /// `PaymentPayload`.
    #[test]
    fn pay_client_v2_envelope_maps_to_legacy_payment_payload() {
        let envelope_json = serde_json::json!({
            "x402Version": 2,
            "accepted": {
                "scheme": "exact",
                "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
                "amount": "10000",
                "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                "payTo": "6cvgmdrsVxyiuPzqMCSBnS7fAmA5Mk2VG4BcfVhC8jdC",
                "maxTimeoutSeconds": 300,
                "extra": {
                    "feePayer": "AepWpq3GQwL8CeKMtZyKtKPa7W91Coygh3ropAJapVdU",
                    "decimals": 6
                }
            },
            "resource": {
                "url": "https://api.example.com/v1/test",
                "description": "API access"
            },
            "payload": { "transaction": "dGVzdA==" }
        });

        let envelope: CanonicalPaymentEnvelope =
            serde_json::from_value(envelope_json).expect("pay-shaped envelope must parse");
        let payload = envelope
            .into_payment_payload()
            .expect("pay-shaped envelope must map");

        assert_eq!(payload.x402_version, 2);
        assert_eq!(payload.resource.url, "https://api.example.com/v1/test");
        assert_eq!(payload.resource.method, "POST");
        assert_eq!(payload.accepted.scheme, "exact");
        assert_eq!(
            payload.accepted.network,
            "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"
        );
        assert_eq!(payload.accepted.amount, "10000");
        assert_eq!(
            payload.accepted.asset,
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
        assert_eq!(
            payload.accepted.pay_to,
            "6cvgmdrsVxyiuPzqMCSBnS7fAmA5Mk2VG4BcfVhC8jdC"
        );
        assert_eq!(payload.accepted.max_timeout_seconds, 300);
        assert_eq!(payload.accepted.escrow_program_id, None);
        match payload.payload {
            PayloadData::Direct(p) => assert_eq!(p.transaction, "dGVzdA=="),
            PayloadData::Escrow(_) => panic!("must map to Direct"),
        }
    }

    /// Round trip: the accepted entry the gateway advertises, echoed verbatim
    /// (as the pay client does), maps back to identical legacy fields.
    #[test]
    fn advertised_accept_echo_round_trips() {
        let canonical =
            CanonicalPaymentRequired::from_payment_required(&legacy_payment_required(vec![
                exact_accept(),
            ]))
            .unwrap();
        let advertised = serde_json::to_value(&canonical.accepts[0]).unwrap();

        let envelope_json = serde_json::json!({
            "x402Version": 2,
            "accepted": advertised,
            "resource": serde_json::to_value(canonical.resource.as_ref().unwrap()).unwrap(),
            "payload": { "transaction": "dGVzdA==" }
        });
        let envelope: CanonicalPaymentEnvelope = serde_json::from_value(envelope_json).unwrap();
        let payload = envelope.into_payment_payload().unwrap();

        let original = exact_accept();
        assert_eq!(payload.accepted.scheme, original.scheme);
        assert_eq!(payload.accepted.network, original.network);
        assert_eq!(payload.accepted.amount, original.amount);
        assert_eq!(payload.accepted.asset, original.asset);
        assert_eq!(payload.accepted.pay_to, original.pay_to);
        assert_eq!(
            payload.accepted.max_timeout_seconds,
            original.max_timeout_seconds
        );
        assert_eq!(payload.resource.url, "/v1/chat/completions");
    }

    #[test]
    fn envelope_rejects_non_v2_version() {
        let envelope_json = serde_json::json!({
            "x402Version": 1,
            "scheme": "exact",
            "network": "solana",
            "payload": { "transaction": "dGVzdA==" }
        });
        let envelope: CanonicalPaymentEnvelope = serde_json::from_value(envelope_json).unwrap();
        let err = envelope.into_payment_payload().unwrap_err();
        assert!(matches!(err, CanonicalPaymentError::UnsupportedVersion(1)));
    }

    #[test]
    fn envelope_rejects_missing_accepted() {
        let envelope_json = serde_json::json!({
            "x402Version": 2,
            "resource": { "url": "/v1/chat/completions" },
            "payload": { "transaction": "dGVzdA==" }
        });
        let envelope: CanonicalPaymentEnvelope = serde_json::from_value(envelope_json).unwrap();
        let err = envelope.into_payment_payload().unwrap_err();
        assert!(matches!(err, CanonicalPaymentError::MissingAccepted));
    }

    #[test]
    fn envelope_rejects_missing_resource() {
        let envelope_json = serde_json::json!({
            "x402Version": 2,
            "accepted": {
                "scheme": "exact",
                "network": SOLANA_NETWORK,
                "amount": "2625",
                "asset": USDC_MINT,
                "payTo": "6cvgmdrsVxyiuPzqMCSBnS7fAmA5Mk2VG4BcfVhC8jdC",
                "maxTimeoutSeconds": 300
            },
            "payload": { "transaction": "dGVzdA==" }
        });
        let envelope: CanonicalPaymentEnvelope = serde_json::from_value(envelope_json).unwrap();
        let err = envelope.into_payment_payload().unwrap_err();
        assert!(matches!(err, CanonicalPaymentError::MissingResource));
    }

    /// Escrow must never enter through the canonical surface, and the echoed
    /// scheme in the error is length-capped (reflected-injection posture).
    #[test]
    fn envelope_rejects_non_exact_scheme_with_capped_echo() {
        let long_scheme = "escrow".to_string() + &"x".repeat(200);
        let envelope_json = serde_json::json!({
            "x402Version": 2,
            "accepted": {
                "scheme": long_scheme,
                "network": SOLANA_NETWORK,
                "amount": "2625",
                "asset": USDC_MINT,
                "payTo": "6cvgmdrsVxyiuPzqMCSBnS7fAmA5Mk2VG4BcfVhC8jdC",
                "maxTimeoutSeconds": 300
            },
            "resource": { "url": "/v1/chat/completions" },
            "payload": { "transaction": "dGVzdA==" }
        });
        let envelope: CanonicalPaymentEnvelope = serde_json::from_value(envelope_json).unwrap();
        match envelope.into_payment_payload().unwrap_err() {
            CanonicalPaymentError::UnsupportedScheme(scheme) => {
                assert!(scheme.starts_with("escrow"));
                assert!(
                    scheme.chars().count() <= MAX_ECHOED_SCHEME_CHARS,
                    "echoed scheme must be length-capped"
                );
            }
            other => panic!("expected UnsupportedScheme, got {other:?}"),
        }
    }

    #[test]
    fn envelope_rejects_signature_proof() {
        let envelope_json = serde_json::json!({
            "x402Version": 2,
            "accepted": {
                "scheme": "exact",
                "network": SOLANA_NETWORK,
                "amount": "2625",
                "asset": USDC_MINT,
                "payTo": "6cvgmdrsVxyiuPzqMCSBnS7fAmA5Mk2VG4BcfVhC8jdC",
                "maxTimeoutSeconds": 300
            },
            "resource": { "url": "/v1/chat/completions" },
            "payload": { "signature": "5wZ2UM8fFakeSignature" }
        });
        let envelope: CanonicalPaymentEnvelope = serde_json::from_value(envelope_json).unwrap();
        let err = envelope.into_payment_payload().unwrap_err();
        assert!(matches!(err, CanonicalPaymentError::UnsupportedProof));
    }

    /// A legacy Solvela `PaymentPayload` JSON must NOT parse as a canonical
    /// envelope (it lacks `x402Version`), so the two surfaces can never be
    /// confused by an inbound decoder that tries both.
    #[test]
    fn legacy_payload_json_does_not_parse_as_canonical_envelope() {
        let legacy = PaymentPayload {
            x402_version: 2,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: exact_accept(),
            payload: PayloadData::Direct(SolanaPayload {
                transaction: "dGVzdA==".to_string(),
            }),
        };
        let json = serde_json::to_string(&legacy).unwrap();
        assert!(
            serde_json::from_str::<CanonicalPaymentEnvelope>(&json).is_err(),
            "legacy snake_case payload must not be parseable as a canonical envelope"
        );
    }

    /// Header constant pin — the pay client looks for `PAYMENT-REQUIRED`
    /// (case-insensitive).
    #[test]
    fn canonical_header_constant_pinned() {
        assert_eq!(CANONICAL_PAYMENT_REQUIRED_HEADER, "payment-required");
        assert!(CANONICAL_PAYMENT_REQUIRED_HEADER.eq_ignore_ascii_case("PAYMENT-REQUIRED"));
    }
}

//! Nano (XNO) receipt verifier — `exact` scheme over `nano:mainnet`.
//!
//! # Settlement model: the receipt IS the settlement
//!
//! Nano is feeless and final at the block: there is no mempool, no fee
//! market, and no transaction the gateway could hold and broadcast later. So
//! unlike [`crate::solana::SolanaVerifier`] (verify a signed-but-unsent
//! transaction, call the provider, then broadcast), the Nano path is:
//!
//! 1. The client publishes its own `send` block to the Nano network and waits
//!    for confirmation (sub-second).
//! 2. The client presents `{ block_hash, account }` as the x402 payload.
//! 3. [`NanoVerifier::verify_payment`] asks a Nano node for `block_info` and
//!    checks: confirmed, `subtype == "send"`, paid to the advertised `pay_to`,
//!    sent by `account`, `amount` ≥ the advertised raw amount, and that the
//!    node first saw the block within [`NANO_RECEIPT_MAX_AGE_SECS`].
//! 4. [`NanoVerifier::settle_payment`] is a no-op that reports success — the
//!    funds already moved in step 1.
//!
//! Consequence the operator must accept: a provider failure AFTER a valid
//! receipt is a paid-but-undelivered call (the Solana `exact` path's deferred
//! broadcast cannot be replicated here). The gateway's provider fallback chain
//! makes that rare; it is the same posture the `escrow` scheme has always had
//! (deposit lands before serving).
//!
//! # Replay
//!
//! A block hash never expires on-chain, so freshness — not the chain — bounds
//! replay. The verifier rejects any block the node first saw more than
//! [`NANO_RECEIPT_MAX_AGE_SECS`] (90 s) ago; the gateway's existing replay
//! cache (keyed on the hash string, 120 s TTL on both the Redis and in-memory
//! paths) therefore covers a Nano receipt exactly the way it covers a Solana
//! recent-blockhash signature. The hash must be the canonical uppercase hex
//! spelling so a used receipt has exactly one cache key.
//!
//! # Trust
//!
//! The only trusted party is the configured Nano node (`rpc_url`) — it is the
//! source of `confirmed`, `amount`, `link_as_account`, and `local_timestamp`.
//! Point it at a node you run or one you trust; a lying node could attest a
//! payment that never happened. This is the same trust the Solana path places
//! in its RPC endpoint.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use reqwest::Client;
use tracing::{debug, info, warn};

use solvela_protocol::nano::{
    is_well_formed_block_hash, is_well_formed_nano_address, NANO_ASSET, NANO_NETWORK,
    NANO_RECEIPT_MAX_AGE_SECS,
};

use crate::traits::{Error, PaymentVerifier};
use crate::types::{PayloadData, PaymentPayload, SettlementResult, VerificationResult};

/// Per-request timeout for the Nano node RPC call.
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Tolerated clock skew between the node's `local_timestamp` and this host:
/// a block the node stamps a few seconds "in the future" is still fresh.
const FUTURE_SKEW_SECS: u64 = 5;

/// The subset of a Nano `block_info` response the verifier reads.
///
/// Field names mirror the Nano RPC (`json_block: true` form). All numeric
/// values arrive as strings and are parsed with `u128`/`u64` — never floats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockInfo {
    /// The account that signed (and therefore owns) the block — the payer.
    pub block_account: String,
    /// For a send: the raw amount transferred. String in the RPC.
    pub amount: String,
    /// `"true"` once the network has cemented the block.
    pub confirmed: String,
    /// `"send"` / `"receive"` / `"change"` / `"epoch"`.
    pub subtype: String,
    /// Unix seconds when THIS node first saw the block (`"0"` if unknown).
    pub local_timestamp: String,
    /// For a send: the destination rendered as a `nano_` address.
    pub link_as_account: String,
}

impl BlockInfo {
    /// Pull the needed fields out of a raw `block_info` JSON response.
    ///
    /// A `{"error": ...}` body (e.g. `"Block not found"`) is surfaced as
    /// [`Error::InvalidTransaction`]; a response missing any needed field is
    /// [`Error::Rpc`]. Both fail closed.
    pub fn from_rpc_response(value: &serde_json::Value) -> Result<Self, Error> {
        if let Some(err) = value.get("error") {
            let msg = err.as_str().unwrap_or("unknown node error");
            return Err(Error::InvalidTransaction(format!(
                "nano node rejected block lookup: {msg}"
            )));
        }
        let str_field = |obj: &serde_json::Value, key: &str| -> Result<String, Error> {
            obj.get(key)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| Error::Rpc(format!("block_info response missing `{key}`")))
        };
        let contents = value
            .get("contents")
            .filter(|c| c.is_object())
            .ok_or_else(|| Error::Rpc("block_info response missing `contents`".to_string()))?;
        // `subtype` and `link_as_account` exist only on STATE blocks. A legacy
        // pre-epoch block (`type: open/send/receive/change`, with `destination`
        // instead of `link_as_account`) carries neither; it decodes with empty
        // strings so `evaluate_receipt` rejects it with a precise reason
        // ("subtype is ''") rather than this looking like an RPC outage.
        let optional_str = |obj: &serde_json::Value, key: &str| -> String {
            obj.get(key)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        Ok(Self {
            block_account: str_field(value, "block_account")?,
            amount: str_field(value, "amount")?,
            confirmed: str_field(value, "confirmed")?,
            subtype: optional_str(value, "subtype"),
            local_timestamp: str_field(value, "local_timestamp")?,
            link_as_account: optional_str(contents, "link_as_account"),
        })
    }
}

/// What a receipt must satisfy — everything the gateway advertised plus the
/// payer the client claimed.
#[derive(Debug, Clone)]
pub struct ExpectedReceipt<'a> {
    /// The `nano_` address the send must pay to.
    pub recipient: &'a str,
    /// The `nano_` address the client claims sent the block.
    pub payer: &'a str,
    /// Minimum raw the send must carry (the advertised `amount`).
    pub min_raw: u128,
    /// Maximum age, in seconds, of the node's `local_timestamp`.
    pub max_age_secs: u64,
}

/// Pure receipt check — no I/O, fully unit-testable.
///
/// Returns `valid: false` with a reason for every rejection; never panics on
/// malformed node data (that is a rejection too).
pub fn evaluate_receipt(
    info: &BlockInfo,
    expected: &ExpectedReceipt<'_>,
    now_unix_secs: u64,
) -> VerificationResult {
    let invalid = |reason: String| VerificationResult {
        valid: false,
        reason: Some(reason),
        verified_amount: None,
    };

    if info.confirmed != "true" {
        return invalid("block is not yet confirmed by the network".to_string());
    }
    if info.subtype != "send" {
        return invalid(format!(
            "block subtype is '{}', expected 'send'",
            info.subtype
        ));
    }
    if info.link_as_account != expected.recipient {
        return invalid("send block does not pay the advertised recipient".to_string());
    }
    if info.block_account != expected.payer {
        return invalid("send block was not signed by the claimed payer account".to_string());
    }
    let amount: u128 = match info.amount.parse() {
        Ok(a) => a,
        Err(_) => return invalid("block amount is not a valid raw integer".to_string()),
    };
    if amount < expected.min_raw {
        return invalid(format!(
            "insufficient payment: block sends {amount} raw, required {} raw",
            expected.min_raw
        ));
    }
    let seen_at: u64 = match info.local_timestamp.parse() {
        Ok(t) if t > 0 => t,
        _ => return invalid("node has no local timestamp for this block".to_string()),
    };
    if seen_at > now_unix_secs.saturating_add(FUTURE_SKEW_SECS) {
        return invalid("block timestamp is in the future".to_string());
    }
    if now_unix_secs.saturating_sub(seen_at) > expected.max_age_secs {
        return invalid(format!(
            "receipt is stale: block first seen {}s ago, limit {}s",
            now_unix_secs.saturating_sub(seen_at),
            expected.max_age_secs
        ));
    }

    VerificationResult {
        valid: true,
        reason: None,
        // Raw amounts (10^30 per XNO) do not fit the u64 micro-USDC slot and
        // the gateway bills the micro-USD quote it advertised, never a
        // chain-derived figure — so no amount is reported here.
        verified_amount: None,
    }
}

/// `exact`-scheme verifier for `nano:mainnet` receipts.
pub struct NanoVerifier {
    rpc_url: String,
    recipient_account: String,
    http_client: Client,
}

impl std::fmt::Debug for NanoVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NanoVerifier")
            .field("rpc_url", &self.rpc_url)
            .field("recipient_account", &self.recipient_account)
            .finish_non_exhaustive()
    }
}

impl NanoVerifier {
    /// Build a verifier for the node at `rpc_url` that must be paid at
    /// `recipient_account` (a syntactically valid `nano_` address).
    pub fn new(rpc_url: &str, recipient_account: &str, http_client: Client) -> Result<Self, Error> {
        if rpc_url.trim().is_empty() {
            return Err(Error::InvalidConfig("nano rpc_url is empty".to_string()));
        }
        if !is_well_formed_nano_address(recipient_account) {
            return Err(Error::InvalidConfig(
                "nano recipient account is not a well-formed nano_ address".to_string(),
            ));
        }
        Ok(Self {
            rpc_url: rpc_url.to_string(),
            recipient_account: recipient_account.to_string(),
            http_client,
        })
    }

    /// The configured recipient `nano_` address.
    pub fn recipient_account(&self) -> &str {
        &self.recipient_account
    }

    /// `block_info` RPC for one hash.
    async fn fetch_block_info(&self, hash: &str) -> Result<BlockInfo, Error> {
        let body = serde_json::json!({
            "action": "block_info",
            "json_block": "true",
            "hash": hash,
        });
        let response = self
            .http_client
            .post(&self.rpc_url)
            .timeout(RPC_TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Rpc(format!("nano block_info request failed: {e}")))?;
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|e| Error::Rpc(format!("nano block_info response was not JSON: {e}")))?;
        BlockInfo::from_rpc_response(&value)
    }

    /// Shared body of `verify_payment` / `verify_payment_to`.
    async fn verify_against(
        &self,
        payload: &PaymentPayload,
        recipient: &str,
    ) -> Result<VerificationResult, Error> {
        let accepted = &payload.accepted;
        if accepted.network != NANO_NETWORK {
            return Err(Error::UnsupportedNetwork(accepted.network.clone()));
        }
        if accepted.scheme != "exact" {
            return Err(Error::PayloadMismatch(format!(
                "NanoVerifier handles scheme 'exact', got '{}'",
                accepted.scheme
            )));
        }
        if accepted.asset != NANO_ASSET {
            return Err(Error::WrongAsset {
                expected: NANO_ASSET.to_string(),
                actual: accepted.asset.chars().take(16).collect(),
            });
        }
        if accepted.pay_to != recipient {
            return Err(Error::WrongRecipient {
                expected: recipient.to_string(),
                actual: accepted.pay_to.chars().take(65).collect(),
            });
        }
        let receipt = match &payload.payload {
            PayloadData::Nano(p) => p,
            PayloadData::Direct(_) => {
                return Err(Error::PayloadMismatch(
                    "NanoVerifier received direct (Solana) payload; expected nano receipt"
                        .to_string(),
                ));
            }
            PayloadData::Escrow(_) => {
                return Err(Error::PayloadMismatch(
                    "NanoVerifier received escrow payload; expected nano receipt".to_string(),
                ));
            }
            PayloadData::Channel(_) => {
                return Err(Error::PayloadMismatch(
                    "NanoVerifier received channel payload; expected nano receipt".to_string(),
                ));
            }
        };
        if !is_well_formed_block_hash(&receipt.block_hash) {
            return Err(Error::InvalidEncoding(
                "nano block_hash must be 64 uppercase hex characters".to_string(),
            ));
        }
        if !is_well_formed_nano_address(&receipt.account) {
            return Err(Error::InvalidEncoding(
                "nano payer account is not a well-formed nano_ address".to_string(),
            ));
        }
        let min_raw: u128 = accepted.amount.parse().map_err(|_| {
            Error::InvalidTransaction("accepted.amount is not a valid raw integer".to_string())
        })?;

        debug!(
            block_hash = %receipt.block_hash,
            payer = %receipt.account,
            min_raw,
            "verifying nano receipt"
        );
        let info = self.fetch_block_info(&receipt.block_hash).await?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let result = evaluate_receipt(
            &info,
            &ExpectedReceipt {
                recipient,
                payer: &receipt.account,
                min_raw,
                max_age_secs: NANO_RECEIPT_MAX_AGE_SECS,
            },
            now,
        );
        if result.valid {
            info!(block_hash = %receipt.block_hash, "nano receipt verified");
        } else {
            warn!(
                block_hash = %receipt.block_hash,
                reason = ?result.reason,
                "nano receipt rejected"
            );
        }
        Ok(result)
    }
}

#[async_trait]
impl PaymentVerifier for NanoVerifier {
    fn network(&self) -> &str {
        NANO_NETWORK
    }

    fn scheme(&self) -> &str {
        "exact"
    }

    async fn verify_payment(&self, payload: &PaymentPayload) -> Result<VerificationResult, Error> {
        self.verify_against(payload, &self.recipient_account).await
    }

    /// Per-service `vendor_wallet` override: the receipt must pay the
    /// caller-supplied `nano_` address instead of the gateway's.
    async fn verify_payment_to(
        &self,
        payload: &PaymentPayload,
        expected_recipient: &str,
    ) -> Result<VerificationResult, Error> {
        if !is_well_formed_nano_address(expected_recipient) {
            return Err(Error::WrongRecipient {
                expected: "a well-formed nano_ address".to_string(),
                actual: expected_recipient.chars().take(65).collect(),
            });
        }
        self.verify_against(payload, expected_recipient).await
    }

    /// No-op: the funds moved when the client published the block (see the
    /// module docs). Reports the block hash as the settlement signature.
    async fn settle_payment(&self, payload: &PaymentPayload) -> Result<SettlementResult, Error> {
        let PayloadData::Nano(receipt) = &payload.payload else {
            return Err(Error::PayloadMismatch(
                "NanoVerifier cannot settle a non-nano payload".to_string(),
            ));
        };
        Ok(SettlementResult {
            success: true,
            tx_signature: Some(receipt.block_hash.clone()),
            network: NANO_NETWORK.to_string(),
            error: None,
            verified_amount: None,
            failure_kind: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{NanoPayload, PaymentAccept, Resource};

    const RECIPIENT: &str = "nano_3t6k35gi95xu6tergt6p69ck76ogmitsa8mnijtpxm9fkcm736xtoncuohr3";
    const PAYER: &str = "nano_1ipx847tk8o46pwxt5qjdbncjqcbwcc1rrmqnkztrfjy5k7z4imsrata9est";
    const HASH: &str = "991CF190094C00F0B68E2E5F75F6BEE95A2E0BD93CEAA4A6734DB9F19B728948";
    const NOW: u64 = 1_700_000_100;

    fn good_info() -> BlockInfo {
        BlockInfo {
            block_account: PAYER.to_string(),
            amount: "2100000000000000000000000000".to_string(),
            confirmed: "true".to_string(),
            subtype: "send".to_string(),
            local_timestamp: (NOW - 10).to_string(),
            link_as_account: RECIPIENT.to_string(),
        }
    }

    fn expected() -> ExpectedReceipt<'static> {
        ExpectedReceipt {
            recipient: RECIPIENT,
            payer: PAYER,
            min_raw: 2_100_000_000_000_000_000_000_000_000,
            max_age_secs: NANO_RECEIPT_MAX_AGE_SECS,
        }
    }

    #[test]
    fn accepts_a_fresh_confirmed_send_to_the_recipient() {
        let r = evaluate_receipt(&good_info(), &expected(), NOW);
        assert!(r.valid, "{:?}", r.reason);
        assert_eq!(r.verified_amount, None);
    }

    #[test]
    fn accepts_overpayment_and_small_future_skew() {
        let mut info = good_info();
        info.amount = "9999999999999999999999999999999".to_string();
        info.local_timestamp = (NOW + 3).to_string();
        assert!(evaluate_receipt(&info, &expected(), NOW).valid);
    }

    #[test]
    fn rejects_unconfirmed_block() {
        let mut info = good_info();
        info.confirmed = "false".to_string();
        let r = evaluate_receipt(&info, &expected(), NOW);
        assert!(!r.valid);
        assert!(r.reason.unwrap().contains("not yet confirmed"));
    }

    #[test]
    fn rejects_non_send_subtype() {
        for sub in ["receive", "change", "epoch", ""] {
            let mut info = good_info();
            info.subtype = sub.to_string();
            assert!(!evaluate_receipt(&info, &expected(), NOW).valid, "{sub:?}");
        }
    }

    #[test]
    fn rejects_wrong_recipient_and_wrong_payer() {
        let mut info = good_info();
        info.link_as_account = PAYER.to_string();
        assert!(!evaluate_receipt(&info, &expected(), NOW).valid);

        let mut info = good_info();
        info.block_account = RECIPIENT.to_string();
        assert!(!evaluate_receipt(&info, &expected(), NOW).valid);
    }

    #[test]
    fn rejects_insufficient_or_malformed_amount() {
        let mut info = good_info();
        info.amount = "2099999999999999999999999999".to_string();
        let r = evaluate_receipt(&info, &expected(), NOW);
        assert!(!r.valid);
        assert!(r.reason.unwrap().contains("insufficient"));

        for bad in ["", "-1", "1.5", "abc", "1e30"] {
            let mut info = good_info();
            info.amount = bad.to_string();
            assert!(!evaluate_receipt(&info, &expected(), NOW).valid, "{bad:?}");
        }
    }

    #[test]
    fn rejects_stale_future_and_unknown_timestamps() {
        let mut info = good_info();
        info.local_timestamp = (NOW - NANO_RECEIPT_MAX_AGE_SECS - 1).to_string();
        let r = evaluate_receipt(&info, &expected(), NOW);
        assert!(!r.valid);
        assert!(r.reason.unwrap().contains("stale"));

        // Exactly at the limit is still fresh.
        let mut info = good_info();
        info.local_timestamp = (NOW - NANO_RECEIPT_MAX_AGE_SECS).to_string();
        assert!(evaluate_receipt(&info, &expected(), NOW).valid);

        let mut info = good_info();
        info.local_timestamp = (NOW + FUTURE_SKEW_SECS + 1).to_string();
        assert!(!evaluate_receipt(&info, &expected(), NOW).valid);

        for bad in ["0", "", "x"] {
            let mut info = good_info();
            info.local_timestamp = bad.to_string();
            assert!(!evaluate_receipt(&info, &expected(), NOW).valid, "{bad:?}");
        }
    }

    #[test]
    fn parses_a_block_info_response() {
        let v = serde_json::json!({
            "block_account": PAYER,
            "amount": "2100000000000000000000000000",
            "balance": "5000000000000000000000000000000",
            "height": "58",
            "local_timestamp": "1700000090",
            "successor": "0000000000000000000000000000000000000000000000000000000000000000",
            "confirmed": "true",
            "contents": {
                "type": "state",
                "account": PAYER,
                "previous": "CE898C131AAEE25E05362F247760F8A3ACF34A9796A5AE0D9204E86B0637965E",
                "representative": "nano_1stofnrxuz3cai7ze75o174bpm7scwj9jn3nxsn8ntzg784jf1gzn1jjdkou",
                "balance": "5000000000000000000000000000000",
                "link": "0000000000000000000000000000000000000000000000000000000000000000",
                "link_as_account": RECIPIENT,
                "signature": "82D41BC16F313E4B2243D14DFFA2FB04679C540C2095FEE7EAE0F2F26880AD56DD48D87A7CC5DD760C5B2D76EE2C205506AA557BF00B60D8DEE312EC7343A501",
                "work": "8a142e07a10996d5"
            },
            "subtype": "send"
        });
        let info = BlockInfo::from_rpc_response(&v).unwrap();
        assert_eq!(info, good_info());
    }

    /// A real (2019) legacy `send` block as a public node renders it today:
    /// `contents.type == "send"` with `destination`, NO `subtype`, NO
    /// `link_as_account`. It parses (so the failure is attributable) and is
    /// then rejected by `evaluate_receipt` — never accepted as a payment.
    #[test]
    fn legacy_pre_state_block_is_rejected_not_an_rpc_error() {
        let v = serde_json::json!({
            "block_account": RECIPIENT,
            "amount": "205676479000000000000000000000000000000",
            "balance": "325586539664609129644855132177",
            "height": "42",
            "local_timestamp": (NOW - 1).to_string(),
            "successor": "6875C0DBFE5C44D8F8CFF431BC69ED5587C68F89F0663F2BC1FBBFCB46DC5989",
            "confirmed": "true",
            "contents": {
                "type": "send",
                "previous": "4270F4FB3A820FE81827065F967A9589DF5CA860443F812D21ECE964AC359E05",
                "destination": "nano_1111111111111111111111111111111111111111111111111111hifc8npp",
                "balance": "000000041C06DF91D202B70A40000011",
                "work": "7202df8a7c380578",
                "signature": "047115CB577AC78F5C66AD79BBF47540DE97A441456004190F22025FE4255285F57010D962601AE64C266C98FA22973DD95AC62309634940B727AC69F0C86D03"
            }
        });
        let info = BlockInfo::from_rpc_response(&v).unwrap();
        assert_eq!(info.subtype, "");
        assert_eq!(info.link_as_account, "");
        let expected = ExpectedReceipt {
            recipient: "nano_1111111111111111111111111111111111111111111111111111hifc8npp",
            payer: RECIPIENT,
            min_raw: 1,
            max_age_secs: NANO_RECEIPT_MAX_AGE_SECS,
        };
        let r = evaluate_receipt(&info, &expected, NOW);
        assert!(!r.valid);
        assert!(r.reason.unwrap().contains("subtype"));
    }

    #[test]
    fn node_error_and_missing_fields_fail_closed() {
        let err = BlockInfo::from_rpc_response(&serde_json::json!({"error": "Block not found"}))
            .unwrap_err();
        assert!(matches!(err, Error::InvalidTransaction(m) if m.contains("Block not found")));

        let err =
            BlockInfo::from_rpc_response(&serde_json::json!({"confirmed": "true"})).unwrap_err();
        assert!(matches!(err, Error::Rpc(_)));

        // Legacy (non-json_block) form carries `contents` as a string — rejected.
        let err = BlockInfo::from_rpc_response(&serde_json::json!({
            "block_account": PAYER, "amount": "1", "confirmed": "true",
            "subtype": "send", "local_timestamp": "1", "contents": "{...}"
        }))
        .unwrap_err();
        assert!(matches!(err, Error::Rpc(_)));
    }

    fn client() -> Client {
        Client::new()
    }

    #[test]
    fn constructor_validates_config() {
        assert!(NanoVerifier::new("http://127.0.0.1:7076", RECIPIENT, client()).is_ok());
        assert!(matches!(
            NanoVerifier::new("", RECIPIENT, client()),
            Err(Error::InvalidConfig(_))
        ));
        assert!(matches!(
            NanoVerifier::new("http://127.0.0.1:7076", "nano_bad", client()),
            Err(Error::InvalidConfig(_))
        ));
        let v = NanoVerifier::new("http://127.0.0.1:7076", RECIPIENT, client()).unwrap();
        assert_eq!(v.network(), NANO_NETWORK);
        assert_eq!(v.scheme(), "exact");
        assert_eq!(v.recipient_account(), RECIPIENT);
    }

    fn payload(payload: PayloadData) -> PaymentPayload {
        PaymentPayload {
            x402_version: 2,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: PaymentAccept {
                scheme: "exact".to_string(),
                network: NANO_NETWORK.to_string(),
                amount: "2100000000000000000000000000".to_string(),
                asset: NANO_ASSET.to_string(),
                pay_to: RECIPIENT.to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload,
        }
    }

    fn nano_payload() -> PayloadData {
        PayloadData::Nano(NanoPayload {
            block_hash: HASH.to_string(),
            account: PAYER.to_string(),
        })
    }

    /// Every structural rejection happens BEFORE any RPC call — the verifier
    /// points at an unroutable address, so reaching the network would hang
    /// (and the 10 s timeout would make the test slow), not pass.
    #[tokio::test]
    async fn structural_rejections_never_reach_the_node() {
        let v = NanoVerifier::new("http://192.0.2.1:7076", RECIPIENT, client()).unwrap();

        let mut p = payload(nano_payload());
        p.accepted.network = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string();
        assert!(matches!(
            v.verify_payment(&p).await,
            Err(Error::UnsupportedNetwork(_))
        ));

        let mut p = payload(nano_payload());
        p.accepted.scheme = "escrow".to_string();
        assert!(matches!(
            v.verify_payment(&p).await,
            Err(Error::PayloadMismatch(_))
        ));

        let mut p = payload(nano_payload());
        p.accepted.asset = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string();
        assert!(matches!(
            v.verify_payment(&p).await,
            Err(Error::WrongAsset { .. })
        ));

        let mut p = payload(nano_payload());
        p.accepted.pay_to = PAYER.to_string();
        assert!(matches!(
            v.verify_payment(&p).await,
            Err(Error::WrongRecipient { .. })
        ));

        let p = payload(PayloadData::Direct(crate::types::SolanaPayload {
            transaction: "dGVzdA==".to_string(),
        }));
        assert!(matches!(
            v.verify_payment(&p).await,
            Err(Error::PayloadMismatch(_))
        ));

        let p = payload(PayloadData::Nano(NanoPayload {
            block_hash: HASH.to_ascii_lowercase(),
            account: PAYER.to_string(),
        }));
        assert!(matches!(
            v.verify_payment(&p).await,
            Err(Error::InvalidEncoding(_))
        ));

        let p = payload(PayloadData::Nano(NanoPayload {
            block_hash: HASH.to_string(),
            account: "nano_short".to_string(),
        }));
        assert!(matches!(
            v.verify_payment(&p).await,
            Err(Error::InvalidEncoding(_))
        ));

        let mut p = payload(nano_payload());
        p.accepted.amount = "1.5".to_string();
        assert!(matches!(
            v.verify_payment(&p).await,
            Err(Error::InvalidTransaction(_))
        ));

        // Vendor override with a malformed recipient fails before the network.
        let p = payload(nano_payload());
        assert!(matches!(
            v.verify_payment_to(&p, "not-an-address").await,
            Err(Error::WrongRecipient { .. })
        ));
    }

    #[tokio::test]
    async fn settle_is_a_no_op_reporting_the_block_hash() {
        let v = NanoVerifier::new("http://192.0.2.1:7076", RECIPIENT, client()).unwrap();
        let s = v.settle_payment(&payload(nano_payload())).await.unwrap();
        assert!(s.success);
        assert_eq!(s.tx_signature.as_deref(), Some(HASH));
        assert_eq!(s.network, NANO_NETWORK);

        let p = payload(PayloadData::Direct(crate::types::SolanaPayload {
            transaction: "dGVzdA==".to_string(),
        }));
        assert!(matches!(
            v.settle_payment(&p).await,
            Err(Error::PayloadMismatch(_))
        ));
    }
}

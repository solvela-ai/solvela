//! Opt-in Nano exact settlement through a trusted mainnet node.
//!
//! A block hash alone is public, replayable data, so it is NOT a credential.
//! Clients provide a signed, unpublished state send block. Settlement requires
//! a successful synchronous `process` (never `force`/Old-block success) and
//! a confirmed send receipt. All gateway replicas MUST use the same node.
//! Nano must be settled before delivery: standalone verification fails closed
//! because signature/work validation requires the node's process operation.
//! Node RPC reference: https://docs.nano.org/commands/rpc-protocol/

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::traits::{Error, PaymentVerifier};
use crate::types::{
    PayloadData, PaymentPayload, SettlementResult, VerificationResult, NANO_ASSET, NANO_NETWORK,
};

/// Signed state block, transported as base64 JSON in `payload.transaction`.
/// The legacy `SolanaPayload` name is retained for source compatibility.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NanoBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub account: String,
    pub previous: String,
    pub representative: String,
    pub balance: String,
    pub link: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_as_account: Option<String>,
    pub signature: String,
    pub work: String,
}

impl NanoBlock {
    pub fn decode(transaction: &str) -> Result<Self, Error> {
        if transaction.len() > 16_384 {
            return Err(invalid("Nano block exceeds size limit"));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(transaction)
            .map_err(|_| invalid("Nano block must be base64 JSON"))?;
        let block: Self = serde_json::from_slice(&bytes)
            .map_err(|_| invalid("malformed Nano state block"))?;
        if block.block_type != "state"
            || !is_hex(&block.previous, 64)
            || block.previous.bytes().all(|b| b == b'0')
            || !is_hex(&block.link, 64)
            || !is_hex(&block.signature, 128)
            || !is_hex(&block.work, 16)
            || !is_nano_address(&block.account)
            || !is_nano_address(&block.representative)
        {
            return Err(invalid("malformed Nano state send block"));
        }
        raw_amount(&block.balance, false)?;
        Ok(block)
    }
}

/// Mainnet node verifier. The operator supplies a trusted RPC URL, never the
/// payer; use a single node across replicas so `process` is a ledger-backed
/// atomic replay gate even after Redis eviction or a gateway restart.
pub struct NanoVerifier {
    rpc_url: String,
    recipient: String,
    http_client: reqwest::Client,
}

impl NanoVerifier {
    pub fn new(
        rpc_url: &str,
        recipient: &str,
        http_client: reqwest::Client,
    ) -> Result<Self, Error> {
        let url = reqwest::Url::parse(rpc_url).map_err(|_| invalid("invalid Nano RPC URL"))?;
        if !matches!(url.scheme(), "http" | "https") || !is_nano_address(recipient) {
            return Err(invalid("invalid Nano RPC URL or recipient"));
        }
        Ok(Self { rpc_url: rpc_url.into(), recipient: recipient.into(), http_client })
    }

    // No automatic retries for process: a timeout may already have moved funds.
    // Strip reqwest errors: they may contain authenticated RPC URLs.
    async fn rpc(&self, body: Value) -> Result<Value, Error> {
        let response = self.http_client.post(&self.rpc_url)
            .timeout(Duration::from_secs(5)).json(&body).send().await
            .map_err(|_| Error::Rpc("Nano RPC unavailable".into()))?
            .error_for_status().map_err(|_| Error::Rpc("Nano RPC HTTP failure".into()))?;
        let value: Value = response.json().await
            .map_err(|_| Error::Rpc("invalid Nano RPC response".into()))?;
        if value.get("error").is_some() {
            // In particular, NEVER accept "Old block" as settlement success.
            return Err(Error::Rpc("Nano node rejected request".into()));
        }
        Ok(value)
    }

    async fn check(&self, payload: &PaymentPayload) -> Result<(NanoBlock, String, u128), Error> {
        let accepted = &payload.accepted;
        if payload.x402_version != 2 || accepted.network != NANO_NETWORK
            || accepted.scheme != "exact" || accepted.asset != NANO_ASSET
            || accepted.pay_to != self.recipient || accepted.escrow_program_id.is_some()
            || accepted.max_timeout_seconds == 0 || accepted.max_timeout_seconds > 300
        {
            return Err(invalid("unsupported Nano payment requirements"));
        }
        let required = raw_amount(&accepted.amount, true)?;
        let PayloadData::Direct(direct) = &payload.payload else {
            return Err(Error::PayloadMismatch("Nano requires a signed state block".into()));
        };
        let block = NanoBlock::decode(&direct.transaction)?;
        // account_key validates the destination's checksum at the node. Use the
        // signed link, NEVER the optional unsigned link_as_account annotation.
        let key = self.rpc(json!({"action": "account_key", "account": self.recipient})).await?;
        let key = key["key"].as_str().filter(|v| is_hex(v, 64))
            .ok_or_else(|| invalid("invalid Nano recipient key response"))?.to_owned();
        if !block.link.eq_ignore_ascii_case(&key) {
            return Err(invalid("wrong Nano recipient"));
        }
        let previous = self.rpc(json!({
            "action": "block_info", "json_block": true, "hash": block.previous
        })).await?;
        if previous["confirmed"] != "true" || previous["block_account"] != block.account {
            return Err(invalid("Nano predecessor must be confirmed and belong to payer"));
        }
        let before = raw_amount(previous["balance"].as_str()
            .ok_or_else(|| invalid("missing Nano predecessor balance"))?, false)?;
        let after = raw_amount(&block.balance, false)?;
        let sent = before.checked_sub(after).filter(|v| *v > 0)
            .ok_or_else(|| invalid("Nano block is not a send"))?;
        if sent < required {
            return Err(invalid("insufficient Nano payment"));
        }
        Ok((block, key, required))
    }
}

#[async_trait]
impl PaymentVerifier for NanoVerifier {
    fn network(&self) -> &str { NANO_NETWORK }
    fn scheme(&self) -> &str { "exact" }

    /// Nano has no simulation RPC. Never report an unvalidated signature as
    /// valid: callers must use the atomic pre-delivery settlement operation.
    async fn verify_payment(&self, _payload: &PaymentPayload) -> Result<VerificationResult, Error> {
        Err(Error::PayloadMismatch("Nano requires verify_and_settle before delivery".into()))
    }

    async fn verify_and_settle_payment(&self, payload: &PaymentPayload) -> Result<SettlementResult, Error> {
        self.settle_payment(payload).await
    }

    async fn settle_payment(&self, payload: &PaymentPayload) -> Result<SettlementResult, Error> {
        let (block, key, required) = self.check(payload).await?;
        let processed = self.rpc(json!({
            "action": "process", "json_block": true, "subtype": "send", "block": block
        })).await?;
        let hash = processed["hash"].as_str().filter(|v| is_hex(v, 64))
            .ok_or_else(|| invalid("missing Nano process hash"))?.to_ascii_uppercase();

        // Publish is not finality. Wait only within the advertised budget, with
        // a hard 10-second cap; missing/unknown confirmation fails closed.
        let wait = async {
            loop {
                let receipt = self.rpc(json!({
                    "action": "block_info", "json_block": true, "hash": hash
                })).await?;
                match receipt["confirmed"].as_str() {
                    Some("false") => tokio::time::sleep(Duration::from_millis(200)).await,
                    Some("true") => {
                        if receipt["subtype"] != "send"
                            || receipt["block_account"] != block.account
                            || receipt["contents"]["type"] != "state"
                            || receipt["contents"]["account"] != block.account
                            || !receipt["contents"]["link"].as_str()
                                .is_some_and(|v| v.eq_ignore_ascii_case(&key))
                            || raw_amount(receipt["amount"].as_str()
                                .ok_or_else(|| invalid("missing Nano receipt amount"))?, true)? < required
                        {
                            return Err(invalid("Nano receipt does not match payment"));
                        }
                        return Ok(());
                    }
                    _ => return Err(invalid("missing Nano confirmation status")),
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(payload.accepted.max_timeout_seconds.min(10)), wait)
            .await.map_err(|_| Error::Timeout)??;
        Ok(SettlementResult {
            success: true, tx_signature: Some(hash), network: NANO_NETWORK.into(),
            error: None, verified_amount: None, failure_kind: None,
        })
    }
}

/// Nano uses 30 decimals; amounts cannot pass through the USDC u64 path.
pub fn raw_amount(value: &str, positive: bool) -> Result<u128, Error> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid("Nano amount must be decimal raw units"));
    }
    let amount: u128 = value.parse().map_err(|_| invalid("Nano raw amount overflow"))?;
    if positive && amount == 0 {
        return Err(invalid("Nano payment amount must be positive"));
    }
    Ok(amount)
}

/// Shape check only; the configured node validates the checksum.
pub fn is_nano_address(value: &str) -> bool {
    value.strip_prefix("nano_").is_some_and(|v| {
        v.len() == 60 && matches!(v.as_bytes()[0], b'1' | b'3')
            && v.bytes().all(|b| b"13456789abcdefghijkmnopqrstuwxyz".contains(&b))
    })
}

fn is_hex(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn invalid(message: &str) -> Error { Error::InvalidTransaction(message.into()) }

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use crate::facilitator::Facilitator;
    use crate::types::{PaymentAccept, Resource, SolanaPayload};

    const ACCOUNT: &str = "nano_1ipx847tk8o46pwxt5qjdbncjqcbwcc1rrmqnkztrfjy5k7z4imsrata9est";
    const DESTINATION: &str = "nano_18gmu6engqhgtjnppqam181o5nfhj4sdtgyhy36dan3jr9spt84rzwmktafc";
    const RAW: &str = "1000000000000000000000000000000";

    fn payload() -> PaymentPayload {
        let block = serde_json::json!({
            "type": "state", "account": ACCOUNT, "previous": "A".repeat(64),
            "representative": ACCOUNT, "balance": RAW, "link": "B".repeat(64),
            "signature": "C".repeat(128), "work": "0123456789abcdef"
        });
        PaymentPayload {
            x402_version: 2,
            resource: Resource { url: "/v1/chat/completions".into(), method: "POST".into() },
            accepted: PaymentAccept {
                scheme: "exact".into(), network: NANO_NETWORK.into(), amount: RAW.into(),
                asset: NANO_ASSET.into(), pay_to: DESTINATION.into(), max_timeout_seconds: 1,
                escrow_program_id: None,
            },
            payload: PayloadData::Direct(SolanaPayload {
                transaction: base64::engine::general_purpose::STANDARD.encode(block.to_string()),
            }),
        }
    }

    fn previous() -> serde_json::Value {
        serde_json::json!({ "confirmed": "true", "block_account": ACCOUNT,
            "balance": "2000000000000000000000000000000" })
    }

    fn receipt() -> serde_json::Value {
        serde_json::json!({ "confirmed": "true", "subtype": "send", "amount": RAW,
            "block_account": ACCOUNT, "contents": {
                "type": "state", "account": ACCOUNT, "link": "B".repeat(64)
            } })
    }

    // A scripted local RPC: unexpected calls/order/body fail the test, and no
    // public node or real funds are involved.
    async fn rpc(
        script: Vec<(&'static str, serde_json::Value)>,
    ) -> (NanoVerifier, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            for (action, response) in script {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0u8; 4096];
                let body = loop {
                    let n = stream.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let len: usize = headers.lines().find_map(|line| {
                            line.to_ascii_lowercase().strip_prefix("content-length:")
                                .map(|s| s.trim().parse().unwrap())
                        }).unwrap();
                        if bytes.len() >= end + 4 + len {
                            break serde_json::from_slice::<serde_json::Value>(&bytes[end + 4..end + 4 + len]).unwrap();
                        }
                    }
                };
                assert_eq!(body["action"], action);
                if action == "process" {
                    assert_eq!(body["subtype"], "send");
                    assert_eq!(body["block"]["link"], "B".repeat(64));
                    assert!(body.get("force").is_none());
                }
                if action == "block_info" {
                    assert!(body["hash"].as_str().unwrap().bytes().all(|b| b.is_ascii_hexdigit()));
                }
                let response = response.to_string();
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).as_bytes()).await.unwrap();
            }
        });
        (NanoVerifier::new(&url, DESTINATION, reqwest::Client::new()).unwrap(), task)
    }

    fn checks() -> Vec<(&'static str, serde_json::Value)> {
        vec![("account_key", serde_json::json!({"key": "B".repeat(64)})),
             ("block_info", previous())]
    }

    #[tokio::test]
    async fn standalone_verification_cannot_authorize_deferred_nano_settlement() {
        let (verifier, server) = rpc(vec![]).await;
        assert!(verifier.verify_payment(&payload()).await.is_err());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn waits_for_confirmation_after_publish() {
        let mut unconfirmed = receipt();
        unconfirmed["confirmed"] = "false".into();
        let mut script = checks();
        script.push(("process", serde_json::json!({"hash": "D".repeat(64)})));
        script.push(("block_info", unconfirmed));
        script.push(("block_info", receipt()));
        let (verifier, server) = rpc(script).await;
        assert!(verifier.settle_payment(&payload()).await.unwrap().success);
        server.await.unwrap();
    }

    #[test]
    fn raw_units_are_checked_u128_and_hash_only_proofs_are_rejected() {
        assert_eq!(raw_amount(RAW, true).unwrap(), 10u128.pow(30));
        for invalid in ["", "+1", "-1", "1.5", "1e30", "0", "340282366920938463463374607431768211456"] {
            assert!(raw_amount(invalid, true).is_err());
        }
        assert!(NanoBlock::decode(&"D".repeat(64)).is_err());
        assert!(NanoBlock::decode("dGVzdA==").is_err());
    }

    #[tokio::test]
    async fn confirmed_send_settles_via_facilitator_without_u64_truncation() {
        let mut script = checks();
        script.push(("process", serde_json::json!({"hash": "D".repeat(64)})));
        script.push(("block_info", receipt()));
        let (verifier, server) = rpc(script).await;
        let f = Facilitator::new(vec![Arc::new(verifier)]);
        let result = f.verify_and_settle(&payload()).await.unwrap();
        assert!(result.success);
        assert_eq!(result.network, NANO_NETWORK);
        assert_eq!(result.tx_signature.as_deref(), Some("D".repeat(64).as_str()));
        assert_eq!(result.verified_amount, None); // raw units must never be cast to u64
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_wrong_destination_underpayment_and_receive_before_publish() {
        for (key, balance) in [
            ("E".repeat(64), RAW.to_owned()),
            ("B".repeat(64), "1500000000000000000000000000000".to_owned()),
            ("B".repeat(64), "3000000000000000000000000000000".to_owned()),
        ] {
            let script = if key != "B".repeat(64) {
                vec![("account_key", serde_json::json!({"key": key}))]
            } else { checks() };
            let (verifier, server) = rpc(script).await;
            let mut p = payload();
            let PayloadData::Direct(ref mut direct) = p.payload else { unreachable!() };
            let mut block: serde_json::Value = serde_json::from_slice(&base64::engine::general_purpose::STANDARD.decode(&direct.transaction).unwrap()).unwrap();
            block["balance"] = balance.into();
            direct.transaction = base64::engine::general_purpose::STANDARD.encode(block.to_string());
            assert!(verifier.settle_payment(&p).await.is_err());
            server.await.unwrap();
        }
    }


    #[tokio::test]
    async fn unconfirmed_or_wrong_account_predecessors_are_rejected_before_publish() {
        for field in ["confirmed", "block_account", "balance"] {
            let mut p = previous();
            match field {
                "confirmed" => p["confirmed"] = "false".into(),
                "block_account" => p["block_account"] = DESTINATION.into(),
                "balance" => p["balance"] = "not-an-amount".into(),
                _ => unreachable!(),
            }
            let (verifier, server) = rpc(vec![
                ("account_key", serde_json::json!({"key": "B".repeat(64)})),
                ("block_info", p),
            ]).await;
            assert!(verifier.settle_payment(&payload()).await.is_err());
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn reencoded_payment_cannot_bypass_node_replay_rejection() {
        let mut script = checks();
        script.push(("process", serde_json::json!({"hash": "D".repeat(64)})));
        script.push(("block_info", receipt()));
        script.extend(checks());
        script.push(("process", serde_json::json!({"error": "Old block"})));
        let (verifier, server) = rpc(script).await;
        let mut p = payload();
        assert!(verifier.settle_payment(&p).await.unwrap().success);
        let PayloadData::Direct(ref mut direct) = p.payload else { unreachable!() };
        let block: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::STANDARD.decode(&direct.transaction).unwrap()).unwrap();
        direct.transaction = base64::engine::general_purpose::STANDARD.encode(
            serde_json::to_string_pretty(&block).unwrap());
        assert!(verifier.settle_payment(&p).await.is_err());
        server.await.unwrap();
    }
    #[tokio::test]
    async fn old_blocks_and_invalid_signatures_never_settle() {
        for error in ["Old block", "Bad signature", "Fork"] {
            let mut script = checks();
            script.push(("process", serde_json::json!({"error": error})));
            let (verifier, server) = rpc(script).await;
            assert!(verifier.settle_payment(&payload()).await.is_err());
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn receipt_requires_confirmed_send_with_matching_amount_and_link() {
        for field in ["confirmed", "subtype", "amount", "link", "account"] {
            let mut r = receipt();
            match field {
                "confirmed" => r["confirmed"] = serde_json::Value::Null,
                "subtype" => r["subtype"] = "receive".into(),
                "amount" => r["amount"] = "1".into(),
                "link" => r["contents"]["link"] = "E".repeat(64).into(),
                "account" => r["block_account"] = DESTINATION.into(),
                _ => unreachable!(),
            }
            let mut script = checks();
            script.push(("process", serde_json::json!({"hash": "D".repeat(64)})));
            script.push(("block_info", r));
            let (verifier, server) = rpc(script).await;
            assert!(verifier.settle_payment(&payload()).await.is_err(), "{field}");
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn rejects_wrong_network_scheme_asset_version_and_zero_amount_without_rpc() {
        let (verifier, server) = rpc(vec![]).await;
        for field in ["network", "scheme", "asset", "version", "amount", "recipient"] {
            let mut p = payload();
            match field {
                "network" => p.accepted.network = "solana:mainnet".into(),
                "scheme" => p.accepted.scheme = "escrow".into(),
                "asset" => p.accepted.asset = "USDC".into(),
                "version" => p.x402_version = 1,
                "amount" => p.accepted.amount = "0".into(),
                "recipient" => p.accepted.pay_to = ACCOUNT.into(),
                _ => unreachable!(),
            }
            assert!(verifier.settle_payment(&p).await.is_err(), "{field}");
        }
        server.await.unwrap();
    }
}

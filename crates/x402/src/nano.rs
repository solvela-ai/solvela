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
//! 2. The client presents `{ block_hash, account, signature }` as the x402
//!    payload. `signature` is the account key's Ed25519-BLAKE2b signature over
//!    the **BLAKE2b-256 digest** of
//!    [`solvela_protocol::nano::receipt_signing_message`] — the block hash,
//!    quoted raw amount, `pay_to` and request resource. Signing a 32-byte hash
//!    (rather than the variable-length message) is Nano's native signing shape,
//!    so a standard Nano signer can produce it (`signBlock({hash})` in the JS
//!    library, or a node's hash-signing RPC) without Solvela-specific crypto.
//!    It proves the presenter holds the paying account's key (a published send
//!    block is public, so the hash alone would be a bearer token).
//! 3. [`NanoVerifier::verify_payment`] checks the signature against the public
//!    key decoded from `account` BEFORE any network call, then asks a Nano
//!    node for `block_info` and checks: confirmed, `subtype == "send"`, paid
//!    to the advertised `pay_to`, sent by `account`, `amount` ≥ the advertised
//!    raw amount, and that the node first saw the block within
//!    [`NANO_RECEIPT_MAX_AGE_SECS`].
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
//! A block hash never expires on-chain, and a resyncing node can restamp
//! `local_timestamp` on an old block, so freshness alone can never bound
//! replay. The gateway therefore records each redeemed hash in a durable,
//! non-expiring store (PostgreSQL primary key, or Redis `SET NX` when no
//! database is configured) BEFORE the provider call, and refuses a hash it has
//! seen before. This verifier is consulted first; the caller only marks the
//! hash spent after it returns valid, so a receipt that fails verification
//! (not yet confirmed, wrong amount, bad signature) is never burned.
//! [`NANO_RECEIPT_MAX_AGE_SECS`] remains defence-in-depth against a receipt
//! presented long after the payment it references. The hash must be the
//! canonical uppercase hex spelling so a used receipt has exactly one store
//! key.
//!
//! # Trust
//!
//! The only trusted party is the configured Nano node (`rpc_url`) — it is the
//! source of `confirmed`, `amount`, `link_as_account`, and `local_timestamp`.
//! Point it at a node you run or one you trust; a lying node could attest a
//! payment that never happened. This is the same trust the Solana path places
//! in its RPC endpoint. The RPC URL is redacted to `scheme://host[:port]` in
//! logs and `Debug` output (hosted nodes embed API keys in the path), redirects
//! are disabled, and response bodies are capped. Only `nano_` addresses are
//! accepted; legacy `xrb_` spellings are rejected.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use blake2::digest::{Update, VariableOutput};
use blake2::{Blake2b512, Blake2bVar, Digest};
use curve25519_dalek::edwards::{CompressedEdwardsY, EdwardsPoint};
use curve25519_dalek::scalar::Scalar;
use reqwest::redirect::Policy;
use reqwest::Client;
use tracing::{debug, info, warn};

use solvela_protocol::nano::{
    is_well_formed_block_hash, is_well_formed_nano_address, is_well_formed_nano_signature,
    receipt_signing_message, NANO_ADDRESS_LEN, NANO_ASSET, NANO_NETWORK, NANO_RECEIPT_MAX_AGE_SECS,
};

use crate::traits::{Error, PaymentVerifier};
use crate::types::{PayloadData, PaymentPayload, SettlementResult, VerificationResult};

/// Per-request timeout for the Nano node RPC call.
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Hard cap on the node's response body. A `block_info` response is a few
/// hundred bytes; anything larger is a misconfigured or hostile endpoint, not
/// a block lookup. The cap is enforced while streaming so a huge body cannot
/// exhaust gateway memory.
const MAX_RPC_RESPONSE_BYTES: usize = 1024 * 1024;

/// Tolerated clock skew between the node's `local_timestamp` and this host:
/// a block the node stamps a few seconds "in the future" is still fresh.
const FUTURE_SKEW_SECS: u64 = 5;

/// Nano's base32 alphabet (no `0`, `2`, `l`, `v`).
const NANO_BASE32_ALPHABET: &[u8] = b"13456789abcdefghijkmnopqrstuwxyz";

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

/// Decode a `nano_` account address to its 32-byte Ed25519 public key.
///
/// Layout: `nano_` + 52 base32 chars (a zero 4-bit pad then the 256-bit
/// public key) + 8 base32 chars (the 40-bit BLAKE2b-5 checksum of the public
/// key, byte-reversed). The checksum is verified here, so a mistyped or
/// hostile address cannot be treated as somebody else's key.
///
/// The legacy `xrb_` prefix is rejected: one account has exactly one accepted
/// wire spelling on Solvela's surfaces (fail closed rather than normalise).
pub fn decode_nano_account(addr: &str) -> Result<[u8; 32], Error> {
    let invalid =
        || Error::InvalidEncoding("nano account is not a valid nano_ address".to_string());
    let body = addr.strip_prefix("nano_").ok_or_else(invalid)?;
    if addr.len() != NANO_ADDRESS_LEN || body.len() != 60 {
        return Err(invalid());
    }
    let (key_chars, sum_chars) = body.split_at(52);

    let key_bits = decode_base32_bits(key_chars).ok_or_else(invalid)?;
    if key_bits.len() != 260 || key_bits[..4].iter().any(|&b| b != 0) {
        return Err(invalid());
    }
    let key_bytes = bits_to_bytes(&key_bits[4..]).ok_or_else(invalid)?;
    let public_key: [u8; 32] = key_bytes.try_into().map_err(|_| invalid())?;

    let sum_bits = decode_base32_bits(sum_chars).ok_or_else(invalid)?;
    let sum_bytes = bits_to_bytes(&sum_bits).ok_or_else(invalid)?;
    let mut expected = blake2b_var(&public_key, 5);
    expected.reverse();
    if sum_bytes != expected {
        return Err(invalid());
    }
    Ok(public_key)
}

/// Decode a Nano base32 string into its bit sequence (5 bits per character,
/// most-significant bit first).
fn decode_base32_bits(s: &str) -> Option<Vec<u8>> {
    let mut bits = Vec::with_capacity(s.len() * 5);
    for c in s.bytes() {
        let value = NANO_BASE32_ALPHABET.iter().position(|&a| a == c)? as u8;
        for shift in (0..5).rev() {
            bits.push((value >> shift) & 1);
        }
    }
    Some(bits)
}

/// Pack a bit sequence into bytes; `None` unless it is a whole number of bytes.
fn bits_to_bytes(bits: &[u8]) -> Option<Vec<u8>> {
    if !bits.len().is_multiple_of(8) {
        return None;
    }
    Some(
        bits.as_chunks::<8>()
            .0
            .iter()
            .map(|chunk| chunk.iter().fold(0u8, |acc, &bit| (acc << 1) | (bit & 1)))
            .collect(),
    )
}

/// BLAKE2b with a caller-chosen digest length (Nano's address checksum uses 5).
fn blake2b_var(input: &[u8], out_len: usize) -> Vec<u8> {
    // out_len is a compile-time constant at both call sites (5); fail closed
    // rather than panic if that ever changes.
    let Ok(mut hasher) = Blake2bVar::new(out_len) else {
        return Vec::new();
    };
    hasher.update(input);
    let mut out = vec![0u8; out_len];
    if hasher.finalize_variable(&mut out).is_err() {
        return Vec::new();
    }
    out
}

/// The 32-byte object a payer signs: the BLAKE2b-256 digest of
/// [`receipt_signing_message`].
///
/// Nano's signing primitive signs a 32-byte hash, so signing this digest lets
/// an ordinary Nano signer produce a receipt signature — the JS
/// `nanocurrency` library's `signBlock({hash})` accepts it directly, as does a
/// node's `sign` RPC where hash signing is enabled. The domain tag inside the
/// preimage keeps this digest distinct from any real block hash.
///
/// Returns a zero digest if the (infallible, fixed-length) BLAKE2b-256
/// construction ever failed; no signature verifies against zeros, so the
/// caller fails closed rather than panicking.
pub fn receipt_signing_hash(
    block_hash: &str,
    amount_raw: &str,
    pay_to: &str,
    resource_method: &str,
    resource_url: &str,
) -> [u8; 32] {
    let message = receipt_signing_message(
        block_hash,
        amount_raw,
        pay_to,
        resource_method,
        resource_url,
    );
    let digest = blake2b_var(&message, 32);
    let mut hash = [0u8; 32];
    let len = digest.len().min(hash.len());
    hash[..len].copy_from_slice(&digest[..len]);
    hash
}

/// Verify an Ed25519 signature that uses **BLAKE2b-512** as its hash — the
/// variant Nano uses for account signatures.
///
/// This is the RFC 8032 verification equation with `H = BLAKE2b-512`:
/// `[S]B = R + [H(R || A || M)]A`. `ed25519-dalek` hardcodes SHA-512, so the
/// curve operations come from `curve25519-dalek` directly. Verification is
/// deliberately strict: R and A must decompress and be non-small-order, and S
/// must be canonical (`< L`), so a signature cannot be re-spelled into a
/// second accepted encoding.
pub fn verify_nano_signature(public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    let Some(public_point) = CompressedEdwardsY(*public_key).decompress() else {
        return false;
    };
    if public_point.is_small_order() {
        return false;
    }
    let mut r_bytes = [0u8; 32];
    r_bytes.copy_from_slice(&signature[..32]);
    let Some(r_point) = CompressedEdwardsY(r_bytes).decompress() else {
        return false;
    };
    if r_point.is_small_order() {
        return false;
    }
    let mut s_bytes = [0u8; 32];
    s_bytes.copy_from_slice(&signature[32..]);
    let Some(s_scalar) = Option::<Scalar>::from(Scalar::from_canonical_bytes(s_bytes)) else {
        return false;
    };

    let mut hasher = Blake2b512::new();
    Digest::update(&mut hasher, r_bytes);
    Digest::update(&mut hasher, public_key);
    Digest::update(&mut hasher, message);
    let digest = hasher.finalize();
    let mut wide = [0u8; 64];
    wide.copy_from_slice(&digest);
    let k = Scalar::from_bytes_mod_order_wide(&wide);

    // [S]B = R + [k]A  <=>  [k](-A) + [S]B == R
    let check = EdwardsPoint::vartime_double_scalar_mul_basepoint(&k, &(-public_point), &s_scalar);
    check == r_point
}

/// `exact`-scheme verifier for `nano:mainnet` receipts.
pub struct NanoVerifier {
    rpc_url: String,
    recipient_account: String,
    http_client: Client,
}

/// Host-only rendering of a Nano RPC URL.
///
/// Hosted Nano nodes commonly embed an API key in the path or query string
/// (e.g. `https://rpc.example/api-key`), so the full URL must never reach a
/// log line or `Debug` output. Only `scheme://host[:port]` is returned.
pub fn redacted_rpc_url(rpc_url: &str) -> String {
    match reqwest::Url::parse(rpc_url) {
        Ok(url) => match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{}://{host}:{port}", url.scheme()),
            (Some(host), None) => format!("{}://{host}", url.scheme()),
            (None, _) => "<invalid nano rpc url>".to_string(),
        },
        Err(_) => "<invalid nano rpc url>".to_string(),
    }
}

impl std::fmt::Debug for NanoVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NanoVerifier")
            .field("rpc_host", &redacted_rpc_url(&self.rpc_url))
            .field("recipient_account", &self.recipient_account)
            .finish_non_exhaustive()
    }
}

impl NanoVerifier {
    /// Build a verifier for the node at `rpc_url` that must be paid at
    /// `recipient_account` (a syntactically valid `nano_` address).
    ///
    /// The verifier owns its HTTP client and hardens it: redirects are
    /// disabled (a compromised/misconfigured endpoint must not bounce the
    /// request — and the request body — to another host) and the per-request
    /// timeout is bounded. Response size is capped separately in
    /// [`Self::fetch_block_info`].
    pub fn new(rpc_url: &str, recipient_account: &str) -> Result<Self, Error> {
        if rpc_url.trim().is_empty() {
            return Err(Error::InvalidConfig("nano rpc_url is empty".to_string()));
        }
        reqwest::Url::parse(rpc_url)
            .map_err(|_| Error::InvalidConfig("nano rpc_url is not a valid URL".to_string()))?;
        if !is_well_formed_nano_address(recipient_account) {
            return Err(Error::InvalidConfig(
                "nano recipient account is not a well-formed nano_ address".to_string(),
            ));
        }
        let http_client = Client::builder()
            .redirect(Policy::none())
            .timeout(RPC_TIMEOUT)
            .build()
            .map_err(|e| Error::InvalidConfig(format!("failed to build nano RPC client: {e}")))?;
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
    ///
    /// The response body is read incrementally under a hard byte cap — a
    /// hostile or broken endpoint cannot stream an unbounded body into the
    /// gateway's memory.
    async fn fetch_block_info(&self, hash: &str) -> Result<BlockInfo, Error> {
        let body = serde_json::json!({
            "action": "block_info",
            "json_block": "true",
            "hash": hash,
        });
        let mut response = self
            .http_client
            .post(&self.rpc_url)
            .timeout(RPC_TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Rpc(format!("nano block_info request failed: {e}")))?;
        if !response.status().is_success() {
            return Err(Error::Rpc(format!(
                "nano block_info returned HTTP {}",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|len| len > MAX_RPC_RESPONSE_BYTES as u64)
        {
            return Err(Error::Rpc(
                "nano block_info response exceeds the size cap".to_string(),
            ));
        }
        let mut buf = Vec::with_capacity(4096);
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| Error::Rpc(format!("nano block_info response read failed: {e}")))?
        {
            if buf.len().saturating_add(chunk.len()) > MAX_RPC_RESPONSE_BYTES {
                return Err(Error::Rpc(
                    "nano block_info response exceeds the size cap".to_string(),
                ));
            }
            buf.extend_from_slice(&chunk);
        }
        let value: serde_json::Value = serde_json::from_slice(&buf)
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

        // ── Signature: prove the presenter holds the payer account's key ─────
        //
        // The send block is public as soon as it is published, so `{block_hash,
        // account}` alone would be a bearer token another party could redeem
        // first. The signature binds the hash to the quoted raw amount, the
        // recipient and the request resource, and is checked against the key
        // decoded from `account` BEFORE any node call — a forged or tampered
        // receipt never reaches the RPC (and never burns the hash).
        if !is_well_formed_nano_signature(&receipt.signature) {
            return Err(Error::InvalidEncoding(
                "nano receipt signature must be 128 hex characters".to_string(),
            ));
        }
        let mut signature_bytes = [0u8; 64];
        hex::decode_to_slice(&receipt.signature, &mut signature_bytes).map_err(|_| {
            Error::InvalidEncoding("nano receipt signature is not valid hex".to_string())
        })?;
        let payer_public_key = decode_nano_account(&receipt.account)?;
        // Sign the CANONICAL decimal raw amount (not the client's raw wire
        // spelling) so `"0021"` and `"21"` cannot produce two valid messages
        // for the same payment. The signed object is the BLAKE2b-256 digest of
        // the canonical message, matching what a standard Nano signer signs.
        let signing_hash = receipt_signing_hash(
            &receipt.block_hash,
            &min_raw.to_string(),
            &accepted.pay_to,
            &payload.resource.method,
            &payload.resource.url,
        );
        if !verify_nano_signature(&payer_public_key, &signing_hash, &signature_bytes) {
            return Err(Error::InvalidTransaction(
                "nano receipt signature does not match the payer account".to_string(),
            ));
        }

        debug!(
            block_hash = %receipt.block_hash,
            payer = %receipt.account,
            min_raw,
            rpc_host = %redacted_rpc_url(&self.rpc_url),
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

    /// Test signer for the receipt golden vectors: the public key derived from
    /// the RAW 32-byte Ed25519 secret key `0x00…01` under Nano's
    /// Ed25519-BLAKE2b construction, rendered as its `nano_` address (checksum
    /// valid). This is NOT a Nano wallet seed: a seed derives account keys via
    /// BLAKE2b-512(seed ‖ index), which for the same bytes gives a different
    /// public key.
    const SIGNER: &str = "nano_3kdbxitaj7f6mrir6miiwtw4muhcc58e6tn5st6rfaxsdnb7gr4roudwn951";
    const SIGNER_PUBKEY_HEX: &str =
        "C969EC348895A49E21824E10E6B829EDEA50CCC26A83CE8986A3B95D12576058";

    /// BLAKE2b-256 digest of
    /// `receipt_signing_message(HASH, "2100000000000000000000000000", RECIPIENT,
    /// "POST", "/v1/chat/completions")` — the 32-byte object the payer signs.
    const RECEIPT_SIGNING_HASH_HEX: &str =
        "69A6A29CB9CB573B1669A5DDFDF063BF9781C047702139479D56308E35BD650B";

    /// Signature by [`SIGNER`] over [`RECEIPT_SIGNING_HASH_HEX`]. Reproduced
    /// byte-for-byte by an independent Python `ed25519-blake2b` implementation
    /// and by the standard Nano `nanocurrency` JS library's
    /// `signBlock({hash: <digest>, secretKey})` — i.e. an ordinary Nano signer
    /// produces exactly this signature, with no Solvela-specific Ed25519 code.
    const RECEIPT_SIG: &str = "C3AE24D5D6B5ECC6866426A9EE9B4B74B14B7778C8F1956C9DE8EDF87C33C9705BF173A13FF0FBBD9685CD9E3DAB68BC91B3D183A928839186BF5F3282A5080F";

    /// A real confirmed on-chain Nano block (the genesis open block) and its
    /// signature by the account — an external vector for the whole
    /// address-decode + Ed25519-BLAKE2b path.
    const GENESIS_ACCOUNT: &str = RECIPIENT;
    const GENESIS_SIG: &str = "9F0C933C8ADE004D808EA1985FA746A7E95BA2A38F867640F53EC8F180BDFE9E2C1268DEAD7C2664F356E37ABA362BC58E46DBA03E523A7B5A19E4B6EB12BB02";

    /// Signature by a DIFFERENT raw secret key (`0x00…02`) over the same
    /// digest.
    const WRONG_KEY_SIG: &str = "D331505A885A565EA2802E9FD76FA8EEF788992DB13FA523207E6D51C84DB9A8E3A6BF48550A5B078A4A5A15919D8CD25B187BD1D25B6B569391AF1757F78907";

    /// The pre-change signature over the full 209-byte message (not its
    /// digest). It must never verify: the signed object is the 32-byte digest.
    const OLD_FULL_MESSAGE_SIG: &str = "9924E55B2FE2DEE521347541932C79A3AE6DFC27F80139AEB5894257BC94F460CDBF8F36192293B5E31185BC6B32101CC02FF7398AA590AC8706E57532E6BF07";

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

    #[test]
    fn constructor_validates_config() {
        assert!(NanoVerifier::new("http://127.0.0.1:7076", RECIPIENT).is_ok());
        assert!(matches!(
            NanoVerifier::new("", RECIPIENT),
            Err(Error::InvalidConfig(_))
        ));
        assert!(matches!(
            NanoVerifier::new("http://127.0.0.1:7076", "nano_bad"),
            Err(Error::InvalidConfig(_))
        ));
        let v = NanoVerifier::new("http://127.0.0.1:7076", RECIPIENT).unwrap();
        assert_eq!(v.network(), NANO_NETWORK);
        assert_eq!(v.scheme(), "exact");
        assert_eq!(v.recipient_account(), RECIPIENT);
    }

    #[test]
    fn decodes_nano_account_and_verifies_checksum() {
        let genesis = decode_nano_account(GENESIS_ACCOUNT).unwrap();
        assert_eq!(
            hex::encode_upper(genesis),
            "E89208DD038FBB269987689621D52292AE9C35941A7484756ECCED92A65093BA"
        );

        let signer = decode_nano_account(SIGNER).unwrap();
        assert_eq!(hex::encode_upper(signer), SIGNER_PUBKEY_HEX);

        // A one-character checksum tamper is rejected (not silently accepted
        // as somebody else's key).
        let mut tampered = SIGNER.to_string();
        let last = tampered.pop().unwrap();
        tampered.push(if last == '1' { '3' } else { '1' });
        assert!(matches!(
            decode_nano_account(&tampered),
            Err(Error::InvalidEncoding(_))
        ));

        // Legacy prefix and malformed bodies fail closed.
        let xrb = format!("xrb_{}", &SIGNER["nano_".len()..]);
        assert!(matches!(
            decode_nano_account(&xrb),
            Err(Error::InvalidEncoding(_))
        ));
        assert!(matches!(
            decode_nano_account("nano_short"),
            Err(Error::InvalidEncoding(_))
        ));
        assert!(matches!(
            decode_nano_account(""),
            Err(Error::InvalidEncoding(_))
        ));
    }

    #[test]
    fn verifies_a_real_onchain_nano_signature() {
        // The genesis open block's signature is checked by nano-node itself;
        // agreeing with it proves our BLAKE2b hash choice and curve math, not
        // just self-consistency.
        let public_key = decode_nano_account(GENESIS_ACCOUNT).unwrap();
        let message = hex::decode(HASH).unwrap();
        let mut signature = [0u8; 64];
        hex::decode_to_slice(GENESIS_SIG, &mut signature).unwrap();
        assert!(verify_nano_signature(&public_key, &message, &signature));

        // Wrong message / wrong key / tampered signature are all rejected.
        let mut wrong_message = message.clone();
        wrong_message[0] ^= 1;
        assert!(!verify_nano_signature(
            &public_key,
            &wrong_message,
            &signature
        ));
        let mut tampered = signature;
        tampered[63] ^= 1;
        assert!(!verify_nano_signature(&public_key, &message, &tampered));

        let signer_key = decode_nano_account(SIGNER).unwrap();
        assert!(!verify_nano_signature(&signer_key, &message, &signature));
    }

    #[test]
    fn receipt_signature_golden_vectors() {
        let public_key = decode_nano_account(SIGNER).unwrap();
        let amount = "2100000000000000000000000000";
        let message =
            receipt_signing_message(HASH, amount, RECIPIENT, "POST", "/v1/chat/completions");

        // The signed object is the BLAKE2b-256 digest of the canonical message
        // (32 bytes), which is what a standard Nano signer takes.
        let signing_hash =
            receipt_signing_hash(HASH, amount, RECIPIENT, "POST", "/v1/chat/completions");
        assert_eq!(
            hex::encode_upper(signing_hash),
            RECEIPT_SIGNING_HASH_HEX,
            "canonical receipt digest changed — this invalidates every client signature"
        );

        let mut signature = [0u8; 64];
        hex::decode_to_slice(RECEIPT_SIG, &mut signature).unwrap();

        // Valid vector.
        assert!(verify_nano_signature(
            &public_key,
            &signing_hash,
            &signature
        ));

        // The pre-change signature over the full 209-byte message is a valid
        // Ed25519 signature over that byte string, but it must NOT verify as a
        // receipt signature: the signed object is the 32-byte digest. A client
        // still signing the raw message produces the rejected old wire form.
        let mut old_signature = [0u8; 64];
        hex::decode_to_slice(OLD_FULL_MESSAGE_SIG, &mut old_signature).unwrap();
        assert!(verify_nano_signature(&public_key, &message, &old_signature));
        assert!(!verify_nano_signature(
            &public_key,
            &signing_hash,
            &old_signature
        ));

        // Wrong key.
        let mut wrong_key = [0u8; 64];
        hex::decode_to_slice(WRONG_KEY_SIG, &mut wrong_key).unwrap();
        assert!(!verify_nano_signature(
            &public_key,
            &signing_hash,
            &wrong_key
        ));

        // Signature over a different hash.
        let other_hash = receipt_signing_hash(
            "A170D51B94E00371ACE76E35AC81DC9405D5D04D4CEBC399AEACE07AE05DD293",
            amount,
            RECIPIENT,
            "POST",
            "/v1/chat/completions",
        );
        assert!(!verify_nano_signature(&public_key, &other_hash, &signature));

        // Signature over a different amount.
        let other_amount = receipt_signing_hash(
            HASH,
            "2100000000000000000000000001",
            RECIPIENT,
            "POST",
            "/v1/chat/completions",
        );
        assert!(!verify_nano_signature(
            &public_key,
            &other_amount,
            &signature
        ));

        // Signature over a different recipient / resource.
        let other_recipient =
            receipt_signing_hash(HASH, amount, SIGNER, "POST", "/v1/chat/completions");
        assert!(!verify_nano_signature(
            &public_key,
            &other_recipient,
            &signature
        ));
        let other_resource = receipt_signing_hash(HASH, amount, RECIPIENT, "POST", "/v1/messages");
        assert!(!verify_nano_signature(
            &public_key,
            &other_resource,
            &signature
        ));
    }

    #[test]
    fn redeems_nothing_when_signature_is_malformed_or_wrong() {
        // Encoding failures are distinguished from a valid-but-wrong
        // signature; both fail closed well before the node call.
        for bad in ["", "ZZ", &RECEIPT_SIG[..126], &format!("{RECEIPT_SIG}0")] {
            assert!(!is_well_formed_nano_signature(bad), "{bad:?}");
        }
    }

    #[test]
    fn redacted_rpc_url_keeps_only_scheme_and_host() {
        let redacted = redacted_rpc_url("https://rpc.example.com:7076/secret-api-key?token=abc");
        assert_eq!(redacted, "https://rpc.example.com:7076");
        assert!(!redacted.contains("secret"));
        assert!(!redacted.contains("token"));

        let redacted = redacted_rpc_url("http://127.0.0.1:7076");
        assert_eq!(redacted, "http://127.0.0.1:7076");

        assert_eq!(redacted_rpc_url("not a url"), "<invalid nano rpc url>");
    }

    #[tokio::test]
    async fn signature_is_verified_before_any_node_call() {
        // The verifier points at an unroutable address: if any of these
        // reached the network they would hit the 10 s timeout instead of the
        // signature error, so the assertions also prove "no node call".
        let v = NanoVerifier::new("http://192.0.2.1:7076", RECIPIENT).unwrap();

        // Valid encoding, wrong signer key.
        let mut p = payload(nano_payload());
        if let PayloadData::Nano(ref mut n) = p.payload {
            n.signature = WRONG_KEY_SIG.to_string();
        }
        assert!(matches!(
            v.verify_payment(&p).await,
            Err(Error::InvalidTransaction(_))
        ));

        // Signature over a different amount.
        let mut p = payload(nano_payload());
        p.accepted.amount = "2100000000000000000000000001".to_string();
        assert!(matches!(
            v.verify_payment(&p).await,
            Err(Error::InvalidTransaction(_))
        ));

        // Signature over a different block hash.
        let mut p = payload(nano_payload());
        if let PayloadData::Nano(ref mut n) = p.payload {
            n.block_hash =
                "A170D51B94E00371ACE76E35AC81DC9405D5D04D4CEBC399AEACE07AE05DD293".to_string();
        }
        assert!(matches!(
            v.verify_payment(&p).await,
            Err(Error::InvalidTransaction(_))
        ));

        // Malformed signature encoding.
        let mut p = payload(nano_payload());
        if let PayloadData::Nano(ref mut n) = p.payload {
            n.signature = "not-hex".to_string();
        }
        assert!(matches!(
            v.verify_payment(&p).await,
            Err(Error::InvalidEncoding(_))
        ));
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
            account: SIGNER.to_string(),
            signature: RECEIPT_SIG.to_string(),
        })
    }

    /// Every structural rejection happens BEFORE any RPC call — the verifier
    /// points at an unroutable address, so reaching the network would hang
    /// (and the 10 s timeout would make the test slow), not pass.
    #[tokio::test]
    async fn structural_rejections_never_reach_the_node() {
        let v = NanoVerifier::new("http://192.0.2.1:7076", RECIPIENT).unwrap();

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
            account: SIGNER.to_string(),
            signature: RECEIPT_SIG.to_string(),
        }));
        assert!(matches!(
            v.verify_payment(&p).await,
            Err(Error::InvalidEncoding(_))
        ));

        let p = payload(PayloadData::Nano(NanoPayload {
            block_hash: HASH.to_string(),
            account: "nano_short".to_string(),
            signature: RECEIPT_SIG.to_string(),
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
        let v = NanoVerifier::new("http://192.0.2.1:7076", RECIPIENT).unwrap();
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

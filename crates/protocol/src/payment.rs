use serde::{Deserialize, Serialize};

use crate::cost::CostBreakdown;

/// Describes a resource that requires payment.
///
/// `deny_unknown_fields`: the field set is fully owned by this codebase
/// and the x402 spec; rejecting unknown keys at parse time prevents
/// clients from stuffing extra metadata into the inbound
/// `PaymentPayload.resource` where the gateway might later start reading
/// it without realizing it's unvalidated.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    /// The URL path of the resource.
    pub url: String,
    /// HTTP method.
    pub method: String,
}

/// Describes an accepted payment method.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaymentAccept {
    /// Payment scheme (e.g., "exact", "escrow").
    pub scheme: String,
    /// Network identifier (e.g., "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp").
    pub network: String,
    /// Amount in atomic units (USDC has 6 decimals).
    pub amount: String,
    /// Token mint/contract address.
    pub asset: String,
    /// Recipient wallet address.
    pub pay_to: String,
    /// Maximum seconds the payment authorization is valid.
    pub max_timeout_seconds: u64,
    /// Escrow program ID — only present for scheme="escrow".
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub escrow_program_id: Option<String>,
}

/// The full 402 Payment Required response body.
///
/// `extensions` carries OPTIONAL, purely-additive discovery metadata (today
/// only the Coinbase-Bazaar block — see [`bazaar_discovery_extension`]). It is
/// emitted on the gateway→client challenge body so x402 indexers
/// (x402scan / agentcash) mark the resource invocable; it is NEVER part of the
/// value path — clients sign `accepts`, not `extensions`, and the gateway's
/// verify/settle path never re-parses it. The field is `Option` + skipped when
/// `None` so callers that don't advertise it round-trip unchanged.
///
/// Unlike the client→server payment types (which stay strict), this
/// server→client challenge is intentionally NOT `deny_unknown_fields`: it must
/// remain forward-extensible so adding future discovery metadata never breaks a
/// client deserializing the body against an older schema (a robust client
/// ignores challenge fields it does not recognize).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentRequired {
    pub x402_version: u8,
    pub resource: Resource,
    pub accepts: Vec<PaymentAccept>,
    pub cost_breakdown: CostBreakdown,
    pub error: String,
    /// Optional, additive discovery metadata. When present, carries
    /// `{"bazaar": {...}}` (see [`bazaar_discovery_extension`]). Tolerant on
    /// the inbound side: absent in older bodies, ignored when reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<serde_json::Value>,
}

/// Build the STATIC Coinbase-Bazaar discovery extension block embedded under
/// `PaymentRequired.extensions` (`{"bazaar": {...}}`).
///
/// This is the ONLY way a *self-settling* x402 resource (Solvela does not run
/// on Coinbase's facilitator) becomes "invocable" on the discovery indexers:
/// they read invocability from the LIVE 402 challenge body, not from OpenAPI.
///
/// - x402scan reads `extensions.bazaar.info` (its presence flips the resource
///   from "strict non-invocable / skipped" to invocable).
/// - agentcash `@agentcash/discovery` (`extractSchemas2`) reads the request
///   input schema from `extensions.bazaar.schema.properties.input.properties.body`
///   and the response output example from
///   `extensions.bazaar.schema.properties.output.properties.example`.
///
/// The returned value is byte-identical on every call: it contains NO
/// wallet/amount/time/per-request data. It is discovery metadata only —
/// mutating it does not change who pays, how much, or settlement. The request
/// schema mirrors `ChatCompletionRequest` (docs/openapi.json); the output
/// example is a representative `chat.completion` object.
pub fn bazaar_discovery_extension() -> serde_json::Value {
    // A representative (non-binding, static) chat.completion response. Used for
    // BOTH `info.output.example` (x402scan) and
    // `schema.properties.output.properties.example` (agentcash) so the two
    // tools agree on the same example object.
    let example_completion = serde_json::json!({
        "id": "chatcmpl-solvela-example",
        "object": "chat.completion",
        "created": 1_700_000_000,
        "model": "openai/gpt-4o",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Solana is a high-throughput proof-of-stake blockchain."
                },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 12,
            "completion_tokens": 11,
            "total_tokens": 23
        }
    });

    // Faithful JSON Schema of the chat request body (mirrors the
    // ChatCompletionRequest component in docs/openapi.json).
    let request_body_schema = serde_json::json!({
        "type": "object",
        "required": ["model", "messages"],
        "properties": {
            "model": {
                "type": "string",
                "description": "Model id, alias (e.g. \"sonnet\"), or routing profile (e.g. \"auto\")."
            },
            "messages": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "required": ["role", "content"],
                    "properties": {
                        "role": {
                            "type": "string",
                            "enum": ["system", "user", "assistant", "tool"]
                        },
                        "content": { "type": "string" }
                    }
                }
            },
            "max_tokens": { "type": "integer" },
            "temperature": { "type": "number" },
            "top_p": { "type": "number" },
            "stream": { "type": "boolean" }
        }
    });

    serde_json::json!({
        "bazaar": {
            // x402scan invocability gate.
            "info": {
                "input": { "type": "http", "method": "POST", "bodyType": "json" },
                "output": { "type": "json", "example": example_completion }
            },
            // agentcash extractSchemas2 input/output schema paths.
            "schema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "input": {
                        "type": "object",
                        "properties": { "body": request_body_schema }
                    },
                    "output": {
                        "type": "object",
                        "properties": { "example": example_completion }
                    }
                }
            }
        }
    })
}

/// Solana-specific payment data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolanaPayload {
    /// Base64-encoded signed versioned transaction.
    pub transaction: String,
}

/// Escrow-specific payment payload (scheme = "escrow").
///
/// `deny_unknown_fields` interacts with the parent `PayloadData` enum's
/// `untagged` deserialization to reject ambiguous payloads: a client that
/// sends both `transaction` (Direct's field) and `deposit_tx` (Escrow's
/// fields) used to silently deserialize as `Escrow` because Escrow had
/// all required fields. With `deny_unknown_fields` Escrow now rejects
/// the extra `transaction` key, and untagged falls through to Direct
/// (which then rejects the extra `deposit_tx`), so the ambiguous case
/// errors out at parse time instead of silently picking one variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EscrowPayload {
    /// Base64-encoded signed deposit transaction (Solana versioned tx).
    pub deposit_tx: String,
    /// 32-byte request correlation ID — used as escrow PDA seed.
    /// Base64-encoded.
    pub service_id: String,
    /// Agent wallet pubkey (base58) — used to derive escrow PDA.
    pub agent_pubkey: String,
}

/// v0 spend-down channel voucher payload (scheme = "channel").
///
/// The credential presented on a per-call channel DRAW: a cumulative voucher
/// signed by the channel's `session_key`. Mirrors [`EscrowPayload`] as the
/// wire-envelope for a distinct scheme — the SIGNED object is the frozen
/// `solvela_x402::channel::Voucher` (golden-vector-pinned via
/// `GOLDEN_VECTOR_B64`); this struct is only its base58/base64 wire encoding.
///
/// `deny_unknown_fields`: like [`EscrowPayload`], strict rejection lets the
/// parent `untagged` enum disambiguate a channel voucher (field set disjoint
/// from both Escrow's `deposit_tx`/`service_id`/`agent_pubkey` and Direct's
/// `transaction`) rather than silently picking a variant.
///
/// This is a cross-repo wire contract (the PR6 SDK signer round-trips it); its
/// JSON shape is golden-vectored the same way `EscrowPayload` is.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelVoucherPayload {
    /// base58 of the 32-byte channel id the voucher signs over.
    pub channel_id: String,
    /// Signed running fee-inclusive total for this channel (atomic micro-USDC),
    /// monotonic non-decreasing = `last_cumulative + this call's billed`.
    pub cumulative_atomic: u64,
    /// Slot at which the voucher expires.
    pub expiry_slot: u64,
    /// Per-draw audit/correlation sequence number. NOT enforced for uniqueness —
    /// the monotonic `cumulative_atomic` advance + the gateway's per-channel lock
    /// are the replay guard, not this field.
    pub nonce: u64,
    /// base64 of the 32-byte SHA-256 of the RAW request body this voucher pays
    /// for (bound by the signature).
    pub request_digest: String,
    /// base64 of the 64-byte ed25519 signature over the canonical voucher
    /// message, verified against the channel's `session_key`.
    pub signature: String,
}

/// Byte-exact JSON of a [`ChannelVoucherPayload`] for a fixed input — the
/// cross-repo HEADER wire contract, pinned like the signed voucher message's
/// `solvela_x402::channel::GOLDEN_VECTOR_B64`. A field rename/reorder becomes a
/// BUILD failure here, not a draw-time SDK break. Serde emits struct fields in
/// declaration order; because [`PayloadData`] is `untagged`, the enum serializes
/// IDENTICALLY to the inner payload (asserted alongside). DO NOT hand-edit.
pub const CHANNEL_VOUCHER_PAYLOAD_GOLDEN_JSON: &str = r#"{"channel_id":"cid","cumulative_atomic":12600,"expiry_slot":1000750,"nonce":42,"request_digest":"ZA==","signature":"c2ln"}"#;

/// Union of direct-transfer, escrow, and channel-voucher payment payloads.
///
/// `#[serde(untagged)]`: deserialization tries each variant in declaration
/// order and takes the first that succeeds. Order does NOT affect correctness
/// here — all three are `deny_unknown_fields` with DISJOINT required field sets
/// (Escrow `{deposit_tx, service_id, agent_pubkey}` has FEWER fields than
/// Channel `{channel_id, cumulative_atomic, expiry_slot, nonce, request_digest,
/// signature}`, and Direct is just `{transaction}`), so any given object matches
/// AT MOST one variant regardless of order.
///
/// LANDMINE: this soundness depends on serde's `deny_unknown_fields` +
/// `untagged` interaction (serde-rs/serde#1546). The `Cargo.lock` serde version
/// is load-bearing; a serde upgrade that changed untagged/deny_unknown_fields
/// semantics would be caught by
/// `test_payload_data_channel_disambiguates_from_escrow_and_direct` (the hybrid
/// case must still fail to parse) before it could silently mis-route a payment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PayloadData {
    Escrow(EscrowPayload),
    /// v0 spend-down channel voucher (scheme = "channel"). See
    /// [`ChannelVoucherPayload`].
    Channel(ChannelVoucherPayload),
    Direct(SolanaPayload),
}

/// The payment payload sent in the `PAYMENT-SIGNATURE` header.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaymentPayload {
    pub x402_version: u8,
    pub resource: Resource,
    pub accepted: PaymentAccept,
    pub payload: PayloadData,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::*;
    use crate::cost::CostBreakdown;

    #[test]
    fn test_payment_required_serialization() {
        let pr = PaymentRequired {
            x402_version: X402_VERSION,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepts: vec![PaymentAccept {
                scheme: "exact".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "2625".to_string(),
                asset: USDC_MINT.to_string(),
                pay_to: "RecipientWalletPubkeyHere".to_string(),
                max_timeout_seconds: MAX_TIMEOUT_SECONDS,
                escrow_program_id: None,
            }],
            cost_breakdown: CostBreakdown {
                provider_cost: "0.002500".to_string(),
                platform_fee: "0.000125".to_string(),
                total: "0.002625".to_string(),
                currency: "USDC".to_string(),
                fee_percent: 5,
            },
            error: "Payment required".to_string(),
            extensions: None,
        };
        let json = serde_json::to_string_pretty(&pr).unwrap();
        assert!(json.contains("x402_version"));
        assert!(json.contains("solana:"));
        assert!(json.contains("cost_breakdown"));
        // `extensions` is skipped when None — older clients see no new key.
        assert!(
            !json.contains("extensions"),
            "extensions must be absent from the body when None"
        );
        // Round-trips through deny_unknown_fields without the field present.
        let _back: PaymentRequired = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn test_bazaar_discovery_extension_shape_and_stability() {
        let a = bazaar_discovery_extension();
        let b = bazaar_discovery_extension();
        // STATIC: byte-identical on every call (no per-request data).
        assert_eq!(a, b, "bazaar discovery block must be static");

        let bazaar = &a["bazaar"];

        // x402scan reads `.info` (its presence flips invocable).
        assert!(bazaar["info"].is_object());
        assert_eq!(bazaar["info"]["input"]["method"], "POST");
        assert_eq!(
            bazaar["info"]["output"]["example"]["object"],
            "chat.completion"
        );

        // agentcash extractSchemas2 input-schema path.
        let input_body = &bazaar["schema"]["properties"]["input"]["properties"]["body"];
        assert_eq!(input_body["type"], "object");
        assert!(input_body["properties"]["model"].is_object());
        assert!(input_body["properties"]["messages"].is_object());
        let required = input_body["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "model"));
        assert!(required.iter().any(|v| v == "messages"));

        // agentcash extractSchemas2 output-example path.
        let example = &bazaar["schema"]["properties"]["output"]["properties"]["example"];
        assert_eq!(example["object"], "chat.completion");
        assert!(example["choices"][0]["message"].is_object());
        assert!(example["usage"].is_object());
    }

    #[test]
    fn test_payment_required_with_extensions_roundtrips() {
        let pr = PaymentRequired {
            x402_version: X402_VERSION,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepts: vec![PaymentAccept {
                scheme: "exact".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "2625".to_string(),
                asset: USDC_MINT.to_string(),
                pay_to: "RecipientWalletPubkeyHere".to_string(),
                max_timeout_seconds: MAX_TIMEOUT_SECONDS,
                escrow_program_id: None,
            }],
            cost_breakdown: CostBreakdown {
                provider_cost: "0.002500".to_string(),
                platform_fee: "0.000125".to_string(),
                total: "0.002625".to_string(),
                currency: "USDC".to_string(),
                fee_percent: 5,
            },
            error: "Payment required".to_string(),
            extensions: Some(bazaar_discovery_extension()),
        };
        let json = serde_json::to_string(&pr).unwrap();
        assert!(json.contains("\"extensions\""));
        assert!(json.contains("\"bazaar\""));
        // A body carrying the extension still parses under deny_unknown_fields
        // (the field is now known) — round-trip equality on the extension.
        let back: PaymentRequired = serde_json::from_str(&json).unwrap();
        assert_eq!(back.extensions, Some(bazaar_discovery_extension()));
    }

    #[test]
    fn test_payment_accept_escrow_serialization() {
        let accept = PaymentAccept {
            scheme: "escrow".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: "5000".to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: "RecipientWalletPubkeyHere".to_string(),
            max_timeout_seconds: MAX_TIMEOUT_SECONDS,
            escrow_program_id: Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string()),
        };
        let json = serde_json::to_string(&accept).unwrap();
        assert!(json.contains("escrow_program_id"));
        assert!(json.contains("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"));
    }

    #[test]
    fn test_payment_accept_exact_no_escrow_field() {
        let accept = PaymentAccept {
            scheme: "exact".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: "2625".to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: "RecipientWalletPubkeyHere".to_string(),
            max_timeout_seconds: MAX_TIMEOUT_SECONDS,
            escrow_program_id: None,
        };
        let json = serde_json::to_string(&accept).unwrap();
        assert!(
            !json.contains("escrow_program_id"),
            "escrow_program_id should be absent when None"
        );
    }

    #[test]
    fn test_payload_data_direct_roundtrip() {
        let direct = PayloadData::Direct(SolanaPayload {
            transaction: "dGVzdA==".to_string(),
        });
        let json = serde_json::to_string(&direct).unwrap();
        let deserialized: PayloadData = serde_json::from_str(&json).unwrap();
        match deserialized {
            PayloadData::Direct(p) => assert_eq!(p.transaction, "dGVzdA=="),
            PayloadData::Escrow(_) => panic!("expected Direct variant"),
            PayloadData::Channel(_) => panic!("expected Direct variant"),
        }
    }

    #[test]
    fn test_payload_data_escrow_roundtrip() {
        let escrow = PayloadData::Escrow(EscrowPayload {
            deposit_tx: "dGVzdA==".to_string(),
            service_id: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
            agent_pubkey: "11111111111111111111111111111111".to_string(),
        });
        let json = serde_json::to_string(&escrow).unwrap();
        let deserialized: PayloadData = serde_json::from_str(&json).unwrap();
        match deserialized {
            PayloadData::Escrow(p) => {
                assert_eq!(p.deposit_tx, "dGVzdA==");
                assert_eq!(p.agent_pubkey, "11111111111111111111111111111111");
            }
            PayloadData::Direct(_) => panic!("expected Escrow variant"),
            PayloadData::Channel(_) => panic!("expected Escrow variant"),
        }
    }

    #[test]
    fn channel_voucher_payload_matches_golden_json() {
        let v = ChannelVoucherPayload {
            channel_id: "cid".to_string(),
            cumulative_atomic: 12_600,
            expiry_slot: 1_000_750,
            nonce: 42,
            request_digest: "ZA==".to_string(),
            signature: "c2ln".to_string(),
        };
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            CHANNEL_VOUCHER_PAYLOAD_GOLDEN_JSON,
            "ChannelVoucherPayload JSON drifted from the pinned cross-repo header contract"
        );
        // `untagged` PayloadData::Channel serializes IDENTICALLY to the inner
        // payload — the header carries exactly this shape.
        assert_eq!(
            serde_json::to_string(&PayloadData::Channel(v)).unwrap(),
            CHANNEL_VOUCHER_PAYLOAD_GOLDEN_JSON
        );
    }

    #[test]
    fn test_payload_data_channel_roundtrip() {
        let channel = PayloadData::Channel(ChannelVoucherPayload {
            channel_id: "11111111111111111111111111111111".to_string(),
            cumulative_atomic: 12_600,
            expiry_slot: 1_000_750,
            nonce: 42,
            request_digest: "IiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiI=".to_string(),
            signature: "MzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMw==".to_string(),
        });
        let json = serde_json::to_string(&channel).unwrap();
        let deserialized: PayloadData = serde_json::from_str(&json).unwrap();
        match deserialized {
            PayloadData::Channel(p) => {
                assert_eq!(p.channel_id, "11111111111111111111111111111111");
                assert_eq!(p.cumulative_atomic, 12_600);
                assert_eq!(p.expiry_slot, 1_000_750);
                assert_eq!(p.nonce, 42);
            }
            PayloadData::Escrow(_) => panic!("expected Channel variant"),
            PayloadData::Direct(_) => panic!("expected Channel variant"),
        }
    }

    /// The untagged `PayloadData` must disambiguate a channel voucher from BOTH
    /// Escrow and Direct: the three field sets are disjoint and every variant
    /// `deny_unknown_fields`, so a channel voucher can never silently decode as
    /// Escrow/Direct (nor they as Channel). Guards the wire-format contract the
    /// search-route channel fork keys on.
    #[test]
    fn test_payload_data_channel_disambiguates_from_escrow_and_direct() {
        // A channel voucher decodes ONLY as Channel.
        let voucher_json = r#"{"channel_id":"cid","cumulative_atomic":12600,"expiry_slot":1000750,"nonce":42,"request_digest":"ZA==","signature":"c2ln"}"#;
        assert!(matches!(
            serde_json::from_str::<PayloadData>(voucher_json).unwrap(),
            PayloadData::Channel(_)
        ));

        // A Direct payload does NOT decode as Channel.
        let direct_json = r#"{"transaction":"dGVzdA=="}"#;
        assert!(matches!(
            serde_json::from_str::<PayloadData>(direct_json).unwrap(),
            PayloadData::Direct(_)
        ));

        // An Escrow payload does NOT decode as Channel.
        let escrow_json = r#"{"deposit_tx":"dGVzdA==","service_id":"c2Vydg==","agent_pubkey":"11111111111111111111111111111111"}"#;
        assert!(matches!(
            serde_json::from_str::<PayloadData>(escrow_json).unwrap(),
            PayloadData::Escrow(_)
        ));

        // A channel voucher carrying an EXTRA (Direct's) field is rejected by
        // deny_unknown_fields — it matches no variant, never silently downgrades.
        let hybrid = r#"{"channel_id":"cid","cumulative_atomic":1,"expiry_slot":1,"nonce":1,"request_digest":"ZA==","signature":"c2ln","transaction":"x"}"#;
        assert!(serde_json::from_str::<PayloadData>(hybrid).is_err());
    }

    #[test]
    fn test_escrow_payload_serde_roundtrip() {
        let ep = EscrowPayload {
            deposit_tx: "abc123".to_string(),
            service_id: "c2VydmljZTEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNA==".to_string(),
            agent_pubkey: "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz".to_string(),
        };
        let json = serde_json::to_string(&ep).unwrap();
        let deserialized: EscrowPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.deposit_tx, ep.deposit_tx);
        assert_eq!(deserialized.service_id, ep.service_id);
        assert_eq!(deserialized.agent_pubkey, ep.agent_pubkey);
    }
}

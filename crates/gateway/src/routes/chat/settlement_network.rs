//! Per-network validation of the client-echoed `accepted` block.
//!
//! The chat route used to hard-check `accepted.{network, asset, pay_to}`
//! against the Solana config inline. With a second settlement network
//! (`nano:mainnet`, issue #813) those checks — and the amount check, whose
//! unit differs per network — are resolved here from ONE enum so a new
//! network is an explicit arm, never a silent fall-through.
//!
//! GHSA-cgqx-mg48-949v posture throughout: every client-facing message is a
//! FIXED string; the attacker-controlled field is logged (truncated)
//! server-side only.

use tracing::warn;

use solvela_protocol::{
    usdc_atomic_to_nano_raw, MAX_TIMEOUT_SECONDS, NANO_ASSET, NANO_NETWORK, SOLANA_NETWORK,
};
use solvela_x402::types::{PayloadData, PaymentAccept};

use crate::config::{AppConfig, NanoConfig};
use crate::error::GatewayError;

/// The settlement network a paid request is using, resolved from
/// `accepted.network` against what this deployment actually offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettlementNetwork {
    /// USDC-SPL on Solana — `exact` / `escrow`.
    Solana,
    /// XNO receipts on Nano — `exact` only.
    Nano,
}

/// Fixed client-facing rejection for a network this deployment does not offer.
const UNSUPPORTED_NETWORK_MSG: &str =
    "Payment network is unsupported. Use the network advertised in the 402 response.";

/// Resolve `accepted.network` and validate `asset` / `pay_to` / scheme against
/// the configured values for that network — the same values the matching
/// verifier enforces, never compile-time constants.
///
/// `nano:mainnet` resolves ONLY when `config.nano` is set; an unconfigured
/// deployment rejects it exactly like any unknown network (wire-indistinguishable
/// from a build without the feature).
pub(crate) fn resolve_settlement_network(
    config: &AppConfig,
    accepted: &PaymentAccept,
) -> Result<SettlementNetwork, GatewayError> {
    if accepted.network.eq_ignore_ascii_case(SOLANA_NETWORK) {
        // Asset is the CONFIGURED USDC-SPL mint. Cap the logged copy to the
        // max base58 pubkey length (44 chars), chars-based so a multibyte
        // boundary can never panic.
        if accepted.asset != config.solana.usdc_mint {
            let asset_prefix: String = accepted.asset.chars().take(44).collect();
            warn!(
                asset = %asset_prefix,
                expected = %config.solana.usdc_mint,
                "payment asset mismatch"
            );
            return Err(GatewayError::BadRequest(
                "Payment asset is unsupported. Use the asset advertised in the 402 response."
                    .to_string(),
            ));
        }
        if accepted.pay_to != config.solana.recipient_wallet {
            warn!(
                pay_to = %accepted.pay_to,
                expected = %config.solana.recipient_wallet,
                "payment pay_to mismatch"
            );
            return Err(GatewayError::BadRequest(
                "Payment recipient does not match. Use the pay_to advertised in the 402 response."
                    .to_string(),
            ));
        }
        return Ok(SettlementNetwork::Solana);
    }

    if accepted.network == NANO_NETWORK {
        let Some(nano) = &config.nano else {
            warn!(network = NANO_NETWORK, "payment network not configured");
            return Err(GatewayError::BadRequest(
                UNSUPPORTED_NETWORK_MSG.to_string(),
            ));
        };
        // Only `exact` exists on Nano (no escrow program, no channel). Reject
        // BEFORE the channel fork so a `channel` voucher can never ride a
        // Nano-network `accepted` into the draw path.
        if accepted.scheme != "exact" {
            let scheme: String = accepted.scheme.chars().take(16).collect();
            warn!(scheme = %scheme, "nano payment with non-exact scheme");
            return Err(GatewayError::BadRequest(
                "Only the 'exact' scheme is offered on nano:mainnet.".to_string(),
            ));
        }
        if accepted.asset != NANO_ASSET {
            let asset_prefix: String = accepted.asset.chars().take(44).collect();
            warn!(asset = %asset_prefix, expected = NANO_ASSET, "payment asset mismatch");
            return Err(GatewayError::BadRequest(
                "Payment asset is unsupported. Use the asset advertised in the 402 response."
                    .to_string(),
            ));
        }
        if accepted.pay_to != nano.recipient_account {
            let pay_to: String = accepted.pay_to.chars().take(65).collect();
            warn!(
                pay_to = %pay_to,
                expected = %nano.recipient_account,
                "payment pay_to mismatch"
            );
            return Err(GatewayError::BadRequest(
                "Payment recipient does not match. Use the pay_to advertised in the 402 response."
                    .to_string(),
            ));
        }
        return Ok(SettlementNetwork::Nano);
    }

    let network: String = accepted.network.chars().take(64).collect();
    warn!(network = %network, "payment network mismatch");
    Err(GatewayError::BadRequest(
        UNSUPPORTED_NETWORK_MSG.to_string(),
    ))
}

/// C1: validate that the client-signed `accepted.amount` covers the recomputed
/// quote, and return the **micro-USD figure to bill** (`client_amount`).
///
/// - Solana: `amount` is micro-USDC; it must be ≥ `expected_micro` and the
///   signed amount itself is what settles (and is billed).
/// - Nano: `amount` is raw; it must be ≥ the raw conversion of
///   `expected_micro` at the configured rate. The spend ledger is USD-
///   denominated, so the figure billed/recorded is `expected_micro` — the
///   quote the receipt paid for — never a chain-derived number.
pub(crate) fn validate_client_amount(
    network: SettlementNetwork,
    nano: Option<&NanoConfig>,
    accepted_amount: &str,
    expected_micro: u64,
    model: &str,
) -> Result<u64, GatewayError> {
    match network {
        SettlementNetwork::Solana => {
            let client_amount: u64 = accepted_amount.parse().map_err(|_| {
                warn!(
                    amount = %accepted_amount,
                    "client supplied non-integer payment amount"
                );
                GatewayError::BadRequest(
                    "Payment amount must be a valid integer (atomic USDC units).".to_string(),
                )
            })?;
            if client_amount < expected_micro {
                warn!(
                    client_amount,
                    expected_amount = expected_micro,
                    model = %model,
                    "payment amount insufficient"
                );
                return Err(GatewayError::BadRequest(format!(
                    "payment amount insufficient: paid {client_amount} but cost is {expected_micro} atomic USDC"
                )));
            }
            Ok(client_amount)
        }
        SettlementNetwork::Nano => {
            // `resolve_settlement_network` only yields `Nano` when configured;
            // fail closed anyway rather than assume.
            let nano = nano.ok_or_else(|| {
                GatewayError::Internal("nano network resolved without nano config".to_string())
            })?;
            let client_raw: u128 = accepted_amount.parse().map_err(|_| {
                let amount: String = accepted_amount.chars().take(48).collect();
                warn!(amount = %amount, "client supplied non-integer nano amount");
                GatewayError::BadRequest(
                    "Payment amount must be a valid integer (raw units).".to_string(),
                )
            })?;
            let expected_raw = usdc_atomic_to_nano_raw(expected_micro, nano.micro_usd_per_xno)
                .map_err(|e| {
                    GatewayError::Internal(format!("failed to convert quote to raw: {e}"))
                })?;
            if client_raw < expected_raw {
                warn!(
                    client_raw,
                    expected_raw,
                    expected_micro,
                    model = %model,
                    "nano payment amount insufficient"
                );
                return Err(GatewayError::BadRequest(format!(
                    "payment amount insufficient: paid {client_raw} raw but cost is {expected_raw} raw"
                )));
            }
            Ok(expected_micro)
        }
    }
}

/// M6 extension: the payload variant must match the resolved network.
///
/// A Nano-network payment must carry a Nano receipt; a Solana-network payment
/// must NOT. (The exact-vs-escrow pairing on Solana stays with the original
/// inline check.) Fail closed on every cross-pairing — a receipt must never
/// reach the Solana verifier nor a Solana tx the Nano verifier, even though
/// both would also reject it.
pub(crate) fn validate_payload_matches_network(
    network: SettlementNetwork,
    payload: &PayloadData,
) -> Result<(), GatewayError> {
    match (network, payload) {
        (SettlementNetwork::Nano, PayloadData::Nano(_)) => Ok(()),
        (SettlementNetwork::Nano, _) => Err(GatewayError::BadRequest(
            "network is nano:mainnet but the payload is not a nano receipt".to_string(),
        )),
        (SettlementNetwork::Solana, PayloadData::Nano(_)) => Err(GatewayError::BadRequest(
            "payload is a nano receipt but the network is not nano:mainnet".to_string(),
        )),
        (SettlementNetwork::Solana, _) => Ok(()),
    }
}

/// The `nano:mainnet` entry for a 402 `accepts[]`, converting the micro-USD
/// quote to raw at the configured rate (rounded up).
///
/// Returns `None` — and logs — if the conversion overflows (a quote beyond
/// Nano's entire supply); the Solana entries are still advertised, so the
/// challenge degrades to Solana-only rather than failing or mis-quoting.
pub(crate) fn nano_payment_accept(nano: &NanoConfig, amount_micro: &str) -> Option<PaymentAccept> {
    let micro: u64 = match amount_micro.parse() {
        Ok(m) => m,
        Err(_) => {
            warn!(amount = %amount_micro, "402 quote is not a micro-USD integer; omitting nano accept");
            return None;
        }
    };
    let raw = match usdc_atomic_to_nano_raw(micro, nano.micro_usd_per_xno) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, micro, "nano quote conversion failed; omitting nano accept");
            return None;
        }
    };
    Some(PaymentAccept {
        scheme: "exact".to_string(),
        network: NANO_NETWORK.to_string(),
        amount: raw.to_string(),
        asset: NANO_ASSET.to_string(),
        pay_to: nano.recipient_account.clone(),
        max_timeout_seconds: MAX_TIMEOUT_SECONDS,
        escrow_program_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use solvela_x402::types::{NanoPayload, SolanaPayload};

    const NANO_ACCT: &str = "nano_3t6k35gi95xu6tergt6p69ck76ogmitsa8mnijtpxm9fkcm736xtoncuohr3";
    const PAYER: &str = "nano_1ipx847tk8o46pwxt5qjdbncjqcbwcc1rrmqnkztrfjy5k7z4imsrata9est";

    fn nano_config() -> NanoConfig {
        NanoConfig {
            rpc_url: "http://127.0.0.1:7076".to_string(),
            recipient_account: NANO_ACCT.to_string(),
            micro_usd_per_xno: 1_250_000,
        }
    }

    fn config(with_nano: bool) -> AppConfig {
        let mut c = AppConfig::default();
        c.solana.recipient_wallet = "RecipientWalletPubkeyHere".to_string();
        if with_nano {
            c.nano = Some(nano_config());
        }
        c
    }

    fn solana_accept() -> PaymentAccept {
        PaymentAccept {
            scheme: "exact".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: "2625".to_string(),
            asset: config(false).solana.usdc_mint,
            pay_to: "RecipientWalletPubkeyHere".to_string(),
            max_timeout_seconds: MAX_TIMEOUT_SECONDS,
            escrow_program_id: None,
        }
    }

    fn nano_accept() -> PaymentAccept {
        nano_payment_accept(&nano_config(), "2625").unwrap()
    }

    fn msg(e: GatewayError) -> String {
        match e {
            GatewayError::BadRequest(m) | GatewayError::InvalidPayment(m) => m,
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn solana_accept_resolves_and_validates_asset_and_pay_to() {
        let cfg = config(false);
        assert_eq!(
            resolve_settlement_network(&cfg, &solana_accept()).unwrap(),
            SettlementNetwork::Solana
        );
        let mut a = solana_accept();
        a.asset = "WrongMint".to_string();
        assert!(msg(resolve_settlement_network(&cfg, &a).unwrap_err()).contains("asset"));
        let mut a = solana_accept();
        a.pay_to = "Someone".to_string();
        assert!(msg(resolve_settlement_network(&cfg, &a).unwrap_err()).contains("recipient"));
    }

    #[test]
    fn nano_accept_is_rejected_as_unsupported_when_not_configured() {
        let err = resolve_settlement_network(&config(false), &nano_accept()).unwrap_err();
        assert_eq!(msg(err), UNSUPPORTED_NETWORK_MSG);
        // Indistinguishable from any unknown network.
        let mut a = solana_accept();
        a.network = "ethereum:1".to_string();
        assert_eq!(
            msg(resolve_settlement_network(&config(false), &a).unwrap_err()),
            UNSUPPORTED_NETWORK_MSG
        );
    }

    #[test]
    fn nano_accept_resolves_when_configured_and_validates_fields() {
        let cfg = config(true);
        assert_eq!(
            resolve_settlement_network(&cfg, &nano_accept()).unwrap(),
            SettlementNetwork::Nano
        );
        let mut a = nano_accept();
        a.scheme = "escrow".to_string();
        assert!(msg(resolve_settlement_network(&cfg, &a).unwrap_err()).contains("exact"));
        let mut a = nano_accept();
        a.scheme = "channel".to_string();
        assert!(resolve_settlement_network(&cfg, &a).is_err());
        let mut a = nano_accept();
        a.asset = "USDC".to_string();
        assert!(msg(resolve_settlement_network(&cfg, &a).unwrap_err()).contains("asset"));
        let mut a = nano_accept();
        a.pay_to = PAYER.to_string();
        assert!(msg(resolve_settlement_network(&cfg, &a).unwrap_err()).contains("recipient"));
        // Nano network id is case-sensitive (no eq_ignore_ascii_case leniency).
        let mut a = nano_accept();
        a.network = "NANO:MAINNET".to_string();
        assert!(resolve_settlement_network(&cfg, &a).is_err());
    }

    #[test]
    fn solana_amount_validation_bills_the_signed_amount() {
        assert_eq!(
            validate_client_amount(SettlementNetwork::Solana, None, "2625", 2625, "m").unwrap(),
            2625
        );
        // Overpayment is billed as signed (unchanged behaviour).
        assert_eq!(
            validate_client_amount(SettlementNetwork::Solana, None, "3000", 2625, "m").unwrap(),
            3000
        );
        assert!(
            validate_client_amount(SettlementNetwork::Solana, None, "2624", 2625, "m").is_err()
        );
        assert!(validate_client_amount(SettlementNetwork::Solana, None, "x", 2625, "m").is_err());
    }

    #[test]
    fn nano_amount_validation_compares_raw_and_bills_the_micro_usd_quote() {
        let nano = nano_config();
        // $0.002625 at $1.25/XNO = 2.1e27 raw exactly.
        let exact_raw = "2100000000000000000000000000";
        assert_eq!(
            validate_client_amount(SettlementNetwork::Nano, Some(&nano), exact_raw, 2625, "m")
                .unwrap(),
            2625,
            "the billed figure is the micro-USD quote, not the raw amount"
        );
        // Overpaying in raw still bills the quote.
        assert_eq!(
            validate_client_amount(
                SettlementNetwork::Nano,
                Some(&nano),
                "9999999999999999999999999999",
                2625,
                "m"
            )
            .unwrap(),
            2625
        );
        // One raw short is insufficient.
        let err = validate_client_amount(
            SettlementNetwork::Nano,
            Some(&nano),
            "2099999999999999999999999999",
            2625,
            "m",
        )
        .unwrap_err();
        assert!(msg(err).contains("insufficient"));
        // Non-integer / negative / float raw is a 400.
        for bad in ["1.5", "-1", "abc", ""] {
            assert!(
                validate_client_amount(SettlementNetwork::Nano, Some(&nano), bad, 2625, "m")
                    .is_err(),
                "{bad:?}"
            );
        }
        // Nano without config is an internal fail-closed error, never a pass.
        assert!(matches!(
            validate_client_amount(SettlementNetwork::Nano, None, exact_raw, 2625, "m"),
            Err(GatewayError::Internal(_))
        ));
    }

    #[test]
    fn payload_variant_must_match_network() {
        let nano = PayloadData::Nano(NanoPayload {
            block_hash: "AB".repeat(32),
            account: PAYER.to_string(),
        });
        let direct = PayloadData::Direct(SolanaPayload {
            transaction: "dGVzdA==".to_string(),
        });
        assert!(validate_payload_matches_network(SettlementNetwork::Nano, &nano).is_ok());
        assert!(validate_payload_matches_network(SettlementNetwork::Solana, &direct).is_ok());
        assert!(validate_payload_matches_network(SettlementNetwork::Nano, &direct).is_err());
        assert!(validate_payload_matches_network(SettlementNetwork::Solana, &nano).is_err());
    }

    #[test]
    fn nano_accept_quotes_raw_rounded_up_at_the_configured_rate() {
        let a = nano_accept();
        assert_eq!(a.scheme, "exact");
        assert_eq!(a.network, "nano:mainnet");
        assert_eq!(a.asset, "XNO");
        assert_eq!(a.pay_to, NANO_ACCT);
        assert_eq!(a.amount, "2100000000000000000000000000");
        assert_eq!(a.max_timeout_seconds, MAX_TIMEOUT_SECONDS);
        assert!(a.escrow_program_id.is_none());
        // $1 at $3/XNO rounds UP (never under-collects).
        let nano = NanoConfig {
            micro_usd_per_xno: 3_000_000,
            ..nano_config()
        };
        let raw: u128 = nano_payment_accept(&nano, "1000000")
            .unwrap()
            .amount
            .parse()
            .unwrap();
        assert!(raw * 3 >= solvela_protocol::RAW_PER_XNO);
        // Garbage quote → no nano entry (Solana-only challenge), never a panic.
        assert!(nano_payment_accept(&nano, "abc").is_none());
    }
}

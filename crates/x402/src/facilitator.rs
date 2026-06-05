use std::sync::Arc;

use tracing::info;

use crate::traits::{Error, PaymentVerifier};
use crate::types::{PaymentPayload, SettlementResult, VerificationResult};

/// The facilitator service coordinates payment verification and settlement.
///
/// It dispatches to the appropriate `PaymentVerifier` implementation based
/// on the network specified in the payment payload. Currently Solana-only,
/// designed for future multi-chain support.
pub struct Facilitator {
    verifiers: Vec<Arc<dyn PaymentVerifier>>,
}

impl Facilitator {
    /// Create a new facilitator with the given payment verifiers.
    pub fn new(verifiers: Vec<Arc<dyn PaymentVerifier>>) -> Self {
        Self { verifiers }
    }

    /// Find the verifier for a given network and scheme combination.
    fn verifier_for(
        &self,
        network: &str,
        scheme: &str,
    ) -> Result<&Arc<dyn PaymentVerifier>, Error> {
        self.verifiers
            .iter()
            .find(|v| v.network() == network && v.scheme() == scheme)
            .ok_or_else(|| Error::UnsupportedNetwork(format!("{network}/{scheme}")))
    }

    /// Verify a payment payload.
    pub async fn verify(&self, payload: &PaymentPayload) -> Result<VerificationResult, Error> {
        let network = &payload.accepted.network;
        let scheme = &payload.accepted.scheme;
        info!(network, scheme, "routing verification to chain verifier");

        let verifier = self.verifier_for(network, scheme)?;
        verifier.verify_payment(payload).await
    }

    /// Settle a previously-verified payment by broadcasting it on-chain.
    ///
    /// This does NOT re-run verification — the caller is responsible for having
    /// already called [`Facilitator::verify`] and confirmed the payload is
    /// valid. It exists so the gateway can DEFER the on-chain broadcast of an
    /// `exact` transfer until AFTER a successful provider response: verify
    /// up front (non-mutating, simulates success), call the provider, then
    /// settle only if the provider delivered — so a provider failure never
    /// charges the customer (issue #486). For the verify-then-settle-now case
    /// (e.g. the `escrow` deposit, which must land before serving), use
    /// [`Facilitator::verify_and_settle`].
    pub async fn settle(&self, payload: &PaymentPayload) -> Result<SettlementResult, Error> {
        let network = &payload.accepted.network;
        let scheme = &payload.accepted.scheme;
        info!(
            network,
            scheme, "routing deferred settlement to chain verifier"
        );

        let verifier = self.verifier_for(network, scheme)?;
        verifier.settle_payment(payload).await
    }

    /// Verify and then settle a payment.
    pub async fn verify_and_settle(
        &self,
        payload: &PaymentPayload,
    ) -> Result<SettlementResult, Error> {
        let network = &payload.accepted.network;
        let scheme = &payload.accepted.scheme;
        info!(network, scheme, "routing settlement to chain verifier");

        let verifier = self.verifier_for(network, scheme)?;

        // Verify first
        let verification = verifier.verify_payment(payload).await?;
        if !verification.valid {
            return Err(Error::InvalidTransaction(
                verification
                    .reason
                    .unwrap_or_else(|| "verification failed".to_string()),
            ));
        }

        // Then settle
        verifier.settle_payment(payload).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PayloadData, PaymentAccept, Resource, SolanaPayload, SOLANA_NETWORK};

    /// A mock verifier for testing the facilitator dispatch logic.
    struct MockVerifier;

    #[async_trait::async_trait]
    impl PaymentVerifier for MockVerifier {
        fn network(&self) -> &str {
            SOLANA_NETWORK
        }

        fn scheme(&self) -> &str {
            "exact"
        }

        async fn verify_payment(
            &self,
            _payload: &PaymentPayload,
        ) -> Result<VerificationResult, Error> {
            Ok(VerificationResult {
                valid: true,
                reason: None,
                verified_amount: Some(1000),
            })
        }

        async fn settle_payment(
            &self,
            _payload: &PaymentPayload,
        ) -> Result<SettlementResult, Error> {
            Ok(SettlementResult {
                success: true,
                tx_signature: Some("MockTxSig123".to_string()),
                network: SOLANA_NETWORK.to_string(),
                error: None,
                verified_amount: None,
                failure_kind: None,
            })
        }
    }

    fn make_test_payload() -> PaymentPayload {
        PaymentPayload {
            x402_version: 2,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: PaymentAccept {
                scheme: "exact".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "1000".to_string(),
                asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                pay_to: "RecipientPubkey".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload: PayloadData::Direct(SolanaPayload {
                transaction: "base64encodedtx".to_string(),
            }),
        }
    }

    #[tokio::test]
    async fn test_facilitator_verify() {
        let facilitator = Facilitator::new(vec![Arc::new(MockVerifier)]);
        let payload = make_test_payload();

        let result = facilitator.verify(&payload).await;
        assert!(result.is_ok());
        assert!(result.unwrap().valid);
    }

    #[tokio::test]
    async fn test_facilitator_verify_and_settle() {
        let facilitator = Facilitator::new(vec![Arc::new(MockVerifier)]);
        let payload = make_test_payload();

        let result = facilitator.verify_and_settle(&payload).await;
        assert!(result.is_ok());

        let settlement = result.unwrap();
        assert!(settlement.success);
        assert_eq!(settlement.tx_signature, Some("MockTxSig123".to_string()));
    }

    /// A verifier that records whether `verify_payment` was invoked, so a test
    /// can prove `settle()` broadcasts WITHOUT re-verifying — the property the
    /// gateway relies on to defer the `exact` broadcast until after the provider
    /// delivers (#486). Its `verify_payment` would also REJECT (valid:false), so
    /// if `settle()` mistakenly re-verified, settlement would not be reached.
    struct VerifyTrackingVerifier {
        verified: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait::async_trait]
    impl PaymentVerifier for VerifyTrackingVerifier {
        fn network(&self) -> &str {
            SOLANA_NETWORK
        }

        fn scheme(&self) -> &str {
            "exact"
        }

        async fn verify_payment(
            &self,
            _payload: &PaymentPayload,
        ) -> Result<VerificationResult, Error> {
            self.verified
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(VerificationResult {
                valid: false,
                reason: Some("would reject if called".to_string()),
                verified_amount: None,
            })
        }

        async fn settle_payment(
            &self,
            _payload: &PaymentPayload,
        ) -> Result<SettlementResult, Error> {
            Ok(SettlementResult {
                success: true,
                tx_signature: Some("DeferredSettleSig".to_string()),
                network: SOLANA_NETWORK.to_string(),
                error: None,
                verified_amount: None,
                failure_kind: None,
            })
        }
    }

    /// `settle()` must broadcast WITHOUT calling `verify_payment` — it is the
    /// deferred-settlement primitive for the #486 fix (verify up front, call the
    /// provider, then settle only on delivery).
    #[tokio::test]
    async fn settle_does_not_reverify() {
        let verified = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let facilitator = Facilitator::new(vec![Arc::new(VerifyTrackingVerifier {
            verified: std::sync::Arc::clone(&verified),
        })]);
        let payload = make_test_payload();

        let result = facilitator.settle(&payload).await;
        assert!(result.is_ok(), "settle should broadcast");
        assert!(
            result.unwrap().success,
            "settle must reach settle_payment even though verify_payment would reject"
        );
        assert!(
            !verified.load(std::sync::atomic::Ordering::SeqCst),
            "settle() must NOT call verify_payment — verification is the caller's responsibility"
        );
    }

    /// Contrast: `verify_and_settle()` DOES verify first — and here verification
    /// rejects, so settlement must NOT be reached and an error is returned.
    #[tokio::test]
    async fn verify_and_settle_rejects_when_verification_fails() {
        let verified = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let facilitator = Facilitator::new(vec![Arc::new(VerifyTrackingVerifier {
            verified: std::sync::Arc::clone(&verified),
        })]);
        let payload = make_test_payload();

        let result = facilitator.verify_and_settle(&payload).await;
        assert!(
            result.is_err(),
            "verify_and_settle must propagate a failed verification as an error"
        );
        assert!(
            verified.load(std::sync::atomic::Ordering::SeqCst),
            "verify_and_settle must have called verify_payment"
        );
    }

    #[tokio::test]
    async fn test_facilitator_unsupported_network() {
        let facilitator = Facilitator::new(vec![Arc::new(MockVerifier)]);
        let mut payload = make_test_payload();
        payload.accepted.network = "ethereum:1".to_string();

        let result = facilitator.verify(&payload).await;
        assert!(result.is_err());
    }
}

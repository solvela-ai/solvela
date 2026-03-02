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

    #[tokio::test]
    async fn test_facilitator_unsupported_network() {
        let facilitator = Facilitator::new(vec![Arc::new(MockVerifier)]);
        let mut payload = make_test_payload();
        payload.accepted.network = "ethereum:1".to_string();

        let result = facilitator.verify(&payload).await;
        assert!(result.is_err());
    }
}

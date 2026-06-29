//! v0 off-chain spend-down channel — the pure voucher core.
//!
//! "Fund once, draw down." After a one-time on-chain-verified deposit credits a
//! channel's `deposited_atomic`, the agent draws the channel down off-chain by
//! presenting a sequence of monotonic **cumulative vouchers**: each voucher is an
//! ed25519 signature (by the channel's `session_key`) over a canonical message
//! carrying the running fee-inclusive total. The gateway verifies each voucher
//! locally — no RPC, <10ms — and serves the request. Settlement (batch, on-chain
//! in the end-state) redeems the highest voucher; the fee is already baked into
//! the cumulative at the single fee site, so settlement is fee-blind.
//!
//! This module is the **isolated financial core**: pure functions, no DB, no
//! HTTP, no fee math. It owns exactly two things:
//!
//! 1. [`build_voucher_message`] — the byte-exact canonical message the agent
//!    signs and the gateway re-derives. Golden-vector-pinned ([`GOLDEN_VECTOR_B64`]),
//!    the same drift-guard discipline as `solvela-escrow-tx`'s deposit builder.
//! 2. [`verify_voucher`] — every accept rule from the channel scope §2, each a
//!    distinct typed [`ChannelVoucherError`], **all fail-closed**: a corrupt or
//!    unverifiable voucher is a hard error, never a silent `$0` draw. All amounts
//!    are `u64` atomic micro-USDC with checked arithmetic (the draw delta must
//!    never underflow/wrap).
//!
//! The wire/header wrapping, the `PaymentScheme::Channel` enum branch, the DB
//! query layer, and the SDK signer are deliberately NOT here — they are later
//! increments. This core is what they all build on.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

// ---------------------------------------------------------------------------
// Canonical message layout (byte-exact, golden-vector-pinned)
// ---------------------------------------------------------------------------

/// Domain-separation prefix for the channel-voucher signed message. Versioned so
/// a future message-layout change is a new tag, never an ambiguous re-parse of an
/// old signature. Binds a signature to the channel-voucher use, distinct from any
/// other ed25519 message the same key might sign.
pub const DOMAIN_SEP: &[u8] = b"solvela-channel-voucher-v1";

/// Total length of the canonical voucher message in bytes. Fully
/// compile-time-known:
/// `DOMAIN_SEP || channel_id(32) || cumulative(8) || expiry(8) || nonce(8) ||
/// request_digest(32)`. A layout regression fails the BUILD here, not at request
/// time.
const VOUCHER_MESSAGE_LEN: usize = DOMAIN_SEP.len() + 32 + 8 + 8 + 8 + 32;
const _: () = assert!(VOUCHER_MESSAGE_LEN == 114);

/// Byte-exact base64 (standard alphabet) of [`build_voucher_message`] for a fixed
/// input. Pins the voucher-message wire layout so any change to `DOMAIN_SEP`, the
/// field order, or the little-endian encoding breaks the build. Computed
/// independently from the spec byte layout (not from this function's output) and
/// asserted by `voucher_message_matches_golden_vector`.
///
/// Fixed input: `channel_id = [0x11; 32]`, `cumulative_amount = 2_625`,
/// `expiry_slot = 1_000_750`, `nonce = 42`, `request_digest = [0x22; 32]`.
///
/// DO NOT hand-edit. If this changes, the voucher layout changed — and every SDK
/// signer + the gateway verifier must agree on the new bytes before it is pinned.
pub const GOLDEN_VECTOR_B64: &str =
    "c29sdmVsYS1jaGFubmVsLXZvdWNoZXItdjEREREREREREREREREREREREREREREREREREREREREREUEKAAAAAAAALkUPAAAAAAAqAAAAAAAAACIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIi";

/// Slot margin a voucher must retain before its `expiry_slot` to be accepted.
/// Mirrors the escrow `MIN_EXPIRY_BUFFER_SLOTS` convention
/// (`crates/x402/src/escrow/verifier.rs`): the gateway never accepts an
/// about-to-expire voucher it could not settle in time.
pub const MIN_EXPIRY_BUFFER_SLOTS: u64 = 50;

/// Build the canonical voucher message the agent signs and the gateway
/// re-derives, byte-for-byte:
///
/// ```text
/// DOMAIN_SEP
///   || channel_id        : [u8; 32]
///   || cumulative_amount : u64 little-endian   (atomic micro-USDC)
///   || expiry_slot       : u64 little-endian
///   || nonce             : u64 little-endian
///   || request_digest    : [u8; 32]            (SHA-256 of the canonical request)
/// ```
///
/// Infallible — pure concatenation. Both the SDK signer (later increment) and
/// [`verify_voucher`] go through this single function, so they can never drift;
/// that is exactly the golden-vector contract ([`GOLDEN_VECTOR_B64`]).
pub fn build_voucher_message(
    channel_id: &[u8; 32],
    cumulative_amount: u64,
    expiry_slot: u64,
    nonce: u64,
    request_digest: &[u8; 32],
) -> Vec<u8> {
    let mut msg = Vec::with_capacity(VOUCHER_MESSAGE_LEN);
    msg.extend_from_slice(DOMAIN_SEP);
    msg.extend_from_slice(channel_id);
    msg.extend_from_slice(&cumulative_amount.to_le_bytes());
    msg.extend_from_slice(&expiry_slot.to_le_bytes());
    msg.extend_from_slice(&nonce.to_le_bytes());
    msg.extend_from_slice(request_digest);
    msg
}

// ---------------------------------------------------------------------------
// Voucher + channel-state inputs
// ---------------------------------------------------------------------------

/// A cumulative spend voucher presented on a paid request. All fields except
/// `signature` are covered by the ed25519 signature via [`build_voucher_message`].
///
/// Carries no secret material (the signature and digest are public), so it
/// derives `Debug` directly.
#[derive(Debug, Clone)]
pub struct Voucher {
    /// The channel this voucher draws against (must match the channel state).
    pub channel_id: [u8; 32],
    /// Signed running total of all fee-inclusive call costs on this channel
    /// (atomic micro-USDC), monotonic non-decreasing.
    pub cumulative_atomic: u64,
    /// Slot at which the voucher expires.
    pub expiry_slot: u64,
    /// Per-draw nonce (audit / request binding).
    pub nonce: u64,
    /// SHA-256 of the canonical request body this voucher pays for.
    pub request_digest: [u8; 32],
    /// ed25519 signature over [`build_voucher_message`], verified vs the
    /// channel's `session_key`.
    pub signature: [u8; 64],
}

/// The verifier's view of a channel for a single draw: the persistent channel
/// state plus the request context this draw must bind to. Plain input struct, no
/// DB handle. `session_key` is a public ed25519 key, not a secret.
#[derive(Debug, Clone)]
pub struct ChannelState {
    /// The channel this draw is for (guards cross-channel replay).
    pub channel_id: [u8; 32],
    /// On-chain-verified deposited principal — the hard ceiling on `cumulative`.
    pub deposited_atomic: u64,
    /// Cumulative value already settled — `cumulative` must not regress below it.
    pub settled_atomic: u64,
    /// Highest cumulative accepted so far; the next draw delta is measured from
    /// here.
    pub last_cumulative_atomic: u64,
    /// ed25519 pubkey vouchers are verified against.
    pub session_key: [u8; 32],
    /// SHA-256 of the request actually being served; the voucher's
    /// `request_digest` must equal this.
    pub expected_request_digest: [u8; 32],
}

// ---------------------------------------------------------------------------
// Error type — every accept rule is its own fail-closed variant
// ---------------------------------------------------------------------------

/// Why a voucher was rejected. Every variant is a hard refusal — there is no
/// silent `$0`/`unwrap_or_default` path on the draw. Variants map 1:1 to the
/// channel scope §2 accept rules.
#[derive(Debug, thiserror::Error)]
pub enum ChannelVoucherError {
    /// The voucher's `channel_id` does not match the channel being drawn.
    #[error("voucher channel_id does not match the channel being drawn")]
    ChannelMismatch,
    /// The ed25519 signature did not verify against the channel's `session_key`
    /// (also covers a malformed session-key point).
    #[error("voucher signature is invalid for the channel session key")]
    InvalidSignature,
    /// The voucher's `request_digest` does not match the request being served.
    #[error("voucher request_digest does not match the served request")]
    RequestDigestMismatch,
    /// The voucher expires within [`MIN_EXPIRY_BUFFER_SLOTS`] (or has already
    /// expired): too close to settle in time.
    #[error(
        "voucher expiry_slot {expiry_slot} too close to current slot {current_slot} \
         (buffer {buffer} < required {MIN_EXPIRY_BUFFER_SLOTS})"
    )]
    Expired {
        /// The voucher's claimed expiry slot.
        expiry_slot: u64,
        /// The current slot supplied to the verifier.
        current_slot: u64,
        /// Remaining slots before expiry (saturating; 0 if already expired).
        buffer: u64,
    },
    /// `cumulative` is lower than `last_cumulative` — a stale/replayed voucher.
    /// Caught by checked subtraction (the delta must never underflow/wrap).
    #[error("voucher cumulative {cumulative} is below the last accepted {last_cumulative}")]
    NonMonotonicCumulative {
        /// The voucher's cumulative.
        cumulative: u64,
        /// The channel's last accepted cumulative.
        last_cumulative: u64,
    },
    /// The draw delta (`cumulative - last_cumulative`) does not equal the amount
    /// billed for this call.
    #[error("voucher draw delta {actual_delta} != billed {expected_billed}")]
    DeltaMismatch {
        /// The amount the gateway billed for this call.
        expected_billed: u64,
        /// The voucher's actual cumulative delta.
        actual_delta: u64,
    },
    /// `cumulative` regressed below the already-settled amount.
    #[error("voucher cumulative {cumulative} is below settled {settled}")]
    BelowSettled {
        /// The voucher's cumulative.
        cumulative: u64,
        /// The channel's settled amount.
        settled: u64,
    },
    /// `cumulative` exceeds the on-chain-verified deposited principal (over-draw).
    #[error("voucher cumulative {cumulative} exceeds deposited {deposited}")]
    OverDraw {
        /// The voucher's cumulative.
        cumulative: u64,
        /// The channel's deposited principal.
        deposited: u64,
    },
}

/// Verify a draw voucher against the channel state, fail-closed on every rule.
///
/// Returns `Ok(())` iff ALL of the channel scope §2 accept rules hold:
/// - the voucher is for this channel (`channel_id` matches);
/// - the ed25519 signature verifies against `session_key`;
/// - the `request_digest` matches the request actually served;
/// - the voucher is not within [`MIN_EXPIRY_BUFFER_SLOTS`] of expiry;
/// - the draw delta (`cumulative - last_cumulative`, checked) equals `billed_atomic`;
/// - `cumulative >= settled`;
/// - `cumulative <= deposited`.
///
/// Any failure is a distinct typed [`ChannelVoucherError`] — never a silent `$0`
/// draw. The authenticity gate (signature) runs before any signed field is
/// trusted; the draw-delta subtraction is checked so a stale/lower voucher can
/// never underflow into a spurious accept.
///
/// `billed_atomic` is taken as given (the caller computes it fail-closed at the
/// single fee site — this core does no fee math). `current_slot` is the slot the
/// expiry margin is measured against.
pub fn verify_voucher(
    state: &ChannelState,
    voucher: &Voucher,
    billed_atomic: u64,
    current_slot: u64,
) -> Result<(), ChannelVoucherError> {
    // 1. Structural: the voucher must be for THIS channel. Rejects a
    //    cross-channel replay before we trust any signed field.
    if voucher.channel_id != state.channel_id {
        return Err(ChannelVoucherError::ChannelMismatch);
    }

    // 2. Authenticity: ed25519 over the canonical message vs the session key.
    //    Everything below trusts the signed fields, so this gate is first.
    let message = build_voucher_message(
        &voucher.channel_id,
        voucher.cumulative_atomic,
        voucher.expiry_slot,
        voucher.nonce,
        &voucher.request_digest,
    );
    let verifying_key = VerifyingKey::from_bytes(&state.session_key)
        .map_err(|_| ChannelVoucherError::InvalidSignature)?;
    let signature = Signature::from_bytes(&voucher.signature);
    verifying_key
        .verify(&message, &signature)
        .map_err(|_| ChannelVoucherError::InvalidSignature)?;

    // 3. Request binding: the voucher must pay for the request actually served
    //    (closes the x402/MPP replay-against-a-different-request attack class).
    if voucher.request_digest != state.expected_request_digest {
        return Err(ChannelVoucherError::RequestDigestMismatch);
    }

    // 4. Expiry: refuse a voucher too close to (or past) expiry to settle in
    //    time. Saturating so a past expiry yields buffer 0 (rejected), never
    //    wraps — mirrors the escrow expiry-buffer check exactly.
    let buffer = voucher.expiry_slot.saturating_sub(current_slot);
    if buffer < MIN_EXPIRY_BUFFER_SLOTS {
        return Err(ChannelVoucherError::Expired {
            expiry_slot: voucher.expiry_slot,
            current_slot,
            buffer,
        });
    }

    // 5. Monotonic draw delta — CHECKED. A lower-or-equal-than-last cumulative
    //    underflows here, so a stale/replayed voucher is a hard error, never a
    //    silent $0 (or wrapped huge) draw.
    let delta = voucher
        .cumulative_atomic
        .checked_sub(state.last_cumulative_atomic)
        .ok_or(ChannelVoucherError::NonMonotonicCumulative {
            cumulative: voucher.cumulative_atomic,
            last_cumulative: state.last_cumulative_atomic,
        })?;
    if delta != billed_atomic {
        return Err(ChannelVoucherError::DeltaMismatch {
            expected_billed: billed_atomic,
            actual_delta: delta,
        });
    }

    // 6. Settled floor: cumulative must never regress below the settled amount.
    //    (Guaranteed when the invariant settled <= last holds; kept as a hard
    //    guard against corrupt state — defense in depth.)
    if voucher.cumulative_atomic < state.settled_atomic {
        return Err(ChannelVoucherError::BelowSettled {
            cumulative: voucher.cumulative_atomic,
            settled: state.settled_atomic,
        });
    }

    // 7. Deposit ceiling: the agent can never draw more than it funded on-chain.
    if voucher.cumulative_atomic > state.deposited_atomic {
        return Err(ChannelVoucherError::OverDraw {
            cumulative: voucher.cumulative_atomic,
            deposited: state.deposited_atomic,
        });
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};

    // Fixed golden-vector input (mirrors the GOLDEN_VECTOR_B64 doc comment).
    const GOLDEN_CHANNEL_ID: [u8; 32] = [0x11; 32];
    const GOLDEN_CUMULATIVE: u64 = 2_625;
    const GOLDEN_EXPIRY: u64 = 1_000_750;
    const GOLDEN_NONCE: u64 = 42;
    const GOLDEN_REQUEST_DIGEST: [u8; 32] = [0x22; 32];

    /// Deterministic signing key for voucher tests (seed `[7u8; 32]`).
    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    /// Sign a voucher with `key` over the canonical message and assemble a
    /// [`Voucher`]. The happy-path fields are chosen so the draw delta equals
    /// `billed`.
    fn signed_voucher(
        key: &SigningKey,
        channel_id: [u8; 32],
        cumulative: u64,
        expiry_slot: u64,
        nonce: u64,
        request_digest: [u8; 32],
    ) -> Voucher {
        let msg =
            build_voucher_message(&channel_id, cumulative, expiry_slot, nonce, &request_digest);
        let signature = key.sign(&msg).to_bytes();
        Voucher {
            channel_id,
            cumulative_atomic: cumulative,
            expiry_slot,
            nonce,
            request_digest,
            signature,
        }
    }

    /// A channel state whose `session_key` is `key`'s pubkey and that accepts a
    /// draw of `billed` starting from `last`.
    fn state_for(
        key: &SigningKey,
        channel_id: [u8; 32],
        deposited: u64,
        settled: u64,
        last: u64,
        request_digest: [u8; 32],
    ) -> ChannelState {
        ChannelState {
            channel_id,
            deposited_atomic: deposited,
            settled_atomic: settled,
            last_cumulative_atomic: last,
            session_key: key.verifying_key().to_bytes(),
            expected_request_digest: request_digest,
        }
    }

    // -- build_voucher_message: byte-exact golden vector --------------------

    #[test]
    fn voucher_message_matches_golden_vector() {
        let msg = build_voucher_message(
            &GOLDEN_CHANNEL_ID,
            GOLDEN_CUMULATIVE,
            GOLDEN_EXPIRY,
            GOLDEN_NONCE,
            &GOLDEN_REQUEST_DIGEST,
        );
        let expected = base64::engine::general_purpose::STANDARD
            .decode(GOLDEN_VECTOR_B64)
            .expect("golden vector must be valid base64");
        assert_eq!(
            msg, expected,
            "voucher message bytes drifted from the pinned golden vector"
        );
        assert_eq!(msg.len(), VOUCHER_MESSAGE_LEN, "message length must be 114");
        assert!(
            msg.starts_with(DOMAIN_SEP),
            "message must start with DOMAIN_SEP"
        );
    }

    // -- happy path ---------------------------------------------------------

    #[test]
    fn happy_path_valid_voucher_accepts() {
        let key = test_signing_key();
        let billed: u64 = 2_100;
        let last: u64 = 10_500;
        let cumulative = last + billed; // 12_600
        let digest = [0x33; 32];
        let voucher = signed_voucher(&key, [9u8; 32], cumulative, 1_000_000, 1, digest);
        let state = state_for(&key, [9u8; 32], 50_000, 0, last, digest);

        assert!(
            verify_voucher(&state, &voucher, billed, 900_000).is_ok(),
            "a correctly-signed, in-bounds voucher must be accepted"
        );
    }

    // -- over-draw: cumulative > deposited ----------------------------------

    #[test]
    fn over_draw_rejected() {
        let key = test_signing_key();
        let billed: u64 = 2_000;
        let last: u64 = 0;
        let cumulative = last + billed; // delta matches billed
        let digest = [0x33; 32];
        let voucher = signed_voucher(&key, [9u8; 32], cumulative, 1_000_000, 1, digest);
        // deposited (1_000) is below cumulative (2_000).
        let state = state_for(&key, [9u8; 32], 1_000, 0, last, digest);

        assert!(matches!(
            verify_voucher(&state, &voucher, billed, 900_000),
            Err(ChannelVoucherError::OverDraw {
                cumulative: 2_000,
                deposited: 1_000
            })
        ));
    }

    // -- stale/replay: lower cumulative -------------------------------------

    #[test]
    fn stale_lower_cumulative_rejected() {
        let key = test_signing_key();
        let last: u64 = 5_000;
        let cumulative: u64 = 3_000; // below last → non-monotonic
        let digest = [0x33; 32];
        let voucher = signed_voucher(&key, [9u8; 32], cumulative, 1_000_000, 1, digest);
        let state = state_for(&key, [9u8; 32], 50_000, 0, last, digest);

        assert!(matches!(
            verify_voucher(&state, &voucher, 1_000, 900_000),
            Err(ChannelVoucherError::NonMonotonicCumulative {
                cumulative: 3_000,
                last_cumulative: 5_000
            })
        ));
    }

    // -- delta mismatch: cumulative - last != billed ------------------------

    #[test]
    fn delta_mismatch_rejected() {
        let key = test_signing_key();
        let last: u64 = 1_000;
        let cumulative: u64 = 2_000; // delta = 1_000
        let billed: u64 = 500; // but billed is 500
        let digest = [0x33; 32];
        let voucher = signed_voucher(&key, [9u8; 32], cumulative, 1_000_000, 1, digest);
        let state = state_for(&key, [9u8; 32], 50_000, 0, last, digest);

        assert!(matches!(
            verify_voucher(&state, &voucher, billed, 900_000),
            Err(ChannelVoucherError::DeltaMismatch {
                expected_billed: 500,
                actual_delta: 1_000
            })
        ));
    }

    // -- wrong request_digest -----------------------------------------------

    #[test]
    fn wrong_request_digest_rejected() {
        let key = test_signing_key();
        let billed: u64 = 1_000;
        let last: u64 = 0;
        let cumulative = last + billed;
        let signed_digest = [0x33; 32];
        let voucher = signed_voucher(&key, [9u8; 32], cumulative, 1_000_000, 1, signed_digest);
        // The channel served a DIFFERENT request than the voucher was signed for.
        let served_digest = [0x44; 32];
        let state = state_for(&key, [9u8; 32], 50_000, 0, last, served_digest);

        assert!(matches!(
            verify_voucher(&state, &voucher, billed, 900_000),
            Err(ChannelVoucherError::RequestDigestMismatch)
        ));
    }

    // -- expired within the buffer ------------------------------------------

    #[test]
    fn expiry_within_buffer_rejected() {
        let key = test_signing_key();
        let billed: u64 = 1_000;
        let last: u64 = 0;
        let cumulative = last + billed;
        let digest = [0x33; 32];
        let expiry: u64 = 1_000_000;
        let voucher = signed_voucher(&key, [9u8; 32], cumulative, expiry, 1, digest);
        let state = state_for(&key, [9u8; 32], 50_000, 0, last, digest);

        // current_slot leaves only 49 slots (< MIN_EXPIRY_BUFFER_SLOTS = 50).
        let current_slot = expiry - (MIN_EXPIRY_BUFFER_SLOTS - 1);
        assert!(matches!(
            verify_voucher(&state, &voucher, billed, current_slot),
            Err(ChannelVoucherError::Expired { buffer: 49, .. })
        ));

        // Exactly at the buffer boundary (50 slots remaining) is accepted.
        let at_boundary = expiry - MIN_EXPIRY_BUFFER_SLOTS;
        assert!(verify_voucher(&state, &voucher, billed, at_boundary).is_ok());
    }

    // -- bad signature ------------------------------------------------------

    #[test]
    fn bad_signature_rejected() {
        let key = test_signing_key();
        let billed: u64 = 1_000;
        let last: u64 = 0;
        let cumulative = last + billed;
        let digest = [0x33; 32];
        let mut voucher = signed_voucher(&key, [9u8; 32], cumulative, 1_000_000, 1, digest);
        // Flip a signature byte — must no longer verify.
        voucher.signature[0] ^= 0xFF;
        let state = state_for(&key, [9u8; 32], 50_000, 0, last, digest);

        assert!(matches!(
            verify_voucher(&state, &voucher, billed, 900_000),
            Err(ChannelVoucherError::InvalidSignature)
        ));
    }

    #[test]
    fn signature_from_other_key_rejected() {
        let signer = test_signing_key();
        let billed: u64 = 1_000;
        let last: u64 = 0;
        let cumulative = last + billed;
        let digest = [0x33; 32];
        let voucher = signed_voucher(&signer, [9u8; 32], cumulative, 1_000_000, 1, digest);
        // Channel's session_key is a DIFFERENT key than the one that signed.
        let other = SigningKey::from_bytes(&[8u8; 32]);
        let state = state_for(&other, [9u8; 32], 50_000, 0, last, digest);

        assert!(matches!(
            verify_voucher(&state, &voucher, billed, 900_000),
            Err(ChannelVoucherError::InvalidSignature)
        ));
    }

    // -- checked-arithmetic underflow guard ---------------------------------

    #[test]
    fn checked_delta_does_not_underflow() {
        let key = test_signing_key();
        // last = u64::MAX, cumulative = 0: an unchecked `cumulative - last` would
        // wrap to a huge value and could spuriously match a crafted `billed`.
        // The checked subtraction must surface NonMonotonicCumulative instead.
        let last: u64 = u64::MAX;
        let cumulative: u64 = 0;
        let digest = [0x33; 32];
        let voucher = signed_voucher(&key, [9u8; 32], cumulative, 1_000_000, 1, digest);
        let state = state_for(&key, [9u8; 32], u64::MAX, 0, last, digest);

        // Even with billed chosen as the wrapped delta, it must NOT accept.
        let wrapped = 0u64.wrapping_sub(u64::MAX); // == 1
        assert!(matches!(
            verify_voucher(&state, &voucher, wrapped, 900_000),
            Err(ChannelVoucherError::NonMonotonicCumulative {
                cumulative: 0,
                last_cumulative: u64::MAX
            })
        ));
    }

    // -- channel mismatch (cross-channel replay) ----------------------------

    #[test]
    fn channel_mismatch_rejected() {
        let key = test_signing_key();
        let billed: u64 = 1_000;
        let last: u64 = 0;
        let cumulative = last + billed;
        let digest = [0x33; 32];
        let voucher = signed_voucher(&key, [1u8; 32], cumulative, 1_000_000, 1, digest);
        // State is for a DIFFERENT channel than the voucher.
        let state = state_for(&key, [2u8; 32], 50_000, 0, last, digest);

        assert!(matches!(
            verify_voucher(&state, &voucher, billed, 900_000),
            Err(ChannelVoucherError::ChannelMismatch)
        ));
    }

    // -- cumulative below settled (invariant guard) -------------------------

    #[test]
    fn below_settled_rejected() {
        let key = test_signing_key();
        // Contrived corrupt state (settled > last) so a valid delta still lands
        // a cumulative below settled — exercises the hard floor guard.
        let last: u64 = 0;
        let billed: u64 = 1_000;
        let cumulative = last + billed; // 1_000
        let settled: u64 = 5_000; // > cumulative
        let digest = [0x33; 32];
        let voucher = signed_voucher(&key, [9u8; 32], cumulative, 1_000_000, 1, digest);
        let state = state_for(&key, [9u8; 32], 50_000, settled, last, digest);

        assert!(matches!(
            verify_voucher(&state, &voucher, billed, 900_000),
            Err(ChannelVoucherError::BelowSettled {
                cumulative: 1_000,
                settled: 5_000
            })
        ));
    }
}

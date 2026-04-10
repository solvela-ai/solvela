package rcr

import (
	"bytes"
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/binary"
	"errors"
	"fmt"
	"runtime"

	"filippo.io/edwards25519"
	"github.com/mr-tron/base58"
)

// ---------------------------------------------------------------------------
// Solana program ID constants (base58-encoded)
// ---------------------------------------------------------------------------

const (
	SystemProgramIDB58            = "11111111111111111111111111111111"
	TokenProgramIDB58             = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
	// CORRECT spelling — last 5 chars are A8knL, NOT e1bxs.
	// Typo historical bug; see PR #12.
	AssociatedTokenProgramIDB58 = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
)

// USDCDecimals is the fixed-point decimals value for USDC-SPL on Solana.
const USDCDecimals uint8 = 6

// ---------------------------------------------------------------------------
// Base58 helpers
// ---------------------------------------------------------------------------

// base58DecodePubkey decodes a base58-encoded Solana public key into a fixed
// 32-byte array. Returns an error if the input is not valid base58 or not
// exactly 32 bytes long.
func base58DecodePubkey(s string) ([32]byte, error) {
	var out [32]byte
	raw, err := base58.Decode(s)
	if err != nil {
		return out, fmt.Errorf("invalid base58: %w", err)
	}
	if len(raw) != 32 {
		return out, fmt.Errorf("pubkey must be 32 bytes, got %d", len(raw))
	}
	copy(out[:], raw)
	return out, nil
}

// base58EncodePubkey encodes a 32-byte public key as base58.
func base58EncodePubkey(key [32]byte) string {
	return base58.Encode(key[:])
}

// ---------------------------------------------------------------------------
// ed25519 curve check
// ---------------------------------------------------------------------------

// isOnEd25519Curve reports whether a candidate 32-byte value is a valid
// compressed point on the ed25519 curve. Used to reject PDA candidates that
// would collide with a normal wallet keypair.
func isOnEd25519Curve(candidate [32]byte) bool {
	_, err := new(edwards25519.Point).SetBytes(candidate[:])
	return err == nil
}

// ---------------------------------------------------------------------------
// PDA derivation
// ---------------------------------------------------------------------------

// findProgramAddress derives a Solana Program Derived Address, mirroring the
// Solana runtime's `Pubkey::find_program_address`. It iterates bump seeds from
// 255 down to 0, returning the first off-curve candidate along with its bump.
//
// Hash input order: seeds || bump || program_id || "ProgramDerivedAddress".
// Note: the program id comes BEFORE the PDA marker, not after.
func findProgramAddress(seeds [][]byte, programID [32]byte) ([32]byte, uint8, error) {
	var result [32]byte
	for bump := 255; bump >= 0; bump-- {
		h := sha256.New()
		for _, seed := range seeds {
			h.Write(seed)
		}
		h.Write([]byte{byte(bump)})
		h.Write(programID[:])
		h.Write([]byte("ProgramDerivedAddress"))
		sum := h.Sum(nil)
		var cand [32]byte
		copy(cand[:], sum)
		if !isOnEd25519Curve(cand) {
			return cand, uint8(bump), nil
		}
	}
	return result, 0, errors.New("no off-curve PDA found for provided seeds")
}

// ---------------------------------------------------------------------------
// ATA derivation
// ---------------------------------------------------------------------------

// deriveATA derives the Associated Token Account for a given wallet and mint.
// Seeds: [wallet, token_program, mint], program: associated_token_program.
func deriveATA(wallet, mint [32]byte) ([32]byte, error) {
	tokenProgram, err := base58DecodePubkey(TokenProgramIDB58)
	if err != nil {
		return [32]byte{}, fmt.Errorf("token program id: %w", err)
	}
	ataProgram, err := base58DecodePubkey(AssociatedTokenProgramIDB58)
	if err != nil {
		return [32]byte{}, fmt.Errorf("ata program id: %w", err)
	}
	pda, _, err := findProgramAddress(
		[][]byte{wallet[:], tokenProgram[:], mint[:]},
		ataProgram,
	)
	if err != nil {
		return [32]byte{}, err
	}
	return pda, nil
}

// ---------------------------------------------------------------------------
// Anchor discriminator
// ---------------------------------------------------------------------------

// anchorDiscriminator computes the first 8 bytes of sha256("global:<name>"),
// which Anchor uses to tag its program instructions.
func anchorDiscriminator(name string) [8]byte {
	sum := sha256.Sum256([]byte("global:" + name))
	var out [8]byte
	copy(out[:], sum[:8])
	return out
}

// ---------------------------------------------------------------------------
// compact-u16 encoding (Solana wire format)
// ---------------------------------------------------------------------------

// encodeCompactU16 encodes a uint16 using Solana's compact-u16 (short_vec)
// format. Up to 127 fits in one byte; larger values use 7-bit continuation.
func encodeCompactU16(v uint16) []byte {
	if v < 0x80 {
		return []byte{byte(v)}
	}
	if v < 0x4000 {
		return []byte{byte(v&0x7f) | 0x80, byte(v >> 7)}
	}
	// Max 3 bytes for uint16
	return []byte{
		byte(v&0x7f) | 0x80,
		byte((v>>7)&0x7f) | 0x80,
		byte(v >> 14),
	}
}

// ---------------------------------------------------------------------------
// u64 LE encoding
// ---------------------------------------------------------------------------

// putUint64LE writes a little-endian 8-byte representation of v into dst.
// The pointer-to-array parameter makes it structurally impossible to pass a
// too-small buffer — the compiler enforces exactly 8 bytes.
func putUint64LE(dst *[8]byte, v uint64) {
	binary.LittleEndian.PutUint64(dst[:], v)
}

// ---------------------------------------------------------------------------
// Keypair decode + integrity check (single source of truth)
// ---------------------------------------------------------------------------

// decodeAndValidateKeypair decodes a base58 64-byte Solana keypair and
// verifies that the stored pubkey at bytes [32:64] matches the pubkey
// derived by running ed25519 key generation on the seed at bytes [0:32].
// This catches corrupted or malicious keypairs where the stored pubkey
// has been swapped.
//
// A naive `priv := ed25519.PrivateKey(kpBytes); priv.Public()` is a NO-OP:
// Go's `Public()` just extracts bytes [32:64] rather than re-deriving from
// the seed. This helper is the only sanctioned place in the SDK to decode
// a keypair — `exact.go` and `escrow.go` both call it. The returned
// `ed25519.PrivateKey` is the `derived` one produced by `NewKeyFromSeed`,
// which is guaranteed consistent with the seed, NOT a cast of the raw input.
//
// Mirrors `SigningKey::from_keypair_bytes` in crates/x402/src/escrow/deposit.rs.
func decodeAndValidateKeypair(b58 string) (ed25519.PrivateKey, [32]byte, error) {
	kpBytes, err := base58.Decode(b58)
	if err != nil {
		return nil, [32]byte{}, &SigningError{Message: "invalid base58 keypair: " + err.Error(), cause: err}
	}
	if len(kpBytes) != 64 {
		return nil, [32]byte{}, &SigningError{Message: fmt.Sprintf("keypair must be 64 bytes, got %d", len(kpBytes))}
	}
	// Zero the raw input buffer on exit. `kpBytes` holds the seed half
	// [0:32] of the keypair; the returned ed25519.PrivateKey is produced
	// via NewKeyFromSeed which COPIES the seed into a fresh slice, so this
	// buffer can be scrubbed without affecting the returned value.
	// runtime.KeepAlive prevents the compiler from optimizing the zeroing
	// loop away.
	defer func() {
		for i := range kpBytes {
			kpBytes[i] = 0
		}
		runtime.KeepAlive(kpBytes)
	}()

	seed := kpBytes[:32]
	storedPub := kpBytes[32:64]

	// Re-derive the pubkey from the seed via ed25519 key generation.
	derived := ed25519.NewKeyFromSeed(seed)
	derivedPub := derived.Public().(ed25519.PublicKey)

	if !bytes.Equal(storedPub, derivedPub) {
		return nil, [32]byte{}, &SigningError{Message: "stored pubkey does not match seed-derived pubkey"}
	}

	var agentPubkey [32]byte
	copy(agentPubkey[:], derivedPub)
	return derived, agentPubkey, nil
}

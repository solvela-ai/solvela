package solvela

// Escrow deposit-transaction builder for the Go SDK.
//
// Byte-exact port of the canonical Rust builder crates/escrow-tx/src/deposit.rs
// (+ pda.rs). The output MUST reproduce GOLDEN_VECTOR_B64 for the fixed
// golden-vector input; the gateway verifier (crates/x402/src/escrow/)
// independently re-derives the same layout, so any divergence here is a
// fund-misdirection bug, not a style nit.
//
// Wire layout (single source of truth — see the Rust file and the solvela-x402
// skill):
//
//   - Escrow PDA seeds: [b"escrow", agent_pubkey(32), service_id(32)].
//   - Vault ATA: ATA(escrow_pda, usdc_mint).
//   - Instruction data (56 bytes): anchorDiscriminator("deposit")[8] ||
//     amount: u64 LE || service_id: [32] || expiry_slot: u64 LE.
//   - Account list sorted by writability; program key appended last (index 9).
//   - Instruction account-index order: [0, 4, 5, 1, 2, 3, 6, 7, 8].
//   - Legacy message header [1, 0, 6]; length prefixes are single-byte
//     compact-u16 (valid only for lengths <= 127 — reject larger rather than
//     truncating).
//   - Wire tx: compact-u16(1) || ed25519_signature(64) || message, then base64
//     (standard alphabet).
//
// Derivation primitives (PDA/ATA find_program_address, the Anchor discriminator,
// ed25519 signing) are computed with the Go standard library (crypto/ed25519,
// crypto/sha256, filippo.io/edwards25519 for on-curve checks) plus mr-tron/base58
// — the same dependency footprint as the rest of the SDK. We deliberately do NOT
// pull in a full Solana SDK: the message must match the canonical
// writability-sorted layout byte-for-byte, which a generic message builder would
// re-derive differently.

import (
	"bytes"
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"fmt"
	"io"

	"filippo.io/edwards25519"
	"github.com/mr-tron/base58"
)

// Well-known program constants (match crates/escrow-tx/src/pda.rs).
const (
	ataProgramID    = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
	tokenProgramID  = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
	systemProgramID = "11111111111111111111111111111111"

	// compactU16SingleByteMax is the largest length representable as a single
	// compact-u16 byte. Lengths above this would require a continuation byte;
	// emitting a bare byte there would corrupt the encoding, so we reject.
	compactU16SingleByteMax = 127
)

// GoldenVectorB64 is the byte-exact base64 deposit transaction for the fixed
// golden-vector input, copied verbatim from GOLDEN_VECTOR_B64 in
// crates/escrow-tx/src/deposit.rs.
//
// DO NOT hand-edit. If this changes, the deposit-tx layout changed — the
// on-chain/Rust value is ground truth. The drift-guard test
// TestGoldenVectorMatchesRustSource pins this against the Rust source file.
const GoldenVectorB64 = "Ad449R7ht12UKokgbDUYh29Ey/wDEwPioT/QtJOPKwSO4RHIaFRqS0DH+bPpEo2vCK78jbRLpP/6FV/5TY+qLw8BAAYKGX9rI+FshTLGq8g4+s1ep4m+DHaykgM0A5v6iz02jWFyvMLyv5o/k5XBGVxL9QMaEsQP6v01aK7nXazb0N/wMBS12eVticTdq8DzgM47jC/J8D1uNnFG6ZNqwpSU6GOelSnAbR4dY/56biCP6roeTxLQ8Q4TL7a2ArnBn2RW9HJ+jAiHYL/eHd3PMsF/IJuCQu5SqvEx+s2I0OosbQsG8sb6evO+2606PWXzaqvJdDGxu+TC0vbg5HymAgNFL11hBt324ddloZPZy+FGzut5rBy0he1fWzeROoz1hX7/AKmMlyWPTiSJ8bs9ECkUjg2DC1oTmdr/EIQEjnvY2+n4WQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgo61GtoIM8r3akxjdsa2N5CEHZ8QBYuAbfwPDOL78eGrq6urq6urq6urq6urq6urq6urq6urq6urq6urq6urqwEJCQAEBQECAwYHCDjyI8aJUuHytkEKAAAAAAAABwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcuRQ8AAAAAAA=="

// DepositParams holds the inputs required to build an escrow deposit
// transaction.
//
// The service_id, expirySlot, and recentBlockhash are injected so the builder
// is a pure, deterministic function (golden-vector testable). Production code
// (the escrow signer) generates the CSPRNG service_id, computes expirySlot from
// the current slot, and fetches the blockhash, then calls this builder.
//
// SECURITY: agentKeypair is raw ed25519 secret key material. It never crosses
// the package boundary (the field is unexported) and is never logged. Callers
// inside the package pass it from the wallet's private key.
type DepositParams struct {
	// agentKeypair is the 64-byte ed25519 private key (32-byte seed ||
	// 32-byte public key), matching crypto/ed25519.PrivateKey.
	agentKeypair ed25519.PrivateKey
	// ProviderWalletB58 is the base58-encoded provider wallet pubkey.
	ProviderWalletB58 string
	// USDCMintB58 is the base58-encoded USDC mint pubkey.
	USDCMintB58 string
	// EscrowProgramIDB58 is the base58-encoded escrow program ID.
	EscrowProgramIDB58 string
	// Amount is the deposit amount in atomic USDC units (must be > 0).
	Amount uint64
	// ServiceID is the 32-byte service identifier that seeds the escrow PDA.
	ServiceID [32]byte
	// ExpirySlot is the slot at which the escrow deposit expires.
	ExpirySlot uint64
	// RecentBlockhash is the 32-byte recent blockhash from getLatestBlockhash.
	RecentBlockhash [32]byte
}

// redactedDepositParams renders a DepositParams with the secret keypair
// redacted. It mirrors the Rust `impl Debug for DepositParams`
// (crates/escrow-tx/src/deposit.rs) and the Wallet.String() redaction: the raw
// ed25519 secret material in agentKeypair must NEVER appear in a log line via
// %v / %+v / %#v.
func (p DepositParams) redactedDepositParams() string {
	return fmt.Sprintf("DepositParams{agentKeypair:[REDACTED], ProviderWalletB58:%q, "+
		"USDCMintB58:%q, EscrowProgramIDB58:%q, Amount:%d, ServiceID:[32]byte{...}, "+
		"ExpirySlot:%d, RecentBlockhash:[32]byte{...}}",
		p.ProviderWalletB58, p.USDCMintB58, p.EscrowProgramIDB58, p.Amount, p.ExpirySlot)
}

// String implements fmt.Stringer so plain %s / %v formatting redacts the
// keypair.
func (p DepositParams) String() string { return p.redactedDepositParams() }

// GoString implements fmt.GoStringer so %#v formatting redacts the keypair
// (the default %#v would otherwise dump the raw secret-key bytes).
func (p DepositParams) GoString() string { return p.redactedDepositParams() }

// Format implements fmt.Formatter so every verb (%v, %+v, %#v, %s) routes
// through the redacted representation rather than reflecting over the struct
// fields. fmt consults Formatter before Stringer/GoStringer, so this is the
// authoritative guard against the keypair leaking. A pointer to DepositParams
// also satisfies fmt.Formatter via this value receiver, so *DepositParams is
// redacted too.
func (p DepositParams) Format(f fmt.State, _ rune) {
	_, _ = io.WriteString(f, p.redactedDepositParams())
}

// buildDepositTx builds a signed escrow deposit transaction, base64-encoded.
//
// Returns a base64 (standard alphabet) wire-format Solana legacy transaction
// ready for the gateway / sendTransaction.
//
// Errors (all wrapped in *SignerError, never leaking the keypair):
//   - zero amount
//   - malformed provider/mint/program address
//   - keypair of the wrong length
//   - PDA or ATA derivation failure
//   - message-too-long (a length field that would exceed the single-byte
//     compact-u16 limit)
func buildDepositTx(params *DepositParams) (string, error) {
	// Step 1: fail-closed amount guard. The amount is a uint64 on the wire, so
	// a negative/float amount is structurally impossible here (unlike the
	// dynamically-typed Python port); we still reject zero before building
	// anything — a zero-value escrow deposit is never legitimate.
	if params.Amount == 0 {
		return "", &SignerError{Message: "deposit amount must not be zero"}
	}

	// Step 2: recover the agent pubkey from the keypair. crypto/ed25519's
	// PrivateKey is seed(32) || pubkey(32); the trailing 32 bytes are the
	// public key, identical to Solana's keypair layout.
	if len(params.agentKeypair) != ed25519.PrivateKeySize {
		return "", &SignerError{Message: "invalid agent keypair: wrong length"}
	}
	// PrivateKey.Public() just copies the stored bytes [32:64]; it does NOT
	// re-derive the pubkey from the seed. Re-derive it from the seed and
	// require it to match the stored public half — mirroring the Rust guard
	// (crates/escrow-tx/src/deposit.rs). A keypair with a corrupted public half
	// would sign fine but seed the escrow PDA from the wrong agent identity,
	// producing an un-claimable deposit on-chain. Fail closed instead.
	storedPub := params.agentKeypair.Public().(ed25519.PublicKey)
	derivedPriv := ed25519.NewKeyFromSeed(params.agentKeypair.Seed())
	derivedPub := derivedPriv.Public().(ed25519.PublicKey)
	if !bytes.Equal(storedPub, derivedPub) {
		return "", &SignerError{Message: "invalid agent keypair: derived pubkey does not match stored pubkey"}
	}
	var agentPubkey [32]byte
	copy(agentPubkey[:], storedPub)

	// Step 3: parse all addresses.
	provider, err := decodeBs58Pubkey(params.ProviderWalletB58)
	if err != nil {
		return "", &SignerError{Message: fmt.Sprintf("invalid address for provider_wallet: %v", err)}
	}
	usdcMint, err := decodeBs58Pubkey(params.USDCMintB58)
	if err != nil {
		return "", &SignerError{Message: fmt.Sprintf("invalid address for usdc_mint: %v", err)}
	}
	escrowProgram, err := decodeBs58Pubkey(params.EscrowProgramIDB58)
	if err != nil {
		return "", &SignerError{Message: fmt.Sprintf("invalid address for escrow_program_id: %v", err)}
	}
	tokenProgram, err := decodeBs58Pubkey(tokenProgramID)
	if err != nil {
		return "", &SignerError{Message: fmt.Sprintf("invalid address for token_program: %v", err)}
	}
	ataProgram, err := decodeBs58Pubkey(ataProgramID)
	if err != nil {
		return "", &SignerError{Message: fmt.Sprintf("invalid address for ata_program: %v", err)}
	}
	systemProgram, err := decodeBs58Pubkey(systemProgramID)
	if err != nil {
		return "", &SignerError{Message: fmt.Sprintf("invalid address for system_program: %v", err)}
	}

	// Step 4: derive escrow PDA.
	escrowPDA, _, ok := findProgramAddress([][]byte{[]byte("escrow"), agentPubkey[:], params.ServiceID[:]}, escrowProgram)
	if !ok {
		return "", &SignerError{Message: "failed to derive escrow PDA"}
	}

	// Step 5: derive agent ATA and vault ATA.
	agentATA, ok := deriveATAAddress(agentPubkey, usdcMint)
	if !ok {
		return "", &SignerError{Message: "failed to derive agent ATA"}
	}
	vaultATA, ok := deriveATAAddress(escrowPDA, usdcMint)
	if !ok {
		return "", &SignerError{Message: "failed to derive vault ATA"}
	}

	// Step 6: account list sorted by writability (writable signer, writable
	// non-signers, readonly non-signers). The program key is appended last
	// (index 9) inside the message serializer.
	accounts := [][32]byte{
		agentPubkey,   // 0: signer, writable
		escrowPDA,     // 1: writable, non-signer
		agentATA,      // 2: writable, non-signer
		vaultATA,      // 3: writable, non-signer
		provider,      // 4: readonly
		usdcMint,      // 5: readonly
		tokenProgram,  // 6: readonly
		ataProgram,    // 7: readonly
		systemProgram, // 8: readonly
	}

	// Step 7: instruction data = discriminator || amount(LE u64) || service_id
	// || expiry_slot(LE u64) = 8 + 8 + 32 + 8 = 56 bytes.
	ixData := make([]byte, 0, 56)
	disc := anchorDiscriminator("deposit")
	ixData = append(ixData, disc[:]...)
	var amountLE [8]byte
	binary.LittleEndian.PutUint64(amountLE[:], params.Amount)
	ixData = append(ixData, amountLE[:]...)
	ixData = append(ixData, params.ServiceID[:]...)
	var expiryLE [8]byte
	binary.LittleEndian.PutUint64(expiryLE[:], params.ExpirySlot)
	ixData = append(ixData, expiryLE[:]...)
	if len(ixData) != 56 {
		return "", &SignerError{Message: fmt.Sprintf("deposit ix_data must be 56 bytes, got %d", len(ixData))}
	}

	// Anchor program account order remapped into the writability-sorted layout.
	ixAccountIndices := []byte{0, 4, 5, 1, 2, 3, 6, 7, 8}

	// Step 8: legacy message. Header [1, 0, 6]; program_id_index = 9 (last).
	msg, err := buildLegacyMessage(
		[3]byte{1, 0, 6},
		accounts,
		escrowProgram,
		params.RecentBlockhash,
		9, // program_id_index = 9 (last account)
		ixAccountIndices,
		ixData,
	)
	if err != nil {
		return "", err
	}

	// Step 9: sign the raw message bytes with the agent keypair (ed25519).
	signature := ed25519.Sign(params.agentKeypair, msg)
	if len(signature) != ed25519.SignatureSize {
		return "", &SignerError{Message: "unexpected ed25519 signature length"}
	}

	// Step 10: assemble wire tx: compact-u16(1) || signature(64) || message.
	txBytes := make([]byte, 0, 1+ed25519.SignatureSize+len(msg))
	txBytes = append(txBytes, 0x01) // compact-u16: 1 signature
	txBytes = append(txBytes, signature...)
	txBytes = append(txBytes, msg...)

	return base64.StdEncoding.EncodeToString(txBytes), nil
}

// buildLegacyMessage serializes a Solana legacy transaction message.
//
// Layout: header(3) || acct_count(u8) || N*32 keys (+ program key) ||
// blockhash(32) || ix_count(u8)=1 || program_id_index(u8) || ix_acct_count(u8)
// || ix_acct_indices || data_len(u8) || data.
//
// Length prefixes are emitted as a single byte — the correct compact-u16
// encoding only for lengths <= 127. Larger values are rejected (not truncated),
// mirroring the Rust builder.
func buildLegacyMessage(
	header [3]byte,
	accounts [][32]byte,
	programID [32]byte,
	recentBlockhash [32]byte,
	programIDIndex byte,
	ixAccountIndices []byte,
	ixData []byte,
) ([]byte, error) {
	totalAccounts := len(accounts) + 1 // +1 for the appended program key
	if totalAccounts > compactU16SingleByteMax {
		return nil, &SignerError{Message: fmt.Sprintf("account count %d exceeds the 127-byte compact-u16 single-byte limit", totalAccounts)}
	}
	if len(ixAccountIndices) > compactU16SingleByteMax {
		return nil, &SignerError{Message: fmt.Sprintf("instruction account count %d exceeds the 127-byte compact-u16 single-byte limit", len(ixAccountIndices))}
	}
	if len(ixData) > compactU16SingleByteMax {
		return nil, &SignerError{Message: fmt.Sprintf("instruction data length %d exceeds the 127-byte compact-u16 single-byte limit", len(ixData))}
	}

	msg := make([]byte, 0, 3+1+totalAccounts*32+32+1+1+1+len(ixAccountIndices)+1+len(ixData))
	msg = append(msg, header[:]...)
	msg = append(msg, byte(totalAccounts))
	for i := range accounts {
		msg = append(msg, accounts[i][:]...)
	}
	msg = append(msg, programID[:]...) // program key is the last account
	msg = append(msg, recentBlockhash[:]...)
	msg = append(msg, 1) // instruction count
	msg = append(msg, programIDIndex)
	msg = append(msg, byte(len(ixAccountIndices)))
	msg = append(msg, ixAccountIndices...)
	msg = append(msg, byte(len(ixData)))
	msg = append(msg, ixData...)
	return msg, nil
}

// anchorDiscriminator computes the Anchor instruction discriminator:
// sha256("global:<name>")[..8].
func anchorDiscriminator(name string) [8]byte {
	sum := sha256.Sum256([]byte("global:" + name))
	var disc [8]byte
	copy(disc[:], sum[:8])
	return disc
}

// findProgramAddress derives a Program Derived Address using SHA-256 (same as
// the Solana runtime). Returns (pubkey, bump, ok). ok is false if no valid
// off-curve point is found across all 256 bumps.
//
// Hash input order matches solana_program::pubkey::Pubkey::create_program_address:
// seeds || bump || program_id || "ProgramDerivedAddress" (the program id comes
// BEFORE the PDA marker, not after).
func findProgramAddress(seeds [][]byte, programID [32]byte) ([32]byte, uint8, bool) {
	for nonce := 255; nonce >= 0; nonce-- {
		h := sha256.New()
		for _, seed := range seeds {
			h.Write(seed)
		}
		h.Write([]byte{byte(nonce)})
		h.Write(programID[:])
		h.Write([]byte("ProgramDerivedAddress"))
		var candidate [32]byte
		copy(candidate[:], h.Sum(nil))
		if !isOnEd25519Curve(candidate) {
			return candidate, uint8(nonce), true
		}
	}
	return [32]byte{}, 0, false
}

// isOnEd25519Curve reports whether 32 bytes decode to a valid compressed point
// on the ed25519 curve. A PDA must be OFF the curve.
func isOnEd25519Curve(b [32]byte) bool {
	_, err := new(edwards25519.Point).SetBytes(b[:])
	return err == nil
}

// deriveATAAddress derives the Associated Token Account address for a wallet
// and mint: find_program_address([wallet, token_program, mint], ata_program).
func deriveATAAddress(wallet, mint [32]byte) ([32]byte, bool) {
	tokenProgram, err := decodeBs58Pubkey(tokenProgramID)
	if err != nil {
		return [32]byte{}, false
	}
	ataProgram, err := decodeBs58Pubkey(ataProgramID)
	if err != nil {
		return [32]byte{}, false
	}
	addr, _, ok := findProgramAddress([][]byte{wallet[:], tokenProgram[:], mint[:]}, ataProgram)
	return addr, ok
}

// decodeBs58Pubkey decodes a base58-encoded pubkey into 32 bytes.
func decodeBs58Pubkey(s string) ([32]byte, error) {
	var out [32]byte
	raw, err := base58.Decode(s)
	if err != nil {
		return out, fmt.Errorf("invalid base58: %w", err)
	}
	if len(raw) != 32 {
		return out, fmt.Errorf("expected 32 bytes, got %d", len(raw))
	}
	copy(out[:], raw)
	return out, nil
}

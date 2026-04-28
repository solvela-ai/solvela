package solvela

import (
	"crypto/ed25519"
	"encoding/base64"
	"fmt"
	"os"
	"runtime"

	"github.com/mr-tron/base58"
)

// EscrowDepositParams contains all inputs for building an escrow deposit
// transaction. Mirrors crates/x402/src/escrow/deposit.rs::DepositParams.
type EscrowDepositParams struct {
	// AgentKeypairB58 is the base58 64-byte ed25519 keypair of the agent.
	AgentKeypairB58 string
	// ProviderWalletB58 is the gateway recipient wallet (base58).
	ProviderWalletB58 string
	// EscrowProgramIDB58 is the deployed escrow program ID (base58).
	EscrowProgramIDB58 string
	// Amount is the deposit amount in atomic USDC units (must be > 0).
	Amount uint64
	// ServiceID is the 32-byte correlation identifier for the escrow PDA.
	ServiceID [32]byte
	// ExpirySlot is the absolute Solana slot at which the deposit expires.
	ExpirySlot uint64
}

// BuildEscrowDepositResult returns both the encoded transaction and the
// agent pubkey (base58), which the caller needs to populate the x402 payload.
type BuildEscrowDepositResult struct {
	DepositTxB64 string
	AgentPubkey  string
}

// BuildEscrowDeposit builds and signs an Anchor escrow deposit transaction.
// Returns the base64-encoded wire transaction and the agent's base58 pubkey.
//
// Requires SOLANA_RPC_URL env var to fetch a recent blockhash.
// The caller supplies ExpirySlot — use GetExpirySlotFromTimeout as a helper.
//
// On any failure returns a *SigningError (see signing_error.go).
func BuildEscrowDeposit(params EscrowDepositParams) (BuildEscrowDepositResult, error) {
	if params.Amount == 0 {
		return BuildEscrowDepositResult{}, &SigningError{
			Message: "escrow deposit amount must be positive",
		}
	}

	rpcURL := os.Getenv("SOLANA_RPC_URL")
	if rpcURL == "" {
		return BuildEscrowDepositResult{}, &SigningError{
			Message: "SOLANA_RPC_URL environment variable required for on-chain signing",
		}
	}

	// ------------------------------------------------------------------
	// Decode + validate the 64-byte agent keypair via the single canonical
	// helper in solana.go. This re-derives the pubkey from the seed and
	// compares it to the stored pubkey half.
	// ------------------------------------------------------------------
	priv, agent, err := decodeAndValidateKeypair(params.AgentKeypairB58)
	if err != nil {
		return BuildEscrowDepositResult{}, err // already a *SigningError
	}
	agentB58 := base58.Encode(agent[:])

	// Zero the derived private key bytes on exit. `ed25519.NewKeyFromSeed`
	// returns a fresh slice that does NOT alias the original input buffer,
	// so zeroing `priv` is the sanctioned place to scrub the secret.
	defer func() {
		for i := range priv {
			priv[i] = 0
		}
		runtime.KeepAlive(priv)
	}()

	// ------------------------------------------------------------------
	// Decode all static addresses
	// ------------------------------------------------------------------
	provider, err := base58DecodePubkey(params.ProviderWalletB58)
	if err != nil {
		return BuildEscrowDepositResult{}, &SigningError{
			Message: fmt.Sprintf("invalid provider wallet: %v", err),
			cause:   err,
		}
	}
	usdcMint, err := base58DecodePubkey(USDCMint)
	if err != nil {
		return BuildEscrowDepositResult{}, &SigningError{
			Message: fmt.Sprintf("invalid USDC mint: %v", err),
			cause:   err,
		}
	}
	escrowProgramID, err := base58DecodePubkey(params.EscrowProgramIDB58)
	if err != nil {
		return BuildEscrowDepositResult{}, &SigningError{
			Message: fmt.Sprintf("invalid escrow program id: %v", err),
			cause:   err,
		}
	}
	tokenProgram, err := base58DecodePubkey(TokenProgramIDB58)
	if err != nil {
		return BuildEscrowDepositResult{}, &SigningError{
			Message: fmt.Sprintf("invalid token program: %v", err),
			cause:   err,
		}
	}
	ataProgram, err := base58DecodePubkey(AssociatedTokenProgramIDB58)
	if err != nil {
		return BuildEscrowDepositResult{}, &SigningError{
			Message: fmt.Sprintf("invalid ata program: %v", err),
			cause:   err,
		}
	}
	systemProgram, err := base58DecodePubkey(SystemProgramIDB58)
	if err != nil {
		return BuildEscrowDepositResult{}, &SigningError{
			Message: fmt.Sprintf("invalid system program: %v", err),
			cause:   err,
		}
	}

	// ------------------------------------------------------------------
	// Derive the escrow PDA and both ATAs
	// ------------------------------------------------------------------
	escrowPDA, _, err := findProgramAddress(
		[][]byte{[]byte("escrow"), agent[:], params.ServiceID[:]},
		escrowProgramID,
	)
	if err != nil {
		return BuildEscrowDepositResult{}, &SigningError{
			Message: fmt.Sprintf("derive escrow PDA: %v", err),
			cause:   err,
		}
	}
	agentATA, err := deriveATA(agent, usdcMint)
	if err != nil {
		return BuildEscrowDepositResult{}, &SigningError{
			Message: fmt.Sprintf("derive agent ATA: %v", err),
			cause:   err,
		}
	}
	vaultATA, err := deriveATA(escrowPDA, usdcMint)
	if err != nil {
		return BuildEscrowDepositResult{}, &SigningError{
			Message: fmt.Sprintf("derive vault ATA: %v", err),
			cause:   err,
		}
	}

	// ------------------------------------------------------------------
	// Fetch blockhash from RPC
	// ------------------------------------------------------------------
	blockhash, err := getRecentBlockhash(rpcURL)
	if err != nil {
		return BuildEscrowDepositResult{}, &SigningError{
			Message: fmt.Sprintf("fetch blockhash: %v", err),
			cause:   err,
		}
	}

	// ------------------------------------------------------------------
	// Build instruction data:
	//   discriminator(8) || amount_u64_LE(8) || service_id[32] || expiry_u64_LE(8) = 56 bytes
	// ------------------------------------------------------------------
	disc := anchorDiscriminator("deposit")
	ixData := make([]byte, 0, 56)
	ixData = append(ixData, disc[:]...)
	var amountBuf [8]byte
	putUint64LE(&amountBuf, params.Amount)
	ixData = append(ixData, amountBuf[:]...)
	ixData = append(ixData, params.ServiceID[:]...)
	var expiryBuf [8]byte
	putUint64LE(&expiryBuf, params.ExpirySlot)
	ixData = append(ixData, expiryBuf[:]...)
	if len(ixData) != 56 {
		return BuildEscrowDepositResult{}, &SigningError{
			Message: fmt.Sprintf("instruction data length = %d, want 56", len(ixData)),
		}
	}

	// ------------------------------------------------------------------
	// Sort account keys by writability for legacy message format.
	// Order:
	//   0: agent          (signer, writable)
	//   1: escrowPDA      (writable, non-signer)
	//   2: agentATA       (writable, non-signer)
	//   3: vaultATA       (writable, non-signer)
	//   4: provider       (readonly, non-signer)
	//   5: usdcMint       (readonly, non-signer)
	//   6: tokenProgram   (readonly, non-signer)
	//   7: ataProgram     (readonly, non-signer)
	//   8: systemProgram  (readonly, non-signer)
	//   9: escrowProgram  (program key — appended last inside buildLegacyMessage)
	// ------------------------------------------------------------------
	accounts := [][32]byte{
		agent,
		escrowPDA,
		agentATA,
		vaultATA,
		provider,
		usdcMint,
		tokenProgram,
		ataProgram,
		systemProgram,
	}

	// Anchor declaration order: agent, provider, mint, escrow, agent_ata,
	// vault, token, ata, system — which maps to sorted positions:
	//   agent=0, provider=4, mint=5, escrow=1, agent_ata=2, vault=3,
	//   token=6, ata=7, system=8.
	ixAccountIndices := []byte{0, 4, 5, 1, 2, 3, 6, 7, 8}

	// Header: [1, 0, 6] — 1 signer, 0 readonly-signed, 6 readonly-unsigned
	// (provider(4), mint(5), token(6), ata(7), system(8), escrowProgram(9)).
	header := [3]byte{1, 0, 6}

	msg := buildLegacyMessage(
		header,
		accounts,
		escrowProgramID,
		blockhash,
		9, // program_id_index = 9
		ixAccountIndices,
		ixData,
	)

	// ------------------------------------------------------------------
	// Sign and assemble wire format
	// ------------------------------------------------------------------
	sig := ed25519.Sign(priv, msg)

	tx := make([]byte, 0, 1+64+len(msg))
	tx = append(tx, 0x01)
	tx = append(tx, sig...)
	tx = append(tx, msg...)

	return BuildEscrowDepositResult{
		DepositTxB64: base64.StdEncoding.EncodeToString(tx),
		AgentPubkey:  agentB58,
	}, nil
}

// GetExpirySlotFromTimeout computes the absolute slot at which an escrow
// deposit with the given timeout (in seconds) will expire. Solana slots
// are approximately 400ms, so we convert seconds to slots and add a safety
// minimum of 10 slots.
//
// Requires a live RPC endpoint to fetch the current slot.
//
// Overflow guard: on 32-bit platforms (GOARCH=386, arm), `int` is 32 bits
// wide, so the intermediate product `maxTimeoutSeconds * 1000` can overflow
// BEFORE the conversion to uint64. We widen first (`uint64(maxTimeoutSeconds)`)
// then multiply — which uses 64-bit arithmetic everywhere.
func GetExpirySlotFromTimeout(rpcURL string, maxTimeoutSeconds int) (uint64, error) {
	currentSlot, err := getSlot(rpcURL)
	if err != nil {
		return 0, err
	}
	timeoutSlots := uint64(maxTimeoutSeconds) * 1000 / 400
	if timeoutSlots < 10 {
		timeoutSlots = 10
	}
	return currentSlot + timeoutSlots, nil
}

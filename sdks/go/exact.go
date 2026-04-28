package solvela

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"runtime"
	"time"
)

// transferCheckedOpcode is the SPL Token program instruction opcode for
// TransferChecked (index 12 in the SPL token instruction enum).
const transferCheckedOpcode byte = 12

// buildSolanaTransferChecked builds and signs a legacy Solana transaction
// that calls SPL Token TransferChecked for USDC.
//
// payTo:      recipient wallet base58.
// amount:     atomic USDC units (6 decimals).
// privateKey: base58 64-byte ed25519 keypair (32 secret + 32 pubkey).
//
// Requires SOLANA_RPC_URL env var. Returns the base64-encoded wire transaction.
func buildSolanaTransferChecked(payTo string, amount uint64, privateKey string) (string, error) {
	if amount == 0 {
		return "", &SigningError{Message: "payment amount must be positive"}
	}

	rpcURL := os.Getenv("SOLANA_RPC_URL")
	if rpcURL == "" {
		return "", &SigningError{
			Message: "SOLANA_RPC_URL environment variable required for on-chain signing",
		}
	}

	// Decode and genuinely validate the 64-byte keypair via the single
	// canonical helper in solana.go. This re-derives the pubkey from the
	// seed and compares it to the stored pubkey half — catching corrupted
	// or malicious keypairs that a naive cast would silently accept.
	priv, payer, err := decodeAndValidateKeypair(privateKey)
	if err != nil {
		return "", err // already a *SigningError
	}

	// Zero the derived private key bytes on exit. `ed25519.NewKeyFromSeed`
	// returns a fresh slice, so zeroing `priv` is sufficient — there is no
	// aliased raw input buffer to also wipe at this level.
	defer func() {
		for i := range priv {
			priv[i] = 0
		}
		runtime.KeepAlive(priv)
	}()

	recipient, err := base58DecodePubkey(payTo)
	if err != nil {
		return "", &SigningError{Message: fmt.Sprintf("invalid pay_to address: %v", err), cause: err}
	}
	usdcMint, err := base58DecodePubkey(USDCMint)
	if err != nil {
		return "", &SigningError{Message: fmt.Sprintf("invalid USDC mint constant: %v", err), cause: err}
	}
	tokenProgram, err := base58DecodePubkey(TokenProgramIDB58)
	if err != nil {
		return "", &SigningError{Message: fmt.Sprintf("invalid token program constant: %v", err), cause: err}
	}

	senderATA, err := deriveATA(payer, usdcMint)
	if err != nil {
		return "", &SigningError{Message: fmt.Sprintf("derive sender ATA: %v", err), cause: err}
	}
	recipientATA, err := deriveATA(recipient, usdcMint)
	if err != nil {
		return "", &SigningError{Message: fmt.Sprintf("derive recipient ATA: %v", err), cause: err}
	}

	// Fetch recent blockhash from RPC.
	blockhash, err := getRecentBlockhash(rpcURL)
	if err != nil {
		return "", &SigningError{Message: fmt.Sprintf("fetch blockhash: %v", err), cause: err}
	}

	// Build instruction data: [12, amount_u64_LE, decimals].
	ixData := make([]byte, 10)
	ixData[0] = transferCheckedOpcode
	var amountBuf [8]byte
	putUint64LE(&amountBuf, amount)
	copy(ixData[1:9], amountBuf[:])
	ixData[9] = USDCDecimals

	// Sorted account keys (writability-sorted legacy message requirement):
	// 0: payer        (signer, writable)
	// 1: senderATA    (writable, non-signer)
	// 2: recipientATA (writable, non-signer)
	// 3: usdcMint     (readonly, non-signer)
	// 4: tokenProgram (readonly, program — appended last)
	accounts := [][32]byte{
		payer,
		senderATA,
		recipientATA,
		usdcMint,
	}
	programID := tokenProgram

	// SPL TransferChecked declaration order: source, mint, dest, owner.
	// Remap to sorted positions: source=1, mint=3, dest=2, owner=0.
	ixAccountIndices := []byte{1, 3, 2, 0}

	// Header: [1, 0, 2] — 1 signer, 0 readonly-signed, 2 readonly-unsigned
	// (mint at 3 and tokenProgram at 4).
	header := [3]byte{1, 0, 2}

	msg := buildLegacyMessage(
		header,
		accounts,
		programID,
		blockhash,
		4, // program_id_index = 4
		ixAccountIndices,
		ixData,
	)

	// Sign the message bytes.
	sig := ed25519.Sign(priv, msg)

	// Wire format: compact_u16(1) || sig(64) || msg
	tx := make([]byte, 0, 1+64+len(msg))
	tx = append(tx, 0x01)
	tx = append(tx, sig...)
	tx = append(tx, msg...)

	return base64.StdEncoding.EncodeToString(tx), nil
}

// ---------------------------------------------------------------------------
// Legacy message serializer
// ---------------------------------------------------------------------------

// buildLegacyMessage serializes a Solana legacy transaction message.
// Layout matches crates/x402/src/escrow/deposit.rs::build_legacy_message.
func buildLegacyMessage(
	header [3]byte,
	accounts [][32]byte,
	programID [32]byte,
	recentBlockhash [32]byte,
	programIDIndex byte,
	ixAccountIndices []byte,
	ixData []byte,
) []byte {
	totalAccounts := uint16(len(accounts) + 1) // +1 for program key

	var buf bytes.Buffer
	buf.Write(header[:])
	buf.Write(encodeCompactU16(totalAccounts))
	for _, a := range accounts {
		buf.Write(a[:])
	}
	buf.Write(programID[:])
	buf.Write(recentBlockhash[:])
	// Instruction count: 1.
	buf.Write(encodeCompactU16(1))
	// Instruction: programIDIndex || compact_u16(accts) || accts || compact_u16(data_len) || data
	buf.WriteByte(programIDIndex)
	buf.Write(encodeCompactU16(uint16(len(ixAccountIndices))))
	buf.Write(ixAccountIndices)
	buf.Write(encodeCompactU16(uint16(len(ixData))))
	buf.Write(ixData)

	return buf.Bytes()
}

// ---------------------------------------------------------------------------
// RPC helpers (minimal JSON-RPC POST)
// ---------------------------------------------------------------------------

// getRecentBlockhash fetches the latest finalized blockhash from a Solana RPC.
// Returns the raw 32-byte hash suitable for embedding into a message.
func getRecentBlockhash(rpcURL string) ([32]byte, error) {
	reqBody := []byte(`{"jsonrpc":"2.0","id":1,"method":"getLatestBlockhash","params":[{"commitment":"finalized"}]}`)

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, "POST", rpcURL, bytes.NewReader(reqBody))
	if err != nil {
		return [32]byte{}, err
	}
	req.Header.Set("Content-Type", "application/json")

	client := &http.Client{Timeout: 10 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return [32]byte{}, err
	}
	defer resp.Body.Close()

	var parsed struct {
		Result struct {
			Value struct {
				Blockhash string `json:"blockhash"`
			} `json:"value"`
		} `json:"result"`
		Error *struct {
			Message string `json:"message"`
		} `json:"error"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&parsed); err != nil {
		return [32]byte{}, fmt.Errorf("decode rpc response: %w", err)
	}
	if parsed.Error != nil {
		return [32]byte{}, fmt.Errorf("rpc error: %s", parsed.Error.Message)
	}
	if parsed.Result.Value.Blockhash == "" {
		return [32]byte{}, fmt.Errorf("rpc response missing blockhash")
	}
	return base58DecodePubkey(parsed.Result.Value.Blockhash)
}

// getSlot fetches the current confirmed slot from a Solana RPC.
func getSlot(rpcURL string) (uint64, error) {
	reqBody := []byte(`{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[{"commitment":"confirmed"}]}`)

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, "POST", rpcURL, bytes.NewReader(reqBody))
	if err != nil {
		return 0, err
	}
	req.Header.Set("Content-Type", "application/json")

	client := &http.Client{Timeout: 10 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return 0, err
	}
	defer resp.Body.Close()

	var parsed struct {
		Result uint64 `json:"result"`
		Error  *struct {
			Message string `json:"message"`
		} `json:"error"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&parsed); err != nil {
		return 0, fmt.Errorf("decode rpc response: %w", err)
	}
	if parsed.Error != nil {
		return 0, fmt.Errorf("rpc error: %s", parsed.Error.Message)
	}
	return parsed.Result, nil
}

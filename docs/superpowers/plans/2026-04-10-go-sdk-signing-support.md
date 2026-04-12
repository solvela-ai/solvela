# Go SDK Solana Transaction Signing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the Go SDK to full signing parity with the Python and TypeScript SDKs so Go clients can pay real USDC-SPL via both the `exact` (direct SPL transfer) and `escrow` (Anchor deposit PDA) x402 payment schemes.

**Architecture:** The canonical wire-format logic already exists in three places: `crates/x402/src/escrow/deposit.rs` (Rust reference), `sdks/typescript/src/x402.ts` (TS reference), and `sdks/python/rustyclawrouter/x402.py` (Python reference). The Go SDK must mirror these **byte-for-byte** for the wire message, discriminator, PDA, and ATA derivations. The Go implementation uses only Go standard library crypto primitives (`crypto/ed25519`, `crypto/sha256`, `encoding/binary`, `encoding/base64`) plus a single pure-Go base58 library (`github.com/mr-tron/base58`) — we deliberately do **not** depend on `github.com/gagliardetto/solana-go` because we only need a narrow slice of Solana functionality and that dependency drags in ~50 transitive packages.

Because the Rust builder manually serializes the legacy message format (not `VersionedTransaction`/`MessageV0`), the Go builder follows the same pattern: account keys are pre-sorted by writability (writable signers → writable non-signers → readonly non-signers → programs) and the Anchor-expected account order is preserved through an `ix_account_indices` remap array.

**Tech Stack:** Go 1.21, `crypto/ed25519` (stdlib), `crypto/sha256` (stdlib), `crypto/rand` (stdlib), `encoding/base64` (stdlib), `encoding/binary` (stdlib), `github.com/mr-tron/base58` (MIT, pure Go).

---

## Critical Invariants (DO NOT VIOLATE)

These values are anchored to external ground truth (`solders` / `solana-web3.js` / `solana-py`). The Rust reference in `crates/x402/src/escrow/pda.rs:156-235` already contains regression tests pinned to the same constants. **Test authors must use these constants, not values derived from the Go code under test.**

| Invariant | Value | Source of truth |
|---|---|---|
| Escrow PDA regression | agent `[1u8;32]`, service_id `[2u8;32]`, program `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU` → PDA `BEAUsvsWvV4o6y7XkC1bkyTq4FtQnKErcV3dzTFPT5hX`, bump 255 | `solders.pubkey.Pubkey.find_program_address` |
| ATA regression | wallet `4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp`, mint `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` → ATA `CYHVCkLwiEjMBdRiz5MsrrCbVL2YTZuv57TjV3ggxoSN` | Helius `getTokenAccounts` mainnet verification |
| Anchor "deposit" discriminator | First 8 bytes of `sha256("global:deposit")` | Verify with `python -c 'import hashlib; print(hashlib.sha256(b"global:deposit").digest()[:8].hex())'` — **do NOT derive this from our own `anchorDiscriminator()` helper** |
| Associated Token Program ID | `ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL` | Solana SPL canonical (last 5 letters: `A8knL`, **NOT** `e1bxs` — historical typo bug in the RustyClawRouter repo; see PR #12) |
| Token Program ID | `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA` | Solana SPL canonical |
| System Program ID | `11111111111111111111111111111111` | Solana canonical |
| USDC mainnet mint | `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` | Circle USDC mainnet |
| PDA hash input order | `seeds || bump || program_id || "ProgramDerivedAddress"` | Matches `solana_program::pubkey::Pubkey::create_program_address`. **The program id comes BEFORE the marker, not after.** |

**The Escrow PDA regression inputs `agent=[1u8; 32]` and `service_id=[2u8; 32]` are synthetic fixtures — they are NOT valid ed25519 keypairs. Use them ONLY to test `findProgramAddress` directly. NEVER pass them to `BuildEscrowDeposit` or any function that expects a real keypair; that will fail validation in `decodeAndValidateKeypair`.**

---

## Recommended Dispatch Order

Task numbering below is logical but NOT strict execution order. The dispatcher should walk tasks in this order to avoid dependency inversions:

1. Task 1 — add `mr-tron/base58` dependency
2. Task 11 — add `SigningError` type (must land before Tasks 2, 4 and 6 that reference it)
3. Task 2 — add `solana.go` primitives (includes `decodeAndValidateKeypair` helper)
4. Task 3 — add `solana_test.go` (different author from Task 2)
5. Task 4 — add `exact.go` (imports `decodeAndValidateKeypair` from Task 2)
6. Task 5 — add `exact_test.go` (different author from Task 4)
7. Task 6 — add `escrow.go` (imports `decodeAndValidateKeypair` from Task 2)
8. Task 7 — add `escrow_test.go` (different author from Task 6)
9. Task 8 — update `wallet.go` (adds package-private `signExactPayment` / `signEscrowDeposit` methods — depends on Tasks 4 AND 6)
10. Task 9 — update `x402.go` + `client.go` (depends on Task 8 — `createPaymentHeader` takes `*Wallet`, not a raw key string)
11. Task 10 — update `x402_test.go` (different author from Task 9)
12. Task 12 — cross-verification + live devnet smoke test (verifier only)
13. Task 13 — update `HANDOFF.md`

**Dependency chain summary:** 11 → 2 → {3, 4 → 5, 6 → 7} → 8 → 9 → 10 → 12 → 13.

---

## Test Author Separation Rule

The user memory file `feedback_test_author_separation.md` requires: **for cryptographic and on-chain code, the test author must be a different agent from the implementation author.** This applies to Tasks 3, 5, 7, and 10 in this plan — the executor dispatching this plan MUST use a fresh subagent for each test task, and MUST NOT allow the same subagent to both implement and test any single module.

When dispatching via `superpowers:subagent-driven-development`, pass this constraint explicitly in the subagent brief.

---

## File Structure

### Go SDK (`sdks/go/`)

- **Create:** `sdks/go/solana.go` — Low-level crypto primitives: base58 helpers, PDA derivation, ATA derivation, Anchor discriminator, Solana program ID constants.
- **Create:** `sdks/go/solana_test.go` — Externally-anchored KATs for PDA, ATA, and discriminator.
- **Create:** `sdks/go/exact.go` — USDC-SPL `TransferChecked` transaction builder + `getLatestBlockhash` RPC helper.
- **Create:** `sdks/go/exact_test.go` — Exact-scheme builder tests.
- **Create:** `sdks/go/escrow.go` — Anchor escrow deposit transaction builder + `getSlot` RPC helper + `EscrowDepositParams` struct.
- **Create:** `sdks/go/escrow_test.go` — Escrow builder tests.
- **Create:** `sdks/go/signing_error.go` — `SigningError` type implementing `error` + `Unwrap()`.
- **Modify:** `sdks/go/go.mod` — Add `github.com/mr-tron/base58` dependency.
- **Modify:** `sdks/go/wallet.go` — Populate `address` field from base58 keypair on `NewWallet`.
- **Modify:** `sdks/go/x402.go` — Replace `createPaymentHeader` stub with real dispatch; remove escrow rejection; add `GenerateServiceID`.
- **Modify:** `sdks/go/x402_test.go` — Replace stub-accepting assertions and escrow-rejection test with real behavior tests.

### Documentation

- **Modify:** `HANDOFF.md:103` — Remove `- **Go SDK signing**: Still using stub. TypeScript SDK has real signing; Python + CLI have real signing (merged).`

---

## Wire-Format Contract (ground truth: `crates/x402/src/escrow/deposit.rs`)

Every escrow deposit transaction built by the Go SDK must produce bytes that are accepted by the Solana runtime AND produce the same Anchor account ordering as the Rust/Python/TS reference implementations. The critical details:

1. **Legacy (not v0) message format.** Matches the Rust builder. Simpler to serialize manually without external deps.
2. **Account sort order in the message:** `[agent (signer,writable), escrow_pda (writable), agent_ata (writable), vault_ata (writable), provider (readonly), usdc_mint (readonly), token_program (readonly), ata_program (readonly), system_program (readonly), escrow_program (program key, appended last)]` — 10 keys total.
3. **Message header:** `[1, 0, 6]` — 1 signer, 0 readonly-signed, 6 readonly-unsigned. The 6 readonly-unsigned are indices 4..9 inclusive (provider, mint, token, ata, system, escrow_program).
4. **Instruction account indices remap:** Anchor expects the order `agent, provider, mint, escrow, agent_ata, vault, token, ata, system` (0..8 in declaration order). After sorting, those Anchor indices map to the new positions `[0, 4, 5, 1, 2, 3, 6, 7, 8]`.
5. **`program_id_index = 9`** — the escrow program is the 10th key (zero-indexed at 9).
6. **Instruction data:** `anchor_discriminator("deposit") || amount_u64_LE || service_id[32] || expiry_slot_u64_LE` = 56 bytes exactly.
7. **Wire format:** `compact_u16(1) || signature(64) || message` — for a single-signer tx, the compact-u16 `1` encodes as a single byte `0x01`.
8. **Compact-u16 encoding:** For counts ≤ 127, a single byte. All our counts (10 keys, 9 instruction accounts, 56 bytes data, 1 instruction, 1 signature) fit in a single byte. The Go builder MUST still implement compact-u16 as a function (not inline-hardcode single bytes) to guard against future growth and to mirror the canonical encoding.

For the `exact` scheme, the shape is simpler: 4 keys `[sender (signer,writable), sender_ata (writable), recipient_ata (writable), usdc_mint (readonly), token_program (readonly, program)]` and a `TransferChecked` instruction (opcode 12) with data `[12, amount_u64_LE, decimals_u8]` = 10 bytes.

Wait — re-check against TS/Python references before coding. The Python reference uses `MessageV0.try_compile` which handles account sorting/deduplication automatically. The Rust reference hand-sorts. For Go, we hand-sort because we're using minimal primitives. **The test author's ground-truth bytes must come from decoding a reference-built transaction (e.g., a known-good Python-built TransferChecked for the same inputs), NOT from re-running our own Go builder.**

---

## Task 1: Add `github.com/mr-tron/base58` Dependency

**Context:** Go stdlib does not provide base58. `github.com/mr-tron/base58` is the de-facto standard pure-Go base58 library (MIT licensed, ~600 stars, used by many Solana-adjacent Go projects). Single file, zero transitive deps.

**Files:**
- Modify: `sdks/go/go.mod`
- Create: `sdks/go/go.sum` (via `go get`)

- [ ] **Step 1: Fetch the dependency**

Run from `sdks/go/`:
```bash
cd sdks/go && go get github.com/mr-tron/base58@v1.2.0
```
Expected: `go.mod` gets a `require github.com/mr-tron/base58 v1.2.0` line; `go.sum` is created with the hash. No source-code changes yet.

- [ ] **Step 2: Verify the package resolves and builds**

```bash
cd sdks/go && go build ./...
```
Expected: Build succeeds (no source changes yet, so this just confirms the dependency is cleanly resolvable).

- [ ] **Step 3: Verify existing tests still pass**

```bash
cd sdks/go && go test ./...
```
Expected: All 22 existing tests pass (no regression from the dependency addition).

- [ ] **Step 4: Commit**

```bash
git add sdks/go/go.mod sdks/go/go.sum
git commit -m "chore(go-sdk): add mr-tron/base58 dependency for Solana signing"
```

---

## Task 2: Create `sdks/go/solana.go` — Low-Level Crypto Primitives

**PREREQUISITE:** Task 11 (`SigningError` type) MUST be dispatched before this task — `decodeAndValidateKeypair` returns `*SigningError` on malformed / mismatched keypair inputs.

**Context:** This file is the Go equivalent of `crates/x402/src/escrow/pda.rs`. It provides the minimal set of functions needed to derive PDAs, ATAs, the Anchor discriminator, program-ID constants, AND the single canonical keypair decode + validation helper used by both `exact.go` and `escrow.go`. No HTTP, no transaction building — just pure crypto and byte manipulation.

**Solana keypair layout ↔ Go `ed25519.PrivateKey` layout:** Solana 64-byte keypairs follow `[seed(32) || pubkey(32)]`, which matches Go stdlib `crypto/ed25519.PrivateKey` layout exactly (`[seed(32) || pubkey(32)]`). This is why `ed25519.NewKeyFromSeed(kpBytes[:32])` produces the signing key Solana expects — *provided* the stored pubkey at bytes `[32:64]` actually matches the pubkey derived from the seed via ed25519 key generation. A naive cast `priv := ed25519.PrivateKey(kpBytes)` followed by `priv.Public()` is a NO-OP integrity check because `priv.Public()` just extracts bytes `[32:64]` directly — it does not re-derive. To genuinely validate, we must re-derive the pubkey from the seed with `ed25519.NewKeyFromSeed(seed)` and compare the result to the stored pubkey half. This mirrors the Rust reference at `crates/x402/src/escrow/deposit.rs:111-121` which uses `SigningKey::from_keypair_bytes`.

**IMPORTANT — ed25519 curve check:** Go's standard library does NOT expose a "is on ed25519 curve" check. PDA derivation in Solana rejects any candidate that IS on the curve (because it would otherwise collide with a normal wallet). The pure-Go approach used by several Solana Go tools is: attempt to decompress the point using the curve25519 subgroup structure, and if decompression succeeds, the point is on the curve. Because this is subtle, we use a direct approach: the candidate is off-curve iff `ed25519.PublicKey(bytes).Equal(...)` is undefined — actually, Go stdlib doesn't give us this either.

**Approach:** Implement off-curve check using the `crypto/ed25519/internal/edwards25519` algorithm manually — we port the 30-line decompression check from `curve25519-dalek`'s `CompressedEdwardsY::decompress`. Alternatively, use the tiny pure-Go library `filippo.io/edwards25519` (BSD-3, ~200 lines, zero deps — already a transitive dep of Go's stdlib `crypto/ed25519` since Go 1.20). **Decision:** add `filippo.io/edwards25519` as a second explicit dependency. It is pure-Go, vetted (written by Filippo Valsorda, former Go security lead), and exposes `new(edwards25519.Point).SetBytes(candidate)` which returns an error for off-curve points — exactly what we need.

**Files:**
- Modify: `sdks/go/go.mod` — add `filippo.io/edwards25519`
- Create: `sdks/go/solana.go`

- [ ] **Step 1: Add the edwards25519 dependency**

```bash
cd sdks/go && go get filippo.io/edwards25519@v1.1.0
```
Expected: `go.mod` gains `filippo.io/edwards25519 v1.1.0`; `go.sum` updated.

- [ ] **Step 2: Create `sdks/go/solana.go` with the complete implementation**

```go
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
```

- [ ] **Step 3: Verify it compiles**

```bash
cd sdks/go && go build ./...
```
Expected: Build succeeds. No tests added yet — tests land in Task 3 via a different agent.

- [ ] **Step 4: Commit**

```bash
git add sdks/go/go.mod sdks/go/go.sum sdks/go/solana.go
git commit -m "feat(go-sdk): add Solana crypto primitives (PDA, ATA, discriminator)"
```

---

## Task 3: Create `sdks/go/solana_test.go` — Externally-Anchored KATs

**Context:** This is a cryptographic-correctness test file. **The author of this task MUST be a different subagent from the author of Task 2.** The test values are drawn from the Invariants table at the top of this plan — DO NOT re-derive them from the Go code under test. Each constant has been independently verified against `solders` / Helius mainnet / the Solana runtime.

**Files:**
- Create: `sdks/go/solana_test.go`

- [ ] **Step 0: Externally recompute each discriminator BEFORE writing any test**

Before writing any test code, run these commands yourself and observe the output:

```bash
python3 -c 'import hashlib; print(hashlib.sha256(b"global:deposit").digest()[:8].hex())'
python3 -c 'import hashlib; print(hashlib.sha256(b"global:claim").digest()[:8].hex())'
python3 -c 'import hashlib; print(hashlib.sha256(b"global:refund").digest()[:8].hex())'
```

Only after seeing each hex value with your own eyes, hardcode them into the test. Do NOT copy values from this plan verbatim — recompute first. If the values you observe differ from what this plan lists, **TRUST YOUR OBSERVATION** and update the plan in a follow-up commit (never silently skew the tests toward the plan).

**Historical bug warning:** An earlier draft of this plan hardcoded `e445a52e51cb9a1d` as the deposit discriminator. That value is **WRONG** — it is neither the instruction discriminator (`sha256("global:deposit")[:8]`) nor the event discriminator (`sha256("event:Deposit")[:8]` = `3ecdf2aff4a98834`). It appears to have been invented or miscopied. If you find `e445a52e51cb9a1d` referenced anywhere in a plan or test file you are reading, treat it as a known bug that was already fixed and **VERIFY what you computed from Python yourself**. The correct value for `sha256("global:deposit")[:8]` is `f223c68952e1f2b6`.

**Ground-truth comment block requirement:** Every externally-computed constant in the test file MUST carry a comment block of the form:

```go
// EXTERNAL GROUND TRUTH — computed YYYY-MM-DD by <author>
// via: <exact command used to compute>
// DO NOT update this value without recomputing externally first.
const expectedDepositDiscriminator = "f223c68952e1f2b6"
```

Apply this to every constant: deposit disc, claim disc, refund disc, the Escrow PDA regression value, the ATA regression value, and the System program all-zeros expectation. No exceptions — the comment block is the audit trail.

- [ ] **Step 1: Write the failing tests**

Create `sdks/go/solana_test.go`:

```go
package rcr

import (
	"bytes"
	"crypto/ed25519"
	"encoding/hex"
	"strings"
	"testing"

	"github.com/mr-tron/base58"
)

// IMPORTANT — Anchor discriminator pitfalls
// -----------------------------------------
// Anchor generates TWO discriminators per instruction:
//   - sha256("global:<name>")[:8]       → the INSTRUCTION entrypoint discriminator
//   - sha256("event:<EventName>")[:8]   → the event emission discriminator
// We need the INSTRUCTION discriminator here. If you see a value that looks
// like "e445a52e51cb9a1d" (a known-wrong value from an earlier draft of this
// plan, fixed in a later review) or "3ecdf2aff4a98834" (which is actually
// sha256("event:Deposit")[:8], NOT the instruction discriminator) — those
// are NOT the instruction discriminator for "deposit". Recompute via:
//   python3 -c 'import hashlib; print(hashlib.sha256(b"global:deposit").digest()[:8].hex())'
// Expected: f223c68952e1f2b6

// TestAnchorDiscriminatorDeposit locks the "deposit" discriminator to an
// externally-computed value. Do NOT derive this from anchorDiscriminator()
// itself — it was computed via:
//   python -c 'import hashlib; print(hashlib.sha256(b"global:deposit").digest()[:8].hex())'
// Expected: f223c68952e1f2b6
func TestAnchorDiscriminatorDeposit(t *testing.T) {
	const expectedHex = "f223c68952e1f2b6"
	got := anchorDiscriminator("deposit")
	gotHex := hex.EncodeToString(got[:])
	if gotHex != expectedHex {
		t.Fatalf("anchorDiscriminator(\"deposit\") = %s, want %s", gotHex, expectedHex)
	}
}

// TestAnchorDiscriminatorClaim locks the "claim" discriminator, also
// externally verified.
func TestAnchorDiscriminatorClaim(t *testing.T) {
	// Computed via: python -c 'import hashlib; print(hashlib.sha256(b"global:claim").digest()[:8].hex())'
	const expectedHex = "3ec6d6c1d59f6cd2"
	got := anchorDiscriminator("claim")
	gotHex := hex.EncodeToString(got[:])
	if gotHex != expectedHex {
		t.Fatalf("anchorDiscriminator(\"claim\") = %s, want %s", gotHex, expectedHex)
	}
}

// TestAnchorDiscriminatorRefund locks the "refund" discriminator. The Go
// SDK does not currently call refund directly, but we pin the value so
// that if the helper is ever wired into a refund path, the constant is
// already audited and anchored to external ground truth.
func TestAnchorDiscriminatorRefund(t *testing.T) {
	// Computed via: python -c 'import hashlib; print(hashlib.sha256(b"global:refund").digest()[:8].hex())'
	const expectedHex = "0260b7fb3fd02e2e"
	got := anchorDiscriminator("refund")
	gotHex := hex.EncodeToString(got[:])
	if gotHex != expectedHex {
		t.Fatalf("anchorDiscriminator(\"refund\") = %s, want %s", gotHex, expectedHex)
	}
}

// TestEscrowPDARegression is pinned to the `solders`-computed canonical PDA
// from the Invariants table. Agent=[1;32], ServiceID=[2;32],
// Program=9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU.
// Expected PDA: BEAUsvsWvV4o6y7XkC1bkyTq4FtQnKErcV3dzTFPT5hX, bump: 255.
func TestEscrowPDARegression(t *testing.T) {
	programID, err := base58DecodePubkey("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU")
	if err != nil {
		t.Fatalf("decode program id: %v", err)
	}
	var agent [32]byte
	var serviceID [32]byte
	for i := range agent {
		agent[i] = 1
	}
	for i := range serviceID {
		serviceID[i] = 2
	}

	pda, bump, err := findProgramAddress(
		[][]byte{[]byte("escrow"), agent[:], serviceID[:]},
		programID,
	)
	if err != nil {
		t.Fatalf("findProgramAddress: %v", err)
	}

	expected, err := base58DecodePubkey("BEAUsvsWvV4o6y7XkC1bkyTq4FtQnKErcV3dzTFPT5hX")
	if err != nil {
		t.Fatalf("decode expected PDA: %v", err)
	}
	if pda != expected {
		t.Errorf("PDA = %s, want BEAUsvsWvV4o6y7XkC1bkyTq4FtQnKErcV3dzTFPT5hX",
			base58EncodePubkey(pda))
	}
	if bump != 255 {
		t.Errorf("bump = %d, want 255", bump)
	}
}

// TestATARegression is pinned to a Helius-verified mainnet ATA from the
// Invariants table.
// Wallet: 4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp
// Mint:   EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v (USDC)
// ATA:    CYHVCkLwiEjMBdRiz5MsrrCbVL2YTZuv57TjV3ggxoSN
func TestATARegression(t *testing.T) {
	wallet, err := base58DecodePubkey("4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp")
	if err != nil {
		t.Fatalf("decode wallet: %v", err)
	}
	mint, err := base58DecodePubkey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")
	if err != nil {
		t.Fatalf("decode mint: %v", err)
	}

	ata, err := deriveATA(wallet, mint)
	if err != nil {
		t.Fatalf("deriveATA: %v", err)
	}

	expected, err := base58DecodePubkey("CYHVCkLwiEjMBdRiz5MsrrCbVL2YTZuv57TjV3ggxoSN")
	if err != nil {
		t.Fatalf("decode expected ATA: %v", err)
	}
	if ata != expected {
		t.Errorf("ATA = %s, want CYHVCkLwiEjMBdRiz5MsrrCbVL2YTZuv57TjV3ggxoSN",
			base58EncodePubkey(ata))
	}
}

// TestATAProgramIDNotTypo guards against the historical typo bug where
// ATA_PROGRAM_ID ended in "e1bxs" instead of "A8knL". This test decodes the
// constant and re-encodes it, checking the last 5 characters.
func TestATAProgramIDNotTypo(t *testing.T) {
	decoded, err := base58DecodePubkey(AssociatedTokenProgramIDB58)
	if err != nil {
		t.Fatalf("decode ATA program id: %v", err)
	}
	reencoded := base58EncodePubkey(decoded)
	suffix := reencoded[len(reencoded)-5:]
	if suffix != "A8knL" {
		t.Errorf("ATA program id suffix = %s, want A8knL (typo bug regression)", suffix)
	}
}

// TestSystemProgramIDAllZeros verifies the System program id decodes to
// 32 zero bytes (Solana canonical).
func TestSystemProgramIDAllZeros(t *testing.T) {
	decoded, err := base58DecodePubkey(SystemProgramIDB58)
	if err != nil {
		t.Fatalf("decode system program id: %v", err)
	}
	var zero [32]byte
	if decoded != zero {
		t.Errorf("System program id = %x, want all zeros", decoded)
	}
}

// TestIsOnEd25519Curve_WalletOnCurve verifies that a known wallet address
// (decoded from a real base58 pubkey) is on the curve.
func TestIsOnEd25519Curve_WalletOnCurve(t *testing.T) {
	wallet, err := base58DecodePubkey("4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp")
	if err != nil {
		t.Fatalf("decode wallet: %v", err)
	}
	if !isOnEd25519Curve(wallet) {
		t.Error("expected a valid wallet address to be on the ed25519 curve")
	}
}

// TestIsOnEd25519Curve_PDAOffCurve verifies that a derived PDA is off the curve.
func TestIsOnEd25519Curve_PDAOffCurve(t *testing.T) {
	programID, _ := base58DecodePubkey("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU")
	var agent, serviceID [32]byte
	for i := range agent {
		agent[i] = 1
		serviceID[i] = 2
	}
	pda, _, err := findProgramAddress(
		[][]byte{[]byte("escrow"), agent[:], serviceID[:]},
		programID,
	)
	if err != nil {
		t.Fatalf("findProgramAddress: %v", err)
	}
	if isOnEd25519Curve(pda) {
		t.Error("expected PDA to be off the ed25519 curve")
	}
}

// TestCompactU16Encoding verifies the short_vec encoding used in Solana
// messages. Values are externally verified against the Solana SDK reference.
func TestCompactU16Encoding(t *testing.T) {
	cases := []struct {
		in  uint16
		out string
	}{
		{0, "00"},
		{1, "01"},
		{127, "7f"},
		{128, "8001"},
		{129, "8101"},
		{16383, "ff7f"},
		{16384, "808001"},
	}
	for _, c := range cases {
		got := hex.EncodeToString(encodeCompactU16(c.in))
		if got != c.out {
			t.Errorf("encodeCompactU16(%d) = %s, want %s", c.in, got, c.out)
		}
	}
}

// TestBase58RoundTrip verifies encode/decode symmetry.
func TestBase58RoundTrip(t *testing.T) {
	var key [32]byte
	for i := range key {
		key[i] = byte(i)
	}
	encoded := base58EncodePubkey(key)
	decoded, err := base58DecodePubkey(encoded)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	if decoded != key {
		t.Errorf("round-trip mismatch: got %x, want %x", decoded, key)
	}
}

// TestBase58InvalidLength rejects non-32-byte inputs.
func TestBase58InvalidLength(t *testing.T) {
	// "1111" decodes to 3 zero bytes, which is not 32.
	_, err := base58DecodePubkey("1111")
	if err == nil {
		t.Error("expected error for short base58 input")
	}
}

// TestDecodeAndValidateKeypair_Valid accepts a genuine base58 keypair and
// returns a private key whose derived pubkey matches the stored pubkey.
func TestDecodeAndValidateKeypair_Valid(t *testing.T) {
	var seed [32]byte
	for i := range seed {
		seed[i] = 7
	}
	priv := ed25519.NewKeyFromSeed(seed[:])
	b58 := base58.Encode(priv)

	got, pub, err := decodeAndValidateKeypair(b58)
	if err != nil {
		t.Fatalf("decodeAndValidateKeypair: %v", err)
	}
	// Derived priv must match.
	if !bytes.Equal(got, priv) {
		t.Error("returned private key differs from input")
	}
	// Pubkey must match the seed-derived pubkey.
	expectedPub := priv.Public().(ed25519.PublicKey)
	if !bytes.Equal(pub[:], expectedPub) {
		t.Error("returned agent pubkey differs from seed-derived pubkey")
	}
}

// TestDecodeAndValidateKeypair_SwappedPubkey is the critical integrity test.
// We build a malformed 64-byte keypair where the stored pubkey half has been
// replaced with an UNRELATED pubkey. A naive `ed25519.PrivateKey(kp).Public()`
// check would silently accept this (because Go's Public() just extracts
// bytes [32:64] rather than re-deriving). The real helper MUST reject it.
func TestDecodeAndValidateKeypair_SwappedPubkey(t *testing.T) {
	var seedA, seedB [32]byte
	for i := range seedA {
		seedA[i] = 1
	}
	for i := range seedB {
		seedB[i] = 2
	}
	privA := ed25519.NewKeyFromSeed(seedA[:])
	privB := ed25519.NewKeyFromSeed(seedB[:])
	pubB := privB.Public().(ed25519.PublicKey)

	// Build a malformed 64-byte keypair: seedA || pubB (unrelated).
	bad := make([]byte, 64)
	copy(bad[:32], privA[:32])  // seed half of A
	copy(bad[32:], pubB)        // pubkey half of B — mismatch!
	badB58 := base58.Encode(bad)

	_, _, err := decodeAndValidateKeypair(badB58)
	if err == nil {
		t.Fatal("expected error for mismatched stored pubkey, got nil")
	}
	if _, ok := err.(*SigningError); !ok {
		t.Errorf("expected *SigningError, got %T", err)
	}
}

// TestDecodeAndValidateKeypair_ShortLength rejects inputs that decode to
// anything other than 64 bytes.
func TestDecodeAndValidateKeypair_ShortLength(t *testing.T) {
	// 32 zero bytes — wrong length.
	short := base58.Encode(make([]byte, 32))
	_, _, err := decodeAndValidateKeypair(short)
	if err == nil {
		t.Fatal("expected error for 32-byte input")
	}
}

// TestDecodeAndValidateKeypair_InvalidBase58 rejects garbage strings.
// The input "0OIl" uses every character that is explicitly disallowed by
// the Bitcoin base58 alphabet (`0`, `O`, `I`, `l`), so a correct base58
// decoder MUST reject it. This is deterministic and unambiguous — do NOT
// replace it with "not!valid!base58!" because `!` is also not in the
// alphabet but reviewers can't tell that at a glance.
func TestDecodeAndValidateKeypair_InvalidBase58(t *testing.T) {
	_, _, err := decodeAndValidateKeypair("0OIl")
	if err == nil {
		t.Fatal("expected error for invalid base58 input containing disallowed chars")
	}
	if _, ok := err.(*SigningError); !ok {
		t.Errorf("expected *SigningError, got %T", err)
	}
}

// TestDecodeAndValidateKeypair_EmptyString rejects the empty string with a
// SigningError mentioning the 64-byte length requirement. This is the most
// common "user forgot to set SOLANA_WALLET_KEY" path and we want it to
// surface a clear error, not a base58 library error.
func TestDecodeAndValidateKeypair_EmptyString(t *testing.T) {
	_, _, err := decodeAndValidateKeypair("")
	if err == nil {
		t.Fatal("expected error for empty string input")
	}
	se, ok := err.(*SigningError)
	if !ok {
		t.Fatalf("expected *SigningError, got %T", err)
	}
	if !strings.Contains(se.Message, "64 bytes") {
		t.Errorf("expected error to mention \"64 bytes\", got %q", se.Message)
	}
}
```

Note: the test file above imports `bytes`, `crypto/ed25519`, `encoding/hex`, `strings`, `testing`, and `github.com/mr-tron/base58` for the `decodeAndValidateKeypair` tests. Make sure the import block at the top of the file includes all of these.

- [ ] **Step 2: Run tests to verify they pass (or identify drift)**

```bash
cd sdks/go && go test -v -run 'TestAnchor|TestEscrowPDA|TestATA|TestSystem|TestIsOnEd25519|TestCompactU16|TestBase58|TestDecodeAndValidateKeypair'
```
Expected: All ~17 tests PASS (12 primitive KATs including the new `TestAnchorDiscriminatorRefund` + 5 keypair-integrity tests: Valid, SwappedPubkey, ShortLength, InvalidBase58, EmptyString). If the discriminator test fails, regenerate the expected value with the Python command in Step 0 and update the constant — and if you observe `f223c68952e1f2b6`, trust it (that is the correct instruction discriminator for `sha256("global:deposit")[:8]`). If the PDA test fails, **do not** update the expected — re-check the `findProgramAddress` implementation for a bug in the hash input order (program_id must come BEFORE the "ProgramDerivedAddress" marker, not after). If `TestDecodeAndValidateKeypair_SwappedPubkey` fails, the helper is not genuinely re-deriving from the seed — re-read the Task 2 commentary.

- [ ] **Step 3: Commit**

```bash
git add sdks/go/solana_test.go
git commit -m "test(go-sdk): add externally-anchored KATs for PDA, ATA, discriminator"
```

---

## Task 4: Create `sdks/go/exact.go` — USDC TransferChecked Builder

**PREREQUISITE:** Task 11 (`SigningError` type) MUST be dispatched before this task — `exact.go` references `*SigningError`. If the dispatcher walks tasks in numeric order, it should reorder Task 11 ahead of Task 4.

**Context:** This builds and signs a legacy-format Solana transaction containing a single SPL `TransferChecked` instruction. It mirrors `sdks/typescript/src/x402.ts:buildSolanaTransferChecked` (lines 316-407) and `sdks/python/rustyclawrouter/x402.py:build_solana_transfer_checked` (lines 24-128). Because Go has no `TransactionMessage` helper, we hand-serialize the legacy message — same approach as `crates/x402/src/escrow/deposit.rs` but simpler (no PDA, fewer accounts).

**TransferChecked instruction layout** (SPL Token program opcode 12):
- Instruction data: `[12, amount_u64_LE (8 bytes), decimals_u8 (1 byte)]` = 10 bytes total.
- 4 account metas in Anchor/SPL declaration order: `source_ata (writable), mint (readonly), destination_ata (writable), owner (signer, writable)`.
- After sorting by writability: `[owner (signer+writable), source_ata (writable), destination_ata (writable), mint (readonly), token_program (readonly, the program key)]`.

**Account index remap (SPL declaration order → sorted order):**
| SPL declaration | Sorted position |
|---|---|
| 0: source_ata | 1 |
| 1: mint | 3 |
| 2: destination_ata | 2 |
| 3: owner | 0 |

So `ix_account_indices = [1, 3, 2, 0]`. The token program is key index 4 and `program_id_index = 4`. Message header = `[1, 0, 2]` (1 signer, 0 readonly-signed, 2 readonly-unsigned: mint at 3 and token_program at 4).

**Files:**
- Create: `sdks/go/exact.go`

- [ ] **Step 1: Create the file with the complete implementation**

```go
package rcr

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
```

- [ ] **Step 2: Verify it compiles**

```bash
cd sdks/go && go build ./...
```
Expected: Build succeeds. Requires Task 11 (`signing_error.go`) to already be in place.

- [ ] **Step 3: Commit**

```bash
git add sdks/go/exact.go
git commit -m "feat(go-sdk): add Solana TransferChecked transaction builder"
```

---

## Task 5: Create `sdks/go/exact_test.go` — TransferChecked Builder Tests

**Context:** Tests for the exact-scheme builder. **The author of this task MUST be a different subagent from the author of Task 4.** The test uses a deterministic seed `[42u8; 32]` to derive a fixed keypair (matching the Rust reference pattern at `crates/x402/src/escrow/deposit.rs:308-315`) so the test is fully reproducible. RPC-requiring tests are gated on `SOLANA_RPC_URL`; pure-derivation tests are unconditional.

**Files:**
- Create: `sdks/go/exact_test.go`

- [ ] **Step 1: Write the failing tests**

```go
package rcr

import (
	"crypto/ed25519"
	"encoding/base64"
	"os"
	"testing"

	"github.com/mr-tron/base58"
)

// testKeypairB58 returns a deterministic base58-encoded 64-byte keypair derived
// from seed [42; 32]. Matches the Rust reference pattern in
// crates/x402/src/escrow/deposit.rs:test_agent_keypair_b58.
func testKeypairB58(t *testing.T) string {
	t.Helper()
	var seed [32]byte
	for i := range seed {
		seed[i] = 42
	}
	priv := ed25519.NewKeyFromSeed(seed[:])
	// ed25519.PrivateKey already holds [seed(32) || pub(32)] when derived via
	// NewKeyFromSeed, matching the base58 64-byte format Solana keypairs use.
	return base58.Encode(priv)
}

// TestBuildTransferChecked_ZeroAmountRejected ensures zero amount fails before
// touching RPC or the key.
func TestBuildTransferChecked_ZeroAmountRejected(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "http://localhost:0") // unused — must fail earlier
	_, err := buildSolanaTransferChecked(
		"4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		0,
		testKeypairB58(t),
	)
	if err == nil {
		t.Fatal("expected error for zero amount")
	}
	se, ok := err.(*SigningError)
	if !ok {
		t.Fatalf("expected *SigningError, got %T", err)
	}
	if se.Message == "" {
		t.Error("SigningError message must not be empty")
	}
}

// TestBuildTransferChecked_MissingRPCURL ensures absence of SOLANA_RPC_URL
// fails cleanly.
func TestBuildTransferChecked_MissingRPCURL(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "")
	_, err := buildSolanaTransferChecked(
		"4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		1000,
		testKeypairB58(t),
	)
	if err == nil {
		t.Fatal("expected error when SOLANA_RPC_URL unset")
	}
	if _, ok := err.(*SigningError); !ok {
		t.Errorf("expected *SigningError, got %T", err)
	}
}

// TestBuildTransferChecked_InvalidPrivateKey rejects malformed base58 keys.
func TestBuildTransferChecked_InvalidPrivateKey(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "http://127.0.0.1:0")
	_, err := buildSolanaTransferChecked(
		"4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		1000,
		"not!valid!base58!",
	)
	if err == nil {
		t.Fatal("expected error for invalid private key")
	}
}

// TestBuildTransferChecked_InvalidKeyLength rejects keys that decode to the
// wrong length.
func TestBuildTransferChecked_InvalidKeyLength(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "http://127.0.0.1:0")
	// 32-byte key encoded in base58 (correct format would be 64 bytes).
	shortKey := base58.Encode(make([]byte, 32))
	_, err := buildSolanaTransferChecked(
		"4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		1000,
		shortKey,
	)
	if err == nil {
		t.Fatal("expected error for wrong key length")
	}
}

// TestBuildLegacyMessageRoundTripStructure verifies the message layout is
// well-formed by decoding the header and compact-u16 prefixes after building
// a minimal message. Does not require RPC.
func TestBuildLegacyMessageRoundTripStructure(t *testing.T) {
	var k1, k2, prog, blockhash [32]byte
	for i := range k1 {
		k1[i] = 1
		k2[i] = 2
		prog[i] = 3
		blockhash[i] = 4
	}
	msg := buildLegacyMessage(
		[3]byte{1, 0, 1},
		[][32]byte{k1, k2},
		prog,
		blockhash,
		2, // program at index 2
		[]byte{0, 1},
		[]byte{0xAA, 0xBB},
	)

	// Header 3 bytes
	if msg[0] != 1 || msg[1] != 0 || msg[2] != 1 {
		t.Errorf("header mismatch: %v", msg[0:3])
	}
	// account count compact-u16 single byte = 3 (k1, k2, prog)
	if msg[3] != 3 {
		t.Errorf("account count = %d, want 3", msg[3])
	}
	// Next 3*32 = 96 bytes are the keys
	if !bytesEqual(msg[4:36], k1[:]) {
		t.Error("key0 mismatch")
	}
	if !bytesEqual(msg[36:68], k2[:]) {
		t.Error("key1 mismatch")
	}
	if !bytesEqual(msg[68:100], prog[:]) {
		t.Error("program key mismatch")
	}
	// Then blockhash
	if !bytesEqual(msg[100:132], blockhash[:]) {
		t.Error("blockhash mismatch")
	}
	// Then ix count = 1
	if msg[132] != 1 {
		t.Errorf("ix count = %d, want 1", msg[132])
	}
	// Then program_id_index = 2
	if msg[133] != 2 {
		t.Errorf("program_id_index = %d, want 2", msg[133])
	}
	// Then account indices count = 2, indices = 0, 1
	if msg[134] != 2 || msg[135] != 0 || msg[136] != 1 {
		t.Errorf("ix account indices malformed: %v", msg[134:137])
	}
	// Then data len = 2, data = 0xAA, 0xBB
	if msg[137] != 2 || msg[138] != 0xAA || msg[139] != 0xBB {
		t.Errorf("ix data malformed: %v", msg[137:140])
	}
	if len(msg) != 140 {
		t.Errorf("total message length = %d, want 140", len(msg))
	}
}

// bytesEqual is a local helper to avoid a bytes import collision with
// the stdlib import in exact.go.
func bytesEqual(a, b []byte) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// TestBuildTransferChecked_Live runs against a real RPC if SOLANA_RPC_URL is
// set AND RCR_GO_SDK_LIVE_TEST=1. Verifies the output is valid base64 and
// contains the agent pubkey and a plausible signature.
func TestBuildTransferChecked_Live(t *testing.T) {
	if os.Getenv("RCR_GO_SDK_LIVE_TEST") != "1" {
		t.Skip("RCR_GO_SDK_LIVE_TEST not set")
	}
	if os.Getenv("SOLANA_RPC_URL") == "" {
		t.Skip("SOLANA_RPC_URL not set")
	}
	encoded, err := buildSolanaTransferChecked(
		"4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		1000, // 0.001 USDC
		testKeypairB58(t),
	)
	if err != nil {
		t.Fatalf("buildSolanaTransferChecked: %v", err)
	}
	decoded, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}
	// Wire format: 1 + 64 + message
	if len(decoded) < 100 {
		t.Errorf("tx too short: %d bytes", len(decoded))
	}
	if decoded[0] != 0x01 {
		t.Errorf("signature count byte = %d, want 1", decoded[0])
	}
}

// TestBuildSolanaTransferChecked_SignatureVerifies is an end-to-end signing
// correctness test. It asks the builder to sign a real transaction, then
// decodes the wire bytes, extracts the signature (bytes [1:65]) and message
// (bytes [65:]), and verifies the signature using ed25519.Verify against
// the deterministic agent pubkey. A passing result rules out whole classes
// of bugs: wrong message bytes signed, wrong signature length, wrong
// signing key, byte-order bugs, etc.
//
// Gated on SOLANA_RPC_URL + RCR_GO_SDK_LIVE_TEST=1 because
// buildSolanaTransferChecked fetches a blockhash from RPC.
func TestBuildSolanaTransferChecked_SignatureVerifies(t *testing.T) {
	if os.Getenv("RCR_GO_SDK_LIVE_TEST") != "1" {
		t.Skip("RCR_GO_SDK_LIVE_TEST not set")
	}
	if os.Getenv("SOLANA_RPC_URL") == "" {
		t.Skip("SOLANA_RPC_URL not set")
	}

	// Derive the deterministic agent pubkey from seed [42;32].
	var seed [32]byte
	for i := range seed {
		seed[i] = 42
	}
	agentPriv := ed25519.NewKeyFromSeed(seed[:])
	agentPub := agentPriv.Public().(ed25519.PublicKey)

	encoded, err := buildSolanaTransferChecked(
		"4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		1000,
		testKeypairB58(t),
	)
	if err != nil {
		t.Fatalf("buildSolanaTransferChecked: %v", err)
	}
	decoded, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}

	// Wire format: compact_u16(1) || signature(64) || message
	// For a single-signer tx, compact_u16(1) is a single byte 0x01.
	if len(decoded) < 1+64+1 {
		t.Fatalf("decoded tx too short: %d bytes", len(decoded))
	}
	if decoded[0] != 0x01 {
		t.Fatalf("signature count byte = %d, want 1", decoded[0])
	}
	signature := decoded[1:65]
	if len(signature) != 64 {
		t.Fatalf("signature length = %d, want 64", len(signature))
	}
	message := decoded[65:]

	if !ed25519.Verify(agentPub, message, signature) {
		t.Error("ed25519.Verify returned false — the signature does not verify " +
			"against the deterministic agent pubkey for the decoded message bytes")
	}
}
```

- [ ] **Step 2: Run tests to verify they pass**

```bash
cd sdks/go && go test -v -run 'TestBuildTransferChecked|TestBuildLegacyMessage|TestBuildSolanaTransferChecked_SignatureVerifies'
```
Expected: 6 unconditional tests pass + 2 live-gated tests skip unless `SOLANA_RPC_URL` and `RCR_GO_SDK_LIVE_TEST=1` are set (`TestBuildTransferChecked_Live` and `TestBuildSolanaTransferChecked_SignatureVerifies`).

- [ ] **Step 3: Commit**

```bash
git add sdks/go/exact_test.go
git commit -m "test(go-sdk): add TransferChecked builder tests"
```

---

## Task 6: Create `sdks/go/escrow.go` — Anchor Escrow Deposit Builder

**Context:** This is the most complex builder in the SDK. It mirrors `crates/x402/src/escrow/deposit.rs:build_deposit_tx` line-by-line. The account sort order, header, and `ix_account_indices` values are documented in the Wire-Format Contract section at the top of this plan. DO NOT deviate from the Rust reference account ordering — if the on-chain account order is wrong by even one slot, the Anchor program will reject the instruction with `AccountNotInitialized` (Anchor error 3012) or a seeds-mismatch error.

**Files:**
- Create: `sdks/go/escrow.go`

- [ ] **Step 1: Create the file with the complete implementation**

```go
package rcr

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
```

- [ ] **Step 2: Verify it compiles**

```bash
cd sdks/go && go build ./...
```
Expected: Build succeeds.

- [ ] **Step 3: Commit**

```bash
git add sdks/go/escrow.go
git commit -m "feat(go-sdk): add Anchor escrow deposit transaction builder"
```

---

## Task 7: Create `sdks/go/escrow_test.go` — Escrow Builder Tests

**Context:** Tests the escrow deposit builder. **The author of this task MUST be a different subagent from the author of Task 6.** We cannot easily verify the full signed transaction byte-for-byte without a reference Solana signer available in Go, so instead we verify structural invariants: discriminator is present, agent pubkey is present, PDA is derived correctly from the inputs, correct total length, and key error paths.

The test also includes a live-gated integration test that submits the transaction to devnet via `sendTransaction` when `RCR_GO_SDK_LIVE_TEST=1` — see Task 12.

**Files:**
- Create: `sdks/go/escrow_test.go`

- [ ] **Step 0: Externally compute the expected escrow PDA BEFORE writing any test**

Same pattern as Task 3 Step 0: compute the ground-truth PDA externally via Python+`solders` and observe the output yourself. Do NOT derive the expected value from the Go code under test.

Run the following in a shell with `solders` + `pynacl` installed (`pip install solders pynacl`):

```bash
python3 - <<'PY'
from solders.pubkey import Pubkey
from nacl.signing import SigningKey

# Deterministic agent: seed = [42] * 32 → ed25519 pubkey
sk = SigningKey(bytes([42] * 32))
agent = bytes(sk.verify_key)

# Deterministic service id: [0xAB] * 32
service_id = bytes([0xAB] * 32)

# Deployed escrow program id (same as used throughout this plan)
program = Pubkey.from_string("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU")

pda, bump = Pubkey.find_program_address([b"escrow", agent, service_id], program)
print(f"expectedEscrowPDA = {pda}")
print(f"bump             = {bump}")
PY
```

Only after observing the `expectedEscrowPDA` string with your own eyes, hardcode it into the package-level `expectedEscrowPDA` constant in `escrow_test.go` (see Step 1). The `init()` guard in the test file will PANIC at `go test` startup if you leave the placeholder `"<FILL ME IN FROM PYTHON OUTPUT>"` in place — this is deliberate, to prevent a placeholder from silently skipping the assertion when `SOLANA_RPC_URL` is unset.

Apply the same `EXTERNAL GROUND TRUTH` comment-block convention from Task 3 Step 0:

```go
// EXTERNAL GROUND TRUTH — computed YYYY-MM-DD by <author>
// via the solders+pynacl snippet in Task 7 Step 0 with seed=[42]*32,
// service_id=[0xAB]*32, program=9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU.
// DO NOT update this value without recomputing externally first.
```

- [ ] **Step 1: Write the failing tests**

```go
package rcr

import (
	"bytes"
	"crypto/ed25519"
	"encoding/base64"
	"os"
	"strings"
	"testing"

	"github.com/mr-tron/base58"
)

// expectedEscrowPDA is the externally-computed escrow PDA for the deterministic
// inputs used by TestBuildEscrowDeposit_BuilderPDAMatchesExternal. See Task 7
// Step 0 for the Python+solders snippet used to compute it. Leaving this as
// the "<FILL ME IN FROM PYTHON OUTPUT>" placeholder is a HARD FAILURE — the
// init() guard below will panic on `go test` startup so the bug cannot slip
// past when SOLANA_RPC_URL is unset and the test silently skips.
//
// EXTERNAL GROUND TRUTH — computed YYYY-MM-DD by <author>
// via the solders+pynacl snippet in Task 7 Step 0 with seed=[42]*32,
// service_id=[0xAB]*32, program=9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU.
// DO NOT update this value without recomputing externally first.
const expectedEscrowPDA = "<FILL ME IN FROM PYTHON OUTPUT>"

// init is a compile-time-ish placeholder guard. It fires at `go test`
// startup BEFORE any test runs, so if the author left the placeholder
// value in place this entire test binary will panic with a loud message.
// This catches a failure mode the reviewer flagged: a `const expectedPDA
// = "<FILL ME IN...>"` inside a test body compiles fine but never triggers
// if the test itself skips (e.g. because SOLANA_RPC_URL is unset), so a
// placeholder could silently ship. Running the guard from init() removes
// the "test skipped" escape hatch.
func init() {
	if strings.Contains(expectedEscrowPDA, "FILL ME IN") {
		panic("expectedEscrowPDA in sdks/go/escrow_test.go was not filled in " +
			"from external computation; see Task 7 Step 0 in the plan for the " +
			"exact Python+solders snippet to run")
	}
}

// escrowTestKeypairB58 returns the same deterministic base58 64-byte keypair
// used by Task 5 (seed [42; 32]) so the expected agent pubkey is stable
// across all escrow tests.
func escrowTestKeypairB58(t *testing.T) string {
	t.Helper()
	var seed [32]byte
	for i := range seed {
		seed[i] = 42
	}
	priv := ed25519.NewKeyFromSeed(seed[:])
	return base58.Encode(priv)
}

// escrowTestAgentPubkey returns the derived pubkey bytes for the seed-42
// keypair.
func escrowTestAgentPubkey(t *testing.T) [32]byte {
	t.Helper()
	var seed [32]byte
	for i := range seed {
		seed[i] = 42
	}
	priv := ed25519.NewKeyFromSeed(seed[:])
	pub := priv.Public().(ed25519.PublicKey)
	var out [32]byte
	copy(out[:], pub)
	return out
}

func TestBuildEscrowDeposit_ZeroAmountRejected(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "http://127.0.0.1:0")
	_, err := BuildEscrowDeposit(EscrowDepositParams{
		AgentKeypairB58:    escrowTestKeypairB58(t),
		ProviderWalletB58:  "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		EscrowProgramIDB58: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		Amount:             0,
		ServiceID:          [32]byte{1},
		ExpirySlot:         99999999,
	})
	if err == nil {
		t.Fatal("expected error for zero amount")
	}
	if _, ok := err.(*SigningError); !ok {
		t.Errorf("expected *SigningError, got %T", err)
	}
}

func TestBuildEscrowDeposit_MissingRPCURL(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "")
	_, err := BuildEscrowDeposit(EscrowDepositParams{
		AgentKeypairB58:    escrowTestKeypairB58(t),
		ProviderWalletB58:  "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		EscrowProgramIDB58: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		Amount:             1000,
		ServiceID:          [32]byte{1},
		ExpirySlot:         99999999,
	})
	if err == nil {
		t.Fatal("expected error for missing SOLANA_RPC_URL")
	}
}

func TestBuildEscrowDeposit_InvalidProviderAddress(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "http://127.0.0.1:0")
	_, err := BuildEscrowDeposit(EscrowDepositParams{
		AgentKeypairB58:    escrowTestKeypairB58(t),
		ProviderWalletB58:  "not!valid!",
		EscrowProgramIDB58: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		Amount:             1000,
		ServiceID:          [32]byte{1},
		ExpirySlot:         99999999,
	})
	if err == nil {
		t.Fatal("expected error for invalid provider address")
	}
}

func TestBuildEscrowDeposit_InvalidEscrowProgramID(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "http://127.0.0.1:0")
	_, err := BuildEscrowDeposit(EscrowDepositParams{
		AgentKeypairB58:    escrowTestKeypairB58(t),
		ProviderWalletB58:  "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		EscrowProgramIDB58: "not!valid!",
		Amount:             1000,
		ServiceID:          [32]byte{1},
		ExpirySlot:         99999999,
	})
	if err == nil {
		t.Fatal("expected error for invalid escrow program id")
	}
}

// TestBuildEscrowDeposit_BuilderPDAMatchesExternal verifies that the escrow
// PDA slot produced by `BuildEscrowDeposit` (slot [1] of the sorted account
// keys in the wire-format message) matches the package-level
// `expectedEscrowPDA` constant, which was computed EXTERNALLY via the
// Python+solders snippet in Task 7 Step 0 of the plan.
//
// This test intentionally decodes the wire bytes instead of calling
// findProgramAddress — we are testing the builder's output, not the helper.
// (findProgramAddress is exercised separately by Task 3.)
//
// IMPORTANT: `expectedEscrowPDA` is referenced unconditionally and also
// guarded by the init() check at the top of this file. If the test author
// forgot to fill in the Python-computed PDA, the init guard panics at
// `go test` startup — so there is no way to ship a placeholder value even
// if SOLANA_RPC_URL is unset and this test body skips.
//
// This test requires `SOLANA_RPC_URL` to be set to a working endpoint because
// `BuildEscrowDeposit` calls `getRecentBlockhash`. When running locally
// without devnet access, skip with `t.Skip` if the RPC is unreachable.
func TestBuildEscrowDeposit_BuilderPDAMatchesExternal(t *testing.T) {
	rpcURL := os.Getenv("SOLANA_RPC_URL")
	if rpcURL == "" {
		t.Skip("SOLANA_RPC_URL not set — needed for blockhash fetch")
	}
	t.Setenv("SOLANA_RPC_URL", rpcURL)

	// Deterministic inputs — same seed pattern the rest of the suite uses.
	var serviceID [32]byte
	for i := range serviceID {
		serviceID[i] = 0xAB
	}

	result, err := BuildEscrowDeposit(EscrowDepositParams{
		AgentKeypairB58:    escrowTestKeypairB58(t),
		ProviderWalletB58:  "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		EscrowProgramIDB58: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		Amount:             1000,
		ServiceID:          serviceID,
		ExpirySlot:         99999999,
	})
	if err != nil {
		t.Fatalf("BuildEscrowDeposit: %v", err)
	}

	decoded, err := base64.StdEncoding.DecodeString(result.DepositTxB64)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}

	// Parse the wire format to extract account keys.
	// Layout:
	//   compact_u16(sig_count=1)  → 1 byte
	//   signature                 → 64 bytes
	//   message:
	//     header                  → 3 bytes
	//     compact_u16(key_count)  → 1 byte (our count is 10, fits in 1 byte)
	//     account keys            → key_count * 32 bytes
	//     ... (rest ignored)
	//
	// Slot [0] is agent (signer+writable). Slot [1] is the escrow PDA
	// (writable non-signer appended immediately after signers).
	keys := decodeAccountKeysFromWire(t, decoded)
	if len(keys) < 2 {
		t.Fatalf("decoded account keys = %d, want ≥ 2", len(keys))
	}
	pdaSlot := base58EncodePubkey(keys[1])
	if pdaSlot != expectedEscrowPDA {
		t.Errorf("escrow PDA at slot [1] = %s, want %s", pdaSlot, expectedEscrowPDA)
	}
}

// decodeAccountKeysFromWire is a minimal wire-format decoder used only by the
// builder verification test above. It deliberately does NOT share code with
// the builder under test — the whole point is to parse the bytes independently
// and assert that the builder put the right keys at the right slots.
func decodeAccountKeysFromWire(t *testing.T, tx []byte) [][32]byte {
	t.Helper()
	if len(tx) < 1+64+3+1+32 {
		t.Fatalf("tx too short: %d bytes", len(tx))
	}
	// Skip compact-u16 signature count. For a single-signer tx this is 1 byte (0x01).
	if tx[0] != 0x01 {
		t.Fatalf("signature count byte = %d, want 1", tx[0])
	}
	offset := 1
	// Skip the 64-byte signature.
	offset += 64
	// Skip the 3-byte message header.
	offset += 3
	// Read the compact-u16 account count. Our count is always ≤ 127,
	// so it fits in one byte.
	if tx[offset]&0x80 != 0 {
		t.Fatalf("account count is multi-byte compact-u16, not supported here: %#x", tx[offset])
	}
	keyCount := int(tx[offset])
	offset++
	if len(tx) < offset+keyCount*32 {
		t.Fatalf("truncated account keys section: need %d bytes from offset %d, have %d",
			keyCount*32, offset, len(tx)-offset)
	}
	out := make([][32]byte, keyCount)
	for i := 0; i < keyCount; i++ {
		copy(out[i][:], tx[offset+i*32:offset+(i+1)*32])
	}
	return out
}

// TestBuildEscrowDeposit_Live exercises the full builder against a live RPC.
// Gated on SOLANA_RPC_URL + RCR_GO_SDK_LIVE_TEST=1.
//
// This test verifies the returned base64 tx decodes cleanly, contains the
// deposit discriminator, the agent pubkey, and a valid structure.
func TestBuildEscrowDeposit_Live(t *testing.T) {
	if os.Getenv("RCR_GO_SDK_LIVE_TEST") != "1" {
		t.Skip("RCR_GO_SDK_LIVE_TEST not set")
	}
	rpcURL := os.Getenv("SOLANA_RPC_URL")
	if rpcURL == "" {
		t.Skip("SOLANA_RPC_URL not set")
	}

	agentKP := escrowTestKeypairB58(t)
	var serviceID [32]byte
	for i := range serviceID {
		serviceID[i] = 0xAB
	}

	// Compute expiry slot from live RPC.
	expiry, err := GetExpirySlotFromTimeout(rpcURL, 300)
	if err != nil {
		t.Fatalf("GetExpirySlotFromTimeout: %v", err)
	}

	result, err := BuildEscrowDeposit(EscrowDepositParams{
		AgentKeypairB58:    agentKP,
		ProviderWalletB58:  "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		EscrowProgramIDB58: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		Amount:             1000,
		ServiceID:          serviceID,
		ExpirySlot:         expiry,
	})
	if err != nil {
		t.Fatalf("BuildEscrowDeposit: %v", err)
	}

	decoded, err := base64.StdEncoding.DecodeString(result.DepositTxB64)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}

	// Check signature count byte
	if decoded[0] != 0x01 {
		t.Errorf("signature count = %d, want 1", decoded[0])
	}

	// Check the agent pubkey is present in the tx
	agentPub := escrowTestAgentPubkey(t)
	if !bytes.Contains(decoded, agentPub[:]) {
		t.Error("agent pubkey not present in transaction bytes")
	}

	// Check the deposit discriminator is present
	disc := anchorDiscriminator("deposit")
	if !bytes.Contains(decoded, disc[:]) {
		t.Error("deposit discriminator not present in transaction bytes")
	}

	// Sanity check the agent pubkey base58 matches what's returned
	expectedB58 := base58.Encode(agentPub[:])
	if result.AgentPubkey != expectedB58 {
		t.Errorf("AgentPubkey = %s, want %s", result.AgentPubkey, expectedB58)
	}
}

// TestBuildEscrowDeposit_SignatureVerifies is an end-to-end signing
// correctness test analogous to TestBuildSolanaTransferChecked_SignatureVerifies
// in exact_test.go. It calls BuildEscrowDeposit to sign a real escrow
// deposit transaction, then decodes the wire bytes, extracts the signature
// (bytes [1:65]) and message (bytes [65:]), and verifies the signature using
// ed25519.Verify against the deterministic agent pubkey. A passing result
// rules out whole classes of bugs: wrong message bytes signed, wrong
// signature length, wrong signing key, or any byte-order regression that
// doesn't also break the Python cross-verify.
//
// Gated on SOLANA_RPC_URL + RCR_GO_SDK_LIVE_TEST=1 because
// BuildEscrowDeposit fetches a blockhash from RPC.
func TestBuildEscrowDeposit_SignatureVerifies(t *testing.T) {
	if os.Getenv("RCR_GO_SDK_LIVE_TEST") != "1" {
		t.Skip("RCR_GO_SDK_LIVE_TEST not set")
	}
	if os.Getenv("SOLANA_RPC_URL") == "" {
		t.Skip("SOLANA_RPC_URL not set")
	}

	agentPub := escrowTestAgentPubkey(t)
	var serviceID [32]byte
	for i := range serviceID {
		serviceID[i] = 0xAB
	}
	expiry, err := GetExpirySlotFromTimeout(os.Getenv("SOLANA_RPC_URL"), 300)
	if err != nil {
		t.Fatalf("GetExpirySlotFromTimeout: %v", err)
	}

	result, err := BuildEscrowDeposit(EscrowDepositParams{
		AgentKeypairB58:    escrowTestKeypairB58(t),
		ProviderWalletB58:  "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		EscrowProgramIDB58: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		Amount:             1000,
		ServiceID:          serviceID,
		ExpirySlot:         expiry,
	})
	if err != nil {
		t.Fatalf("BuildEscrowDeposit: %v", err)
	}

	decoded, err := base64.StdEncoding.DecodeString(result.DepositTxB64)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}

	// Wire format: compact_u16(1) || signature(64) || message
	if len(decoded) < 1+64+1 {
		t.Fatalf("decoded tx too short: %d bytes", len(decoded))
	}
	if decoded[0] != 0x01 {
		t.Fatalf("signature count byte = %d, want 1", decoded[0])
	}
	signature := decoded[1:65]
	if len(signature) != 64 {
		t.Fatalf("signature length = %d, want 64", len(signature))
	}
	message := decoded[65:]

	pub := ed25519.PublicKey(agentPub[:])
	if !ed25519.Verify(pub, message, signature) {
		t.Error("ed25519.Verify returned false — the escrow deposit signature " +
			"does not verify against the deterministic agent pubkey for the " +
			"decoded message bytes")
	}
}
```

- [ ] **Step 2: Run tests to verify they pass**

```bash
cd sdks/go && go test -v -run 'TestBuildEscrowDeposit'
```
Expected: 4 pure tests pass unconditionally (zero-amount, missing-RPC, invalid-provider, invalid-program-id); `TestBuildEscrowDeposit_BuilderPDAMatchesExternal` skips without `SOLANA_RPC_URL`; `TestBuildEscrowDeposit_Live` and `TestBuildEscrowDeposit_SignatureVerifies` skip without `RCR_GO_SDK_LIVE_TEST=1`. The init() guard at the top of the file MUST have fired successfully (i.e. `expectedEscrowPDA` is no longer the `"<FILL ME IN...>"` placeholder) or the whole test binary panics at startup.

- [ ] **Step 3: Commit**

```bash
git add sdks/go/escrow_test.go
git commit -m "test(go-sdk): add escrow deposit builder tests"
```

---

## Task 8: Update `sdks/go/wallet.go` — Populate `address` + Internal Signing Methods

**Context:** Two changes to `Wallet`:

1. **Populate `address` from the base58 keypair.** The existing `Wallet.Address()` returns an empty string because the stub `NewWallet` never decodes the private key. Now that the Go SDK has base58 primitives, `NewWallet` decodes the 64-byte keypair and extracts the pubkey half (bytes 32..64) as the base58 address. When decoding fails (empty key or malformed), address remains empty and the wallet is still usable for stub mode.

2. **Add package-private signing methods.** Instead of adding a public `PrivateKey()` getter and passing the raw key string around, signing responsibility lives inside `Wallet` via two package-private methods: `signExactPayment(payTo string, amount uint64) (string, error)` and `signEscrowDeposit(params EscrowDepositParams) (string, error)`. These read `w.privateKey` internally; callers at Task 9 never see the raw key string. This preserves parity with Python/TS where the key lives inside the signing context and never leaks via a public getter.

**PREREQUISITE:** Tasks 4 (`exact.go`) AND 6 (`escrow.go`) must already be in place because the new `Wallet` methods call `buildSolanaTransferChecked` and `BuildEscrowDeposit`.

**Files:**
- Modify: `sdks/go/wallet.go`
- Create: `sdks/go/wallet_test.go`

- [ ] **Step 1: Write a failing test**

Create `sdks/go/wallet_test.go`:
```go
package rcr

import (
	"crypto/ed25519"
	"testing"

	"github.com/mr-tron/base58"
)

func TestWalletAddressPopulatedFromKeypair(t *testing.T) {
	var seed [32]byte
	for i := range seed {
		seed[i] = 42
	}
	priv := ed25519.NewKeyFromSeed(seed[:])
	pub := priv.Public().(ed25519.PublicKey)
	keypairB58 := base58.Encode(priv)
	expectedAddr := base58.Encode(pub)

	w := NewWallet(keypairB58)
	if w.Address() != expectedAddr {
		t.Errorf("Address() = %q, want %q", w.Address(), expectedAddr)
	}
	if !w.HasKey() {
		t.Error("HasKey() = false, want true")
	}
}

func TestWalletNoKey(t *testing.T) {
	t.Setenv("SOLANA_WALLET_KEY", "")
	w := NewWallet("")
	if w.HasKey() {
		t.Error("HasKey() = true, want false")
	}
	if w.Address() != "" {
		t.Errorf("Address() = %q, want empty", w.Address())
	}
}

// TestWalletInvalidKeyLeavesAddressEmpty verifies the invariant that
// a malformed key leaves BOTH `address` and `privateKey` empty — and
// therefore `HasKey()` returns false. This is load-bearing: the
// `createPaymentHeader` dispatch uses `HasKey()` to choose between
// real signing and stub mode, and a stray-but-unusable key must not
// push the dispatcher onto the real-signing path.
//
// "0OIl" is the unambiguous invalid input: every character is
// explicitly disallowed by the Bitcoin base58 alphabet.
func TestWalletInvalidKeyLeavesAddressEmpty(t *testing.T) {
	w := NewWallet("0OIl")
	if w.Address() != "" {
		t.Errorf("Address() = %q, want empty for invalid key", w.Address())
	}
	if w.HasKey() {
		t.Error("HasKey() = true, want false for invalid key (invariant: " +
			"HasKey is true only when the key decodes to 64 bytes)")
	}
}

// TestWalletWrongLengthKeyLeavesHasKeyFalse verifies that a base58-valid
// but wrong-length input is also rejected at construction time. A 32-byte
// payload is a common "user pasted the seed instead of the full keypair"
// mistake — we want HasKey() to report false so the caller falls back to
// stub mode cleanly.
func TestWalletWrongLengthKeyLeavesHasKeyFalse(t *testing.T) {
	shortKey := base58.Encode(make([]byte, 32))
	w := NewWallet(shortKey)
	if w.HasKey() {
		t.Error("HasKey() = true, want false for 32-byte input")
	}
	if w.Address() != "" {
		t.Errorf("Address() = %q, want empty for wrong-length key", w.Address())
	}
}
```

- [ ] **Step 2: Run the failing test**

```bash
cd sdks/go && go test -v -run TestWallet
```
Expected: `TestWalletAddressPopulatedFromKeypair` FAILS because `address` is always empty.

- [ ] **Step 3: Update `wallet.go` — decode key and add package-private signing methods**

Replace the current `wallet.go` with:

```go
package rcr

import (
	"os"

	"github.com/mr-tron/base58"
)

// Wallet holds a Solana keypair for signing x402 payment transactions.
// The private key is kept unexported and is NEVER returned via a public
// getter. All signing happens inside package-private methods on Wallet so
// the raw key never crosses the type's boundary.
type Wallet struct {
	privateKey string
	address    string
}

// NewWallet creates a Wallet from a base58-encoded 64-byte keypair.
// If privateKey is empty, it falls back to the SOLANA_WALLET_KEY env var.
//
// Invariant: `w.privateKey` is only populated if the input decodes to a
// valid 64-byte base58 blob. If the input is empty, malformed, or the
// wrong length, `w.privateKey` remains empty AND `w.address` remains
// empty — and `HasKey()` returns false. This guarantees that whenever
// `HasKey()` reports true, the wallet is actually usable for signing
// (no surprise runtime failures inside buildSolanaTransferChecked from
// a key that passed construction but cannot decode).
//
// This behavior is load-bearing: `createPaymentHeader` switches on
// `wallet.HasKey()` to choose between real signing and stub mode. A
// stray invalid key would otherwise make the dispatcher take the real
// signing path and fail confusingly deep inside the builder.
func NewWallet(privateKey string) *Wallet {
	if privateKey == "" {
		privateKey = os.Getenv("SOLANA_WALLET_KEY")
	}
	w := &Wallet{}

	if privateKey != "" {
		if kp, err := base58.Decode(privateKey); err == nil && len(kp) == 64 {
			// Valid key: populate both private key and address.
			// Pubkey is the last 32 bytes of a Solana 64-byte keypair.
			w.privateKey = privateKey
			w.address = base58.Encode(kp[32:64])
		}
		// else: leave w.privateKey and w.address empty — HasKey() will
		// report false and the caller falls back to stub mode.
	}

	return w
}

// HasKey reports whether the wallet has a usable private key configured.
// Returns true iff `NewWallet` received a well-formed 64-byte base58
// keypair — it does NOT merely mean "the caller passed a non-empty string".
func (w *Wallet) HasKey() bool {
	return w.privateKey != ""
}

// Address returns the wallet's public address, or empty string if the key
// was not decodable.
func (w *Wallet) Address() string {
	return w.address
}

// signExactPayment signs an SPL TransferChecked payment for the given amount
// to the base58 payTo address and returns the base64-encoded wire-format
// Solana transaction. Returns an error if the wallet has no key or the
// signing path fails.
//
// Package-private: callers outside this package must go through
// createPaymentHeader, which takes *Wallet and never sees the raw key.
func (w *Wallet) signExactPayment(payTo string, amount uint64) (string, error) {
	if w.privateKey == "" {
		return "", &SigningError{Message: "wallet has no private key configured"}
	}
	return buildSolanaTransferChecked(payTo, amount, w.privateKey)
}

// signEscrowDeposit signs an Anchor escrow deposit transaction for the given
// params and returns the base64-encoded wire transaction + the agent pubkey
// (needed by the x402 escrow payload). Returns an error if the wallet has
// no key or the signing path fails.
//
// Package-private: same rationale as signExactPayment.
func (w *Wallet) signEscrowDeposit(params EscrowDepositParams) (BuildEscrowDepositResult, error) {
	if w.privateKey == "" {
		return BuildEscrowDepositResult{}, &SigningError{Message: "wallet has no private key configured"}
	}
	// Caller supplies everything except the keypair — we plug it in here so
	// the raw key never leaves this method.
	params.AgentKeypairB58 = w.privateKey
	return BuildEscrowDeposit(params)
}
```

Deliberately absent: there is no `PrivateKey()` getter. Any code that needs to sign must hold a `*Wallet` and call one of the two methods above. This matches the Python/TS SDKs, where the raw key lives inside the signing context and is not exposed via a getter.

- [ ] **Step 4: Run the test to verify it passes**

```bash
cd sdks/go && go test -v -run TestWallet
```
Expected: All 4 wallet tests PASS (address-from-keypair, no-key, invalid-key-leaves-empty, wrong-length-key).

- [ ] **Step 5: Commit**

```bash
git add sdks/go/wallet.go sdks/go/wallet_test.go
git commit -m "feat(go-sdk): populate Wallet.address from base58 keypair"
```

---

## Task 9: Update `sdks/go/x402.go` — Real `createPaymentHeader` Dispatch

**Context:** This replaces the stub-only `createPaymentHeader` with a full dispatch that selects between the exact and escrow schemes and calls the real transaction builders via the `*Wallet` signing methods from Task 8. It also removes the hard-coded escrow rejection (lines 62-69 of the current file) and adds the `GenerateServiceID` helper.

`createPaymentHeader` now takes a `*Wallet` pointer (NOT a raw private key string) and calls `wallet.signExactPayment` / `wallet.signEscrowDeposit` — matching the Python/TS SDKs where the raw key never crosses the wallet's type boundary. The current signature `(info *PaymentRequired, resourceURL string)` becomes `(info *PaymentRequired, resourceURL string, wallet *Wallet, requestBody []byte)`. The caller is `client.go:ChatCompletion` (line 135), which must be updated to pass `c.wallet` and the serialized request body.

**PREREQUISITE:** Task 8 MUST be dispatched before this task — `createPaymentHeader` calls `wallet.signExactPayment` and `wallet.signEscrowDeposit`, which are defined in Task 8.

**Files:**
- Modify: `sdks/go/x402.go`
- Modify: `sdks/go/client.go` (the `createPaymentHeader` call site)
- Verify: `sdks/go/client_test.go` — any existing call sites that still reference the old `createPaymentHeader(info, url)` signature must be updated so the file still compiles. Run `go build ./...` at the end of this task to catch build failures before moving on.

- [ ] **Step 1: Replace `sdks/go/x402.go` with the dispatch version**

```go
package rcr

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"os"
)

// paymentPayload is the structure encoded into the PAYMENT-SIGNATURE header
// for the exact scheme.
type paymentPayload struct {
	X402Version int             `json:"x402_version"`
	Resource    paymentResource `json:"resource"`
	Accepted    PaymentAccept   `json:"accepted"`
	Payload     solanaPayload   `json:"payload"`
}

// escrowPaymentPayload is the structure encoded into the PAYMENT-SIGNATURE
// header for the escrow scheme.
type escrowPaymentPayload struct {
	X402Version int             `json:"x402_version"`
	Resource    paymentResource `json:"resource"`
	Accepted    PaymentAccept   `json:"accepted"`
	Payload     escrowPayload   `json:"payload"`
}

type paymentResource struct {
	URL    string `json:"url"`
	Method string `json:"method"`
}

type solanaPayload struct {
	Transaction string `json:"transaction"`
}

type escrowPayload struct {
	DepositTx   string `json:"deposit_tx"`
	ServiceID   string `json:"service_id"`
	AgentPubkey string `json:"agent_pubkey"`
}

// GenerateServiceID produces a 32-byte correlation ID for an escrow deposit
// from the request body and 8 random bytes. Matches the Python/TS helpers in
// sdks/python/rustyclawrouter/x402.py and sdks/typescript/src/x402.ts.
func GenerateServiceID(requestBody []byte) ([32]byte, error) {
	var out [32]byte
	var nonce [8]byte
	if _, err := rand.Read(nonce[:]); err != nil {
		return out, err
	}
	h := sha256.New()
	h.Write(requestBody)
	h.Write(nonce[:])
	sum := h.Sum(nil)
	copy(out[:], sum)
	return out, nil
}

// createPaymentHeader builds the base64-encoded PAYMENT-SIGNATURE header
// from a 402 response. It prefers the escrow scheme when available AND the
// wallet has a key; otherwise it falls back to exact.
//
// When wallet has no key (HasKey() == false), a stub transaction value is
// used (development mode — payments will be rejected by a real gateway).
//
// The function takes a *Wallet (not a raw private key string) so the raw
// key never leaves the wallet's type boundary.
//
// On any real-signing failure returns a *PaymentError wrapping the
// underlying *SigningError for inspection via errors.As.
func createPaymentHeader(
	info *PaymentRequired,
	resourceURL string,
	wallet *Wallet,
	requestBody []byte,
) (string, error) {
	if len(info.Accepts) == 0 {
		return "", &PaymentError{Message: "no payment accepts in 402 response"}
	}

	// Prefer escrow scheme if available with a non-empty program id.
	var selected *PaymentAccept
	for i := range info.Accepts {
		if info.Accepts[i].Scheme == "escrow" && info.Accepts[i].EscrowProgramID != "" {
			selected = &info.Accepts[i]
			break
		}
	}
	if selected == nil {
		selected = &info.Accepts[0]
	}

	if selected.Scheme == "escrow" && selected.EscrowProgramID != "" {
		return buildEscrowHeader(selected, resourceURL, wallet, requestBody)
	}
	return buildExactHeader(selected, resourceURL, wallet)
}

// buildExactHeader builds the header for the direct-SPL-transfer path.
func buildExactHeader(
	accept *PaymentAccept,
	resourceURL string,
	wallet *Wallet,
) (string, error) {
	txB64 := "STUB_BASE64_TX"
	if wallet != nil && wallet.HasKey() {
		amount, err := parseAtomicAmount(accept.Amount)
		if err != nil {
			return "", &PaymentError{
				Message: "invalid amount in 402 response: " + err.Error(),
				cause:   err,
			}
		}
		signed, signErr := wallet.signExactPayment(accept.PayTo, amount)
		if signErr != nil {
			return "", &PaymentError{
				Message: "signing exact transfer failed: " + signErr.Error(),
				cause:   signErr,
			}
		}
		txB64 = signed
	}

	payload := paymentPayload{
		X402Version: X402Version,
		Resource:    paymentResource{URL: resourceURL, Method: "POST"},
		Accepted:    *accept,
		Payload:     solanaPayload{Transaction: txB64},
	}
	return marshalPayload(payload)
}

// buildEscrowHeader builds the header for the escrow deposit path.
func buildEscrowHeader(
	accept *PaymentAccept,
	resourceURL string,
	wallet *Wallet,
	requestBody []byte,
) (string, error) {
	serviceID, err := GenerateServiceID(requestBody)
	if err != nil {
		return "", &PaymentError{
			Message: "failed to generate service id: " + err.Error(),
			cause:   err,
		}
	}
	serviceIDB64 := base64.StdEncoding.EncodeToString(serviceID[:])

	depositTx := "STUB_ESCROW_DEPOSIT_TX"
	agentPubkey := "STUB_AGENT_PUBKEY"

	if wallet != nil && wallet.HasKey() {
		amount, amtErr := parseAtomicAmount(accept.Amount)
		if amtErr != nil {
			return "", &PaymentError{
				Message: "invalid escrow amount: " + amtErr.Error(),
				cause:   amtErr,
			}
		}

		rpcURL := os.Getenv("SOLANA_RPC_URL")
		if rpcURL == "" {
			return "", &PaymentError{
				Message: "SOLANA_RPC_URL env var required for escrow signing",
				cause:   &SigningError{Message: "SOLANA_RPC_URL not set"},
			}
		}
		expiry, slotErr := GetExpirySlotFromTimeout(rpcURL, accept.MaxTimeoutSeconds)
		if slotErr != nil {
			return "", &PaymentError{
				Message: "failed to fetch current slot: " + slotErr.Error(),
				cause:   slotErr,
			}
		}

		result, buildErr := wallet.signEscrowDeposit(EscrowDepositParams{
			ProviderWalletB58:  accept.PayTo,
			EscrowProgramIDB58: accept.EscrowProgramID,
			Amount:             amount,
			ServiceID:          serviceID,
			ExpirySlot:         expiry,
		})
		if buildErr != nil {
			return "", &PaymentError{
				Message: "escrow deposit signing failed: " + buildErr.Error(),
				cause:   buildErr,
			}
		}
		depositTx = result.DepositTxB64
		agentPubkey = result.AgentPubkey
	}

	payload := escrowPaymentPayload{
		X402Version: X402Version,
		Resource:    paymentResource{URL: resourceURL, Method: "POST"},
		Accepted:    *accept,
		Payload: escrowPayload{
			DepositTx:   depositTx,
			ServiceID:   serviceIDB64,
			AgentPubkey: agentPubkey,
		},
	}
	return marshalPayload(payload)
}

// marshalPayload JSON-encodes a payload and wraps it in base64.
func marshalPayload(payload any) (string, error) {
	jsonBytes, err := json.Marshal(payload)
	if err != nil {
		return "", err
	}
	return base64.StdEncoding.EncodeToString(jsonBytes), nil
}

// parseAtomicAmount converts a decimal string amount to a uint64 of
// atomic USDC units (6 decimals). Accepts plain integer strings only — the
// 402 response wire format is already in atomic units.
func parseAtomicAmount(s string) (uint64, error) {
	var v uint64
	for _, ch := range s {
		if ch < '0' || ch > '9' {
			return 0, &SigningError{Message: "amount must contain only digits: " + s}
		}
		v = v*10 + uint64(ch-'0')
	}
	return v, nil
}
```

- [ ] **Step 2: Update `client.go:ChatCompletion` call site**

In `sdks/go/client.go` at line 135, replace:
```go
header, headerErr := createPaymentHeader(paymentInfo, url)
```
with:
```go
header, headerErr := createPaymentHeader(paymentInfo, url, c.wallet, body)
```
(Pass the wallet pointer directly — `createPaymentHeader` owns the `HasKey()` branch.)

- [ ] **Step 3: Update any other call sites (especially `client_test.go`)**

Check `sdks/go/client_test.go` and any other file in the package for call sites that reference the old signature of `createPaymentHeader`. The signature change from `(info, url)` to `(info, url, wallet, body)` will fail the build at those call sites if not updated. Grep for `createPaymentHeader(` and update every hit so the test file still compiles.

- [ ] **Step 4: Verify build**

```bash
cd sdks/go && go build ./...
```
Expected: Build succeeds. **This is a hard gate — do NOT move to Task 10 until the entire package compiles.** A build failure here means a call site was missed in Step 3.

- [ ] **Step 5: Commit**

```bash
git add sdks/go/x402.go sdks/go/client.go sdks/go/client_test.go
git commit -m "feat(go-sdk): wire real Solana signing into createPaymentHeader"
```

---

## Task 10: Update `sdks/go/x402_test.go` — Replace Stub Assertions

**Context:** The existing `x402_test.go` asserts that escrow returns an error and that exact returns the stub `"STUB_BASE64_TX"`. These assertions are now incorrect. **This task's author MUST be a different subagent from Task 9.** The updated tests verify:

1. Without a private key, exact-scheme still returns the stub (degraded dev mode).
2. Without a private key, escrow-scheme no longer returns an error — it returns a stub payload with a real `service_id`.
3. Escrow preference (escrow chosen over exact when both present) still works.
4. With a private key (mocked RPC), exact-scheme attempts real signing (and fails cleanly without a reachable RPC, returning a PaymentError wrapping a SigningError).
5. `GenerateServiceID` returns distinct 32-byte values on each call.

**Files:**
- Modify: `sdks/go/x402_test.go`

- [ ] **Step 1: Replace the test file**

```go
package rcr

import (
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"errors"
	"strings"
	"testing"

	"github.com/mr-tron/base58"
)

// TestCreatePaymentHeader_ExactStubModeNoKey verifies the header builder
// returns a stub transaction when no private key is configured.
func TestCreatePaymentHeader_ExactStubModeNoKey(t *testing.T) {
	info := &PaymentRequired{
		X402Version: 2,
		Accepts: []PaymentAccept{{
			Scheme:            "exact",
			Network:           SolanaNetwork,
			Amount:            "1000",
			Asset:             USDCMint,
			PayTo:             "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
			MaxTimeoutSeconds: 300,
		}},
		CostBreakdown: CostBreakdown{Total: "0.001"},
	}

	wallet := NewWallet("") // stub mode
	header, err := createPaymentHeader(info, "/v1/chat/completions", wallet, nil)
	if err != nil {
		t.Fatalf("createPaymentHeader: %v", err)
	}

	decoded, err := base64.StdEncoding.DecodeString(header)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}
	var payload paymentPayload
	if err := json.Unmarshal(decoded, &payload); err != nil {
		t.Fatalf("json decode: %v", err)
	}
	if payload.Payload.Transaction != "STUB_BASE64_TX" {
		t.Errorf("transaction = %q, want STUB_BASE64_TX (stub mode)",
			payload.Payload.Transaction)
	}
}

// TestCreatePaymentHeader_EscrowStubModeNoKey verifies that without a key,
// the escrow scheme returns a stub payload (no longer a hard error).
func TestCreatePaymentHeader_EscrowStubModeNoKey(t *testing.T) {
	info := &PaymentRequired{
		X402Version: 2,
		Accepts: []PaymentAccept{{
			Scheme:            "escrow",
			Network:           SolanaNetwork,
			Amount:            "1000",
			Asset:             USDCMint,
			PayTo:             "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
			MaxTimeoutSeconds: 300,
			EscrowProgramID:   "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		}},
		CostBreakdown: CostBreakdown{Total: "0.001"},
	}

	wallet := NewWallet("") // stub mode
	header, err := createPaymentHeader(info, "/v1/chat/completions", wallet, []byte(`{"model":"test"}`))
	if err != nil {
		t.Fatalf("createPaymentHeader: %v", err)
	}

	decoded, err := base64.StdEncoding.DecodeString(header)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}
	var payload escrowPaymentPayload
	if err := json.Unmarshal(decoded, &payload); err != nil {
		t.Fatalf("json decode: %v", err)
	}
	if payload.Payload.DepositTx != "STUB_ESCROW_DEPOSIT_TX" {
		t.Errorf("deposit_tx = %q, want STUB_ESCROW_DEPOSIT_TX",
			payload.Payload.DepositTx)
	}
	if payload.Payload.AgentPubkey != "STUB_AGENT_PUBKEY" {
		t.Errorf("agent_pubkey = %q, want STUB_AGENT_PUBKEY",
			payload.Payload.AgentPubkey)
	}
	// service_id is a real base64-encoded 32-byte random value, not a stub.
	sidBytes, err := base64.StdEncoding.DecodeString(payload.Payload.ServiceID)
	if err != nil {
		t.Errorf("service_id base64 decode: %v", err)
	}
	if len(sidBytes) != 32 {
		t.Errorf("service_id length = %d, want 32", len(sidBytes))
	}
}

// TestCreatePaymentHeader_PrefersEscrowWhenBothPresent verifies that when a
// 402 response advertises both exact and escrow, the escrow scheme wins.
func TestCreatePaymentHeader_PrefersEscrowWhenBothPresent(t *testing.T) {
	info := &PaymentRequired{
		X402Version: 2,
		Accepts: []PaymentAccept{
			{
				Scheme:            "exact",
				Network:           SolanaNetwork,
				Amount:            "1000",
				Asset:             USDCMint,
				PayTo:             "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
				MaxTimeoutSeconds: 300,
			},
			{
				Scheme:            "escrow",
				Network:           SolanaNetwork,
				Amount:            "1000",
				Asset:             USDCMint,
				PayTo:             "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
				MaxTimeoutSeconds: 300,
				EscrowProgramID:   "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
			},
		},
		CostBreakdown: CostBreakdown{Total: "0.001"},
	}

	wallet := NewWallet("") // stub mode
	header, err := createPaymentHeader(info, "/v1/chat/completions", wallet, nil)
	if err != nil {
		t.Fatalf("createPaymentHeader: %v", err)
	}
	decoded, _ := base64.StdEncoding.DecodeString(header)
	// Expect an escrow payload shape — look for "deposit_tx" key.
	if !strings.Contains(string(decoded), `"deposit_tx"`) {
		t.Error("expected escrow payload (contains deposit_tx); got exact-shaped header")
	}
}

// TestCreatePaymentHeader_ExactKeyButNoRPC verifies that when a wallet has a
// key but SOLANA_RPC_URL is unset, exact-scheme signing returns a PaymentError
// whose underlying cause is a SigningError mentioning SOLANA_RPC_URL.
func TestCreatePaymentHeader_ExactKeyButNoRPC(t *testing.T) {
	// Deterministic keypair (seed [42;32]) — same pattern as builder tests.
	var seed [32]byte
	for i := range seed {
		seed[i] = 42
	}
	priv := ed25519.NewKeyFromSeed(seed[:])
	keypairB58 := base58.Encode(priv)

	t.Setenv("SOLANA_WALLET_KEY", keypairB58)
	t.Setenv("SOLANA_RPC_URL", "") // explicitly unset
	wallet := NewWallet("")        // picks up SOLANA_WALLET_KEY
	if !wallet.HasKey() {
		t.Fatal("wallet should have key")
	}

	info := &PaymentRequired{
		X402Version: 2,
		Accepts: []PaymentAccept{{
			Scheme:            "exact",
			Network:           SolanaNetwork,
			Amount:            "1000",
			Asset:             USDCMint,
			PayTo:             "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
			MaxTimeoutSeconds: 300,
		}},
		CostBreakdown: CostBreakdown{Total: "0.001"},
	}

	_, err := createPaymentHeader(info, "/v1/chat/completions", wallet, nil)
	if err == nil {
		t.Fatal("expected error when key set but SOLANA_RPC_URL unset")
	}
	pe, ok := err.(*PaymentError)
	if !ok {
		t.Fatalf("expected *PaymentError, got %T", err)
	}
	var se *SigningError
	if !errors.As(pe, &se) {
		t.Fatalf("expected PaymentError to wrap *SigningError, got cause %T", pe.Unwrap())
	}
	if !strings.Contains(se.Error(), "SOLANA_RPC_URL") {
		t.Errorf("expected SigningError to mention SOLANA_RPC_URL, got %q", se.Error())
	}
}

// TestCreatePaymentHeader_EscrowKeyButNoRPC verifies the analogous path for
// the escrow scheme.
func TestCreatePaymentHeader_EscrowKeyButNoRPC(t *testing.T) {
	var seed [32]byte
	for i := range seed {
		seed[i] = 42
	}
	priv := ed25519.NewKeyFromSeed(seed[:])
	keypairB58 := base58.Encode(priv)

	t.Setenv("SOLANA_WALLET_KEY", keypairB58)
	t.Setenv("SOLANA_RPC_URL", "") // explicitly unset
	wallet := NewWallet("")
	if !wallet.HasKey() {
		t.Fatal("wallet should have key")
	}

	info := &PaymentRequired{
		X402Version: 2,
		Accepts: []PaymentAccept{{
			Scheme:            "escrow",
			Network:           SolanaNetwork,
			Amount:            "1000",
			Asset:             USDCMint,
			PayTo:             "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
			MaxTimeoutSeconds: 300,
			EscrowProgramID:   "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		}},
		CostBreakdown: CostBreakdown{Total: "0.001"},
	}

	_, err := createPaymentHeader(info, "/v1/chat/completions", wallet, []byte(`{}`))
	if err == nil {
		t.Fatal("expected error when escrow key set but SOLANA_RPC_URL unset")
	}
	pe, ok := err.(*PaymentError)
	if !ok {
		t.Fatalf("expected *PaymentError, got %T", err)
	}
	var se *SigningError
	if !errors.As(pe, &se) {
		t.Fatalf("expected PaymentError to wrap *SigningError, got cause %T", pe.Unwrap())
	}
	if !strings.Contains(se.Error(), "SOLANA_RPC_URL") {
		t.Errorf("expected SigningError to mention SOLANA_RPC_URL, got %q", se.Error())
	}
}

// TestGenerateServiceID_Distinct verifies that two calls produce different IDs.
func TestGenerateServiceID_Distinct(t *testing.T) {
	a, err := GenerateServiceID([]byte("body"))
	if err != nil {
		t.Fatalf("GenerateServiceID: %v", err)
	}
	b, err := GenerateServiceID([]byte("body"))
	if err != nil {
		t.Fatalf("GenerateServiceID: %v", err)
	}
	if a == b {
		t.Error("expected distinct service IDs from separate calls")
	}
}

// TestGenerateServiceID_Length is a sanity check.
func TestGenerateServiceID_Length(t *testing.T) {
	id, err := GenerateServiceID(nil)
	if err != nil {
		t.Fatalf("GenerateServiceID: %v", err)
	}
	if len(id) != 32 {
		t.Errorf("len = %d, want 32", len(id))
	}
}

// TestPaymentAcceptEscrowProgramID keeps the JSON-unmarshal test from the old
// file intact — it's pure type-level and still relevant.
func TestPaymentAcceptEscrowProgramID(t *testing.T) {
	jsonStr := `{
		"scheme": "escrow",
		"network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
		"amount": "5000",
		"asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
		"pay_to": "recipient",
		"max_timeout_seconds": 300,
		"escrow_program_id": "TestEscrowProgram"
	}`
	var accept PaymentAccept
	if err := json.Unmarshal([]byte(jsonStr), &accept); err != nil {
		t.Fatalf("json unmarshal: %v", err)
	}
	if accept.EscrowProgramID != "TestEscrowProgram" {
		t.Errorf("escrow_program_id = %q, want TestEscrowProgram", accept.EscrowProgramID)
	}
}
```

- [ ] **Step 2: Run the updated tests**

```bash
cd sdks/go && go test -v -run 'TestCreatePaymentHeader|TestGenerateServiceID|TestPaymentAcceptEscrow'
```
Expected: All 8 tests pass (3 stub-mode tests + 2 key-without-RPC tests + 2 GenerateServiceID tests + 1 JSON-unmarshal test).

- [ ] **Step 3: Run the full Go SDK suite to ensure no regression**

```bash
cd sdks/go && go test ./...
```
Expected: All tests pass. The old `TestCreatePaymentHeaderEscrowScheme`, `TestCreatePaymentHeaderPrefersEscrow`, `TestCreatePaymentHeaderExactFallback`, and `TestEscrowPayloadMarshal` are replaced; old `containsSubstr` helper removed.

- [ ] **Step 4: Commit**

```bash
git add sdks/go/x402_test.go
git commit -m "test(go-sdk): replace stub assertions with real-signing behavior tests"
```

---

## Task 11: Add `sdks/go/signing_error.go` — `SigningError` Type

**Context:** Python and TypeScript SDKs have a `SigningError` type distinct from generic payment errors (matches `sdks/python/rustyclawrouter/x402.py:18` and `sdks/typescript/src/x402.ts:12-17`). This lets callers distinguish "key wrong / RPC down" (actionable) from "no key configured" (expected fallback). The Go SDK should follow the same convention.

**IMPORTANT DISPATCH ORDER:** This task should be dispatched BEFORE Tasks 2, 4, and 6, because `solana.go` (Task 2 — specifically `decodeAndValidateKeypair`), `exact.go` (Task 4), and `escrow.go` (Task 6) all reference `*SigningError`. If the dispatcher follows the sequential task order in this plan, those executors will stub the type temporarily — cleaner to just dispatch Task 11 first.

**Files:**
- Create: `sdks/go/signing_error.go`

- [ ] **Step 1: Create the file**

```go
package rcr

import "fmt"

// SigningError represents a failure while building or signing a Solana
// transaction. Distinct from PaymentError so callers can distinguish
// configuration issues (PaymentError — no key, no RPC URL set) from active
// cryptographic or RPC failures (SigningError — bad key, RPC unreachable,
// amount invalid).
type SigningError struct {
	Message string
	cause   error
}

// Error implements the error interface.
func (e *SigningError) Error() string {
	return fmt.Sprintf("signing error: %s", e.Message)
}

// Unwrap returns the underlying cause, if any.
func (e *SigningError) Unwrap() error {
	return e.cause
}
```

- [ ] **Step 2: Verify it builds on its own (before Tasks 4/6 land)**

```bash
cd sdks/go && go build ./...
```
Expected: Build succeeds.

- [ ] **Step 3: Commit**

```bash
git add sdks/go/signing_error.go
git commit -m "feat(go-sdk): add SigningError type for crypto/RPC failures"
```

---

## Task 12: Cross-Verification + End-to-End Live Devnet Smoke Test

**Context:** This task has two phases. The first is a **Python cross-verification** that decodes a Go-built transaction with `solders` (an independent implementation of the Solana wire format) and asserts structural invariants — this runs without devnet funds. The second is an optional live devnet smoke test that actually submits the transaction to the network — this requires a real devnet wallet funded with devnet USDC and SOL.

The Python cross-verification is a MASSIVE improvement over "trust the builder" — it lets us detect byte-level drift from the Rust/Python/TS reference implementations without needing a live chain. The live devnet submission is the final ground truth when we can get it.

**Both phases are gated on `RCR_GO_SDK_LIVE_TEST=1` because they require external tooling (`solders`) and devnet funds.**

- [ ] **Step 0: Cross-verify Go output with `solders` (no devnet funds required)**

This step verifies the Go builder produces a wire-format-correct legacy Solana transaction by decoding it with `solders` (an independent Rust-port implementation of the Solana wire format) and asserting hard-coded invariants. It does NOT require devnet SOL or USDC, but it DOES require `SOLANA_RPC_URL` to be reachable so the Go builder can fetch a blockhash.

The flow is a two-part script: a Go helper that builds a deterministic escrow deposit transaction and writes the base64 output to `/tmp/go-sdk-tx.b64`, followed by a Python decoder at `sdks/go/scripts/cross_verify.py` that parses and asserts the invariants.

**Part A — Go helper (`sdks/go/cmd/cross-verify/main.go`):**

Create this tiny helper in the Go SDK (this is a permanent addition — the verifier reuses it every time the plan is re-run):

```go
package main

import (
	"fmt"
	"os"

	"github.com/mr-tron/base58"
	rcr "github.com/rustyclawrouter/sdks/go" // adjust to the real module path
	"crypto/ed25519"
)

// Emits the base64-encoded escrow deposit transaction for the deterministic
// inputs used throughout the test suite (seed=[42]*32, service_id=[0xAB]*32,
// program=9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU, amount=1000).
// Requires SOLANA_RPC_URL to be set so BuildEscrowDeposit can fetch a
// blockhash.
func main() {
	var seed [32]byte
	for i := range seed {
		seed[i] = 42
	}
	priv := ed25519.NewKeyFromSeed(seed[:])
	keypairB58 := base58.Encode(priv)

	var serviceID [32]byte
	for i := range serviceID {
		serviceID[i] = 0xAB
	}

	expiry, err := rcr.GetExpirySlotFromTimeout(os.Getenv("SOLANA_RPC_URL"), 300)
	if err != nil {
		fmt.Fprintln(os.Stderr, "GetExpirySlotFromTimeout:", err)
		os.Exit(1)
	}

	result, err := rcr.BuildEscrowDeposit(rcr.EscrowDepositParams{
		AgentKeypairB58:    keypairB58,
		ProviderWalletB58:  "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		EscrowProgramIDB58: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		Amount:             1000,
		ServiceID:          serviceID,
		ExpirySlot:         expiry,
	})
	if err != nil {
		fmt.Fprintln(os.Stderr, "BuildEscrowDeposit:", err)
		os.Exit(1)
	}
	// Single line of output: the base64 transaction.
	fmt.Println(result.DepositTxB64)
}
```

Build and run it, redirecting the base64 to a temp file:

```bash
cd sdks/go
export SOLANA_RPC_URL=https://api.devnet.solana.com
go run ./cmd/cross-verify > /tmp/go-sdk-tx.b64
```

Expected: `/tmp/go-sdk-tx.b64` contains exactly one line — the base64-encoded wire transaction.

**Part B — Python cross-verifier (`sdks/go/scripts/cross_verify.py`):**

Install the dependency (pick ONE of the two lines — do NOT run both):

```bash
# Option 1: uv (preferred, one-shot)
uv run --with solders python sdks/go/scripts/cross_verify.py

# Option 2: classic pip
pip install 'solders>=0.23'
python3 sdks/go/scripts/cross_verify.py
```

Create `sdks/go/scripts/cross_verify.py` with:

```python
#!/usr/bin/env python3
"""Cross-verify a Go-built escrow deposit transaction against `solders`.

Reads the base64-encoded transaction from /tmp/go-sdk-tx.b64 (produced by
sdks/go/cmd/cross-verify/main.go) and asserts that the wire format matches
the invariants documented in the Wire-Format Contract at the top of the
Go SDK signing plan.
"""

import base64
from nacl.signing import SigningKey
from solders.pubkey import Pubkey
from solders.transaction import Transaction

TX_PATH = "/tmp/go-sdk-tx.b64"
ESCROW_PROGRAM_ID = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"
EXPECTED_DEPOSIT_DISC = "f223c68952e1f2b6"  # sha256("global:deposit")[:8]
EXPECTED_IX_ACCOUNTS = [0, 4, 5, 1, 2, 3, 6, 7, 8]

# Deterministic agent pubkey: seed = [42] * 32
agent_pub = Pubkey.from_bytes(bytes(SigningKey(bytes([42] * 32)).verify_key))
service_id = bytes([0xAB] * 32)
program = Pubkey.from_string(ESCROW_PROGRAM_ID)
expected_escrow_pda, _bump = Pubkey.find_program_address(
    [b"escrow", bytes(agent_pub), service_id], program
)

with open(TX_PATH) as f:
    tx_b64 = f.read().strip()

raw = base64.b64decode(tx_b64)
tx = Transaction.from_bytes(raw)
msg = tx.message

# (a) legacy message (not v0)
assert msg.__class__.__name__ == "Message", (
    f"expected legacy Message, got {msg.__class__.__name__}"
)

# (b) account count = 10
assert len(msg.account_keys) == 10, (
    f"expected 10 account keys, got {len(msg.account_keys)}"
)

# (c) account_keys[0] is the expected agent pubkey
assert msg.account_keys[0] == agent_pub, (
    f"account_keys[0] = {msg.account_keys[0]}, want {agent_pub}"
)

# (d) account_keys[1] is the expected escrow PDA
assert msg.account_keys[1] == expected_escrow_pda, (
    f"account_keys[1] = {msg.account_keys[1]}, want {expected_escrow_pda}"
)

# (e) account_keys[9] is the escrow program ID
assert msg.account_keys[9] == program, (
    f"account_keys[9] = {msg.account_keys[9]}, want {program}"
)

# (f) instruction[0].program_id_index == 9
ix = msg.instructions[0]
assert ix.program_id_index == 9, (
    f"program_id_index = {ix.program_id_index}, want 9"
)

# (g) instruction[0].accounts == [0, 4, 5, 1, 2, 3, 6, 7, 8]
got_accounts = list(ix.accounts)
assert got_accounts == EXPECTED_IX_ACCOUNTS, (
    f"ix accounts = {got_accounts}, want {EXPECTED_IX_ACCOUNTS}"
)

# (h) instruction[0].data[:8] == the deposit discriminator
got_disc = bytes(ix.data)[:8].hex()
assert got_disc == EXPECTED_DEPOSIT_DISC, (
    f"deposit disc = {got_disc}, want {EXPECTED_DEPOSIT_DISC}"
)

# Header sanity: (1 signer, 0 readonly-signed, 6 readonly-unsigned)
h = msg.header
assert (
    h.num_required_signatures == 1
    and h.num_readonly_signed_accounts == 0
    and h.num_readonly_unsigned_accounts == 6
), f"header = {h}, want (1, 0, 6)"

print("cross_verify: OK")
print(f"  agent         = {agent_pub}")
print(f"  escrow PDA    = {expected_escrow_pda}")
print(f"  program       = {program}")
print(f"  deposit disc  = {got_disc}")
print(f"  ix accounts   = {got_accounts}")
```

Expected: The script prints `cross_verify: OK` and zero assertion failures.

If the Rust CLI supports a dry-run/build-only flag, also build the SAME transaction via the Rust reference and assert byte-for-byte equivalence. If no dry-run path exists, note it as a follow-up and proceed with the solders-only decode (which is still a massive improvement over "trust the builder"). The follow-up item: add a `--dry-run` or `--build-only` flag to `rcr chat` that prints the base64 tx to stdout without submitting it — track this in a separate issue.

- [ ] **Step 1: Provision a devnet keypair (verifier only)**

```bash
# Create a fresh devnet wallet if you don't have one
solana-keygen new --outfile /tmp/go-sdk-test-wallet.json --no-bip39-passphrase

# Fund it with devnet SOL (airdrop)
solana airdrop 2 --keypair /tmp/go-sdk-test-wallet.json --url devnet

# Fund it with devnet USDC (use a devnet faucet, e.g. spl-token-faucet.com,
# or the Circle devnet faucet for USDC_DEV).

# Extract the base58-encoded 64-byte keypair
python3 -c "
import json, base58
kp = json.load(open('/tmp/go-sdk-test-wallet.json'))
print(base58.b58encode(bytes(kp)).decode())
" > /tmp/go-sdk-test-wallet.b58
```

**Note on faucet rate limits:** The `solana airdrop 2` CLI command frequently hits devnet faucet rate limits (especially on shared CI runners and shared IP blocks). If you get a `429 Too Many Requests` or `airdrop failed` error, fall back to the faucet web UI at <https://faucet.solana.com> — paste the test wallet's pubkey (read from `solana-keygen pubkey /tmp/go-sdk-test-wallet.json`) and request the airdrop manually. Do NOT retry the CLI command in a tight loop; it just deepens the rate-limit lockout.

- [ ] **Step 2: Run the live test**

```bash
cd sdks/go
export SOLANA_RPC_URL=https://api.devnet.solana.com
export RCR_GO_SDK_LIVE_TEST=1
export SOLANA_WALLET_KEY=$(cat /tmp/go-sdk-test-wallet.b58)
go test -v -run 'TestBuildTransferChecked_Live|TestBuildEscrowDeposit_Live'
```
Expected: Both live tests pass. Base64 output is valid, contains the agent pubkey, discriminator (for escrow), and a plausible signature.

- [ ] **Step 3: Optional — submit the transaction to devnet**

For extra confidence, manually submit the base64 output to devnet:

```bash
# Decode the base64 into a local file (tx.bin), then:
solana send-transaction --url devnet < tx.bin
```
Expected: devnet accepts the transaction OR rejects with a program-level error (e.g. `InsufficientFunds`) — NOT a malformed-transaction error. A malformed-transaction error (`InvalidAccountData`, `AccountNotInitialized 3012`) indicates a wire-format bug.

- [ ] **Step 4: Rotate the test wallet**

Because the test wallet's private key was exported to `/tmp/`, shred it after the test:

```bash
shred -u /tmp/go-sdk-test-wallet.json /tmp/go-sdk-test-wallet.b58
unset SOLANA_WALLET_KEY
```

---

## Task 13: Update `HANDOFF.md`

**Context:** Remove the "Go SDK signing" line from the `## What's NOT Done` → `### Immediate` section now that it's done.

**Files:**
- Modify: `HANDOFF.md:103`

- [ ] **Step 1: Remove the Go SDK signing line**

Delete line 103 of `HANDOFF.md`:
```
- **Go SDK signing**: Still using stub. TypeScript SDK has real signing; Python + CLI have real signing (merged).
```

- [ ] **Step 2: Verify the section still reads correctly**

Eyeball the `## What's NOT Done` → `### Immediate` section: it should now contain 4 items instead of 5, no duplicate blank lines, no orphan bullets.

- [ ] **Step 3: Commit**

```bash
git add HANDOFF.md
git commit -m "docs: mark Go SDK signing complete in HANDOFF"
```

---

## Final Verification

After all tasks land, run the full Go SDK suite one more time plus a workspace build:

```bash
cd sdks/go && go test -v ./...
cd /home/kennethdixon/projects/RustyClawRouter && cargo build
```
Expected:
- Go: all tests pass (22 existing + ~35 new = ~57 total, live + PDA-cross-verify + signature-verify tests skipped when `SOLANA_RPC_URL` / `RCR_GO_SDK_LIVE_TEST` are unset). New tests added in the second fix round: `TestAnchorDiscriminatorRefund`, `TestDecodeAndValidateKeypair_EmptyString`, `TestBuildSolanaTransferChecked_SignatureVerifies`, `TestBuildEscrowDeposit_SignatureVerifies`, and `TestWalletWrongLengthKeyLeavesHasKeyFalse`. The escrow test binary will panic at startup if the `expectedEscrowPDA` placeholder was left in place (intentional — forces the test author to run Task 7 Step 0).
- Cargo: workspace still builds cleanly — no changes to Rust crates should affect this, but verify anyway.

---

## Remember

- **Test authors separate from implementation authors** for Tasks 3, 5, 7, 10 — crypto/on-chain code quality gate.
- **Ground-truth constants come from external sources** (see Invariants table) — do NOT let a test task derive its expected values from the Go code under test. Every constant gets an `EXTERNAL GROUND TRUTH` comment block.
- **`decodeAndValidateKeypair` is the single source of truth for keypair decode.** Both `exact.go` and `escrow.go` must call it. A naive `ed25519.PrivateKey(kpBytes).Public()` comparison is a NO-OP integrity check — the helper re-derives the pubkey from the seed via `NewKeyFromSeed`.
- **No public `Wallet.PrivateKey()` getter.** Signing lives inside package-private `Wallet` methods (`signExactPayment`, `signEscrowDeposit`). The raw key never crosses the type boundary. `createPaymentHeader` takes `*Wallet`, not a raw string.
- **Wire format is byte-compatible with `crates/x402/src/escrow/deposit.rs`** — that file is the arbiter. If you diverge, you break the gateway's facilitator verifier.
- **Task 7's `BuilderPDAMatchesExternal` test must call `BuildEscrowDeposit` and decode the wire bytes** — not just call `findProgramAddress` directly. The point is to test the builder's output, not the helper.
- **Task 12 Step 0 cross-verification with `solders` is mandatory** before the live devnet submission — it catches wire-format drift without needing devnet funds.
- **Account sort order by writability is load-bearing** — if you get it wrong, Anchor will reject with error 3012 `AccountNotInitialized`.
- **`ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL`** — last 5 chars `A8knL`, not `e1bxs`. PR #12 regression.
- **Dispatch Task 11 (`SigningError`) before Tasks 2, 4 and 6** — the helper AND both builders reference it.
- **Dispatch Tasks 4 and 6 before Task 8** — `Wallet` methods call the builders.
- **Dispatch Task 8 before Task 9** — `createPaymentHeader` calls the Wallet methods.
- **`putUint64LE` takes `*[8]byte`, not `[]byte`** — the compiler enforces exactly 8 bytes at every call site.
- No speculative features. No EVM abstractions. No multi-chain. Just close the Go SDK signing gap to match Python/TS.

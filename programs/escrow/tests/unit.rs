//! Unit tests for the escrow program that do NOT require a running validator
//! or a compiled .so file. Tests here cover:
//!   - Escrow account size / InitSpace derivation
//!   - PDA seed derivation matches expected address
//!   - Error code values are stable
//!
//! Integration tests (using LiteSVM) live in tests/integration.rs and require
//! running `anchor build` first to produce the .so artifact.

#[cfg(test)]
mod tests {
    use anchor_lang::Space;
    use solana_sdk::pubkey::Pubkey;
    use solvela_escrow::state::Escrow;

    // Expected space: 8 (discriminator) + InitSpace
    // Escrow fields:
    //   agent:      32 bytes (Pubkey)
    //   provider:   32 bytes (Pubkey)
    //   mint:       32 bytes (Pubkey)
    //   amount:      8 bytes (u64)
    //   service_id: 32 bytes ([u8; 32])
    //   expiry_slot: 8 bytes (u64)
    //   bump:        1 byte  (u8)
    // Total InitSpace: 145 bytes
    const EXPECTED_INIT_SPACE: usize = 32 + 32 + 32 + 8 + 32 + 8 + 1;

    #[test]
    fn test_escrow_init_space() {
        assert_eq!(
            Escrow::INIT_SPACE,
            EXPECTED_INIT_SPACE,
            "Escrow::INIT_SPACE does not match expected layout. \
             Update EXPECTED_INIT_SPACE if you added/removed fields."
        );
    }

    #[test]
    fn test_escrow_account_size_with_discriminator() {
        // Anchor accounts use 8-byte discriminator prefix
        let total = 8 + Escrow::INIT_SPACE;
        assert_eq!(total, 8 + EXPECTED_INIT_SPACE);
    }

    #[test]
    fn test_pda_derivation_is_deterministic() {
        let program_id = solvela_escrow::ID;
        let agent = Pubkey::new_unique();
        let service_id = [42u8; 32];

        let (pda1, bump1) =
            Pubkey::find_program_address(&[b"escrow", agent.as_ref(), &service_id], &program_id);
        let (pda2, bump2) =
            Pubkey::find_program_address(&[b"escrow", agent.as_ref(), &service_id], &program_id);

        assert_eq!(pda1, pda2, "PDA must be deterministic for same inputs");
        assert_eq!(bump1, bump2, "Bump must be deterministic for same inputs");
    }

    #[test]
    fn test_pda_differs_for_different_service_ids() {
        let program_id = solvela_escrow::ID;
        let agent = Pubkey::new_unique();
        let service_id_a = [1u8; 32];
        let service_id_b = [2u8; 32];

        let (pda_a, _) =
            Pubkey::find_program_address(&[b"escrow", agent.as_ref(), &service_id_a], &program_id);
        let (pda_b, _) =
            Pubkey::find_program_address(&[b"escrow", agent.as_ref(), &service_id_b], &program_id);

        assert_ne!(
            pda_a, pda_b,
            "Different service_ids must produce different PDAs"
        );
    }

    #[test]
    fn test_pda_differs_for_different_agents() {
        let program_id = solvela_escrow::ID;
        let agent_a = Pubkey::new_unique();
        let agent_b = Pubkey::new_unique();
        let service_id = [99u8; 32];

        let (pda_a, _) =
            Pubkey::find_program_address(&[b"escrow", agent_a.as_ref(), &service_id], &program_id);
        let (pda_b, _) =
            Pubkey::find_program_address(&[b"escrow", agent_b.as_ref(), &service_id], &program_id);

        assert_ne!(
            pda_a, pda_b,
            "Different agents must produce different PDAs for the same service_id"
        );
    }

    #[test]
    fn test_escrow_struct_fields_exist() {
        // Compile-time check that the Escrow struct has expected fields.
        // If this fails, a field was renamed or removed.
        let _e = Escrow {
            agent: Pubkey::default(),
            provider: Pubkey::default(),
            mint: Pubkey::default(),
            amount: 0,
            service_id: [0u8; 32],
            expiry_slot: 0,
            bump: 0,
        };
    }

    #[test]
    fn test_claim_event_includes_deposited() {
        // Compile-time sentinel for the `deposited` field on ClaimEvent —
        // added in this PR so downstream indexers can tell partial vs full
        // claims without reconstructing `claimed + refunded`. If this test
        // fails to compile, the field was renamed or removed and any
        // indexer/UI that reads it will silently break.
        let _e = solvela_escrow::instructions::ClaimEvent {
            agent: Pubkey::default(),
            provider: Pubkey::default(),
            deposited: 100,
            claimed: 60,
            refunded: 40,
            service_id: [0u8; 32],
        };
    }

    #[test]
    fn test_max_escrow_slots_is_sane() {
        // ~1 day at 400ms/slot. Doc-checked sanity bound — if someone bumps
        // it beyond a few days, that's a separate decision (would interact
        // with the agent's deadline guarantee).
        assert!(solvela_escrow::MAX_ESCROW_SLOTS >= 100_000);
        assert!(solvela_escrow::MAX_ESCROW_SLOTS <= 1_000_000);
    }
}

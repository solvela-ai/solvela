use chrono::Utc;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::orgs::models::{
    AddMemberRequest, ApiKey, ApiKeyCreated, AssignWalletRequest, CreateApiKeyRequest,
    CreateOrgRequest, CreateTeamRequest, OrgMember, OrgRole, Organization, Team, TeamWallet,
};

/// Number of UUID hex chars (no hyphens) included after `"solvela_k_"` in the
/// stored display prefix. The prefix derives from the API-key row's UUID `id`
/// — NOT from the random key material — so reading the `key_prefix` column
/// reveals zero entropy of the underlying key.
///
/// Old-format rows (created before this change) carry a 4-hex prefix sliced
/// from the key bytes themselves; those keep working because `verify_api_key`
/// looks up by `key_hash`, not by prefix. They should be revoked and reissued
/// at the operator's convenience.
const KEY_PREFIX_DISPLAY_UUID_CHARS: usize = 8;

/// Number of random bytes for the API-key body. 32 bytes (256-bit entropy)
/// matches modern bearer-token recommendations (NIST SP 800-63B, OWASP).
const KEY_RANDOM_BYTES: usize = 32;

/// Generate a new API key: `"solvela_k_"` + 64 random hex chars (256 bits).
///
/// Old-format keys generated before this change carry 128 bits of entropy
/// (16 random bytes / 32 hex chars). Both formats are accepted by
/// `verify_api_key` since verification is hash-based.
pub fn generate_api_key() -> String {
    let bytes: [u8; KEY_RANDOM_BYTES] = rand::random();
    format!("solvela_k_{}", hex::encode(bytes))
}

/// Build the displayable, non-secret prefix for an API-key row.
///
/// Format: `"solvela_k_"` + first 8 chars of the row UUID's simple-form (no
/// hyphens). The prefix contains zero bytes of the underlying random key, so
/// even with full read access to the `api_keys` table an attacker cannot use
/// it to reduce the brute-force search space.
fn key_prefix_for_display(row_id: Uuid) -> String {
    let id_simple = row_id.simple().to_string();
    let len = id_simple.len().min(KEY_PREFIX_DISPLAY_UUID_CHARS);
    format!("solvela_k_{}", &id_simple[..len])
}

/// Compute the SHA-256 hex digest of an API key.
pub fn hash_api_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

/// Create a new organization and auto-enroll the owner as a member with role "owner".
///
/// Both INSERTs run inside a single transaction — either both succeed or neither does.
pub async fn create_org(
    pool: &PgPool,
    req: CreateOrgRequest,
    owner_wallet: String,
) -> Result<Organization, sqlx::Error> {
    let id = Uuid::new_v4();
    let now = Utc::now();

    let mut tx = pool.begin().await?;

    let org = sqlx::query_as::<_, Organization>(
        r#"
        INSERT INTO organizations (id, name, slug, owner_wallet, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, name, slug, owner_wallet, created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(&req.name)
    .bind(&req.slug)
    .bind(&owner_wallet)
    .bind(now)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;

    let member_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO org_members (id, org_id, wallet_address, role, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(member_id)
    .bind(org.id)
    .bind(&owner_wallet)
    .bind(OrgRole::Owner)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(org)
}

/// Fetch a single organization by ID.
pub async fn get_org(pool: &PgPool, org_id: Uuid) -> Result<Option<Organization>, sqlx::Error> {
    sqlx::query_as::<_, Organization>(
        r#"
        SELECT id, name, slug, owner_wallet, created_at, updated_at
        FROM organizations
        WHERE id = $1
        "#,
    )
    .bind(org_id)
    .fetch_optional(pool)
    .await
}

/// List all organizations that a wallet belongs to (via org_members).
pub async fn list_orgs_for_wallet(
    pool: &PgPool,
    wallet: &str,
) -> Result<Vec<Organization>, sqlx::Error> {
    sqlx::query_as::<_, Organization>(
        r#"
        SELECT o.id, o.name, o.slug, o.owner_wallet, o.created_at, o.updated_at
        FROM organizations o
        JOIN org_members m ON m.org_id = o.id
        WHERE m.wallet_address = $1
        ORDER BY o.created_at ASC
        "#,
    )
    .bind(wallet)
    .fetch_all(pool)
    .await
}

/// Create a new team within an organization.
pub async fn create_team(
    pool: &PgPool,
    org_id: Uuid,
    req: CreateTeamRequest,
) -> Result<Team, sqlx::Error> {
    let id = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query_as::<_, Team>(
        r#"
        INSERT INTO teams (id, org_id, name, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, org_id, name, created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(org_id)
    .bind(&req.name)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await
}

/// List all teams for an organization.
pub async fn list_teams(pool: &PgPool, org_id: Uuid) -> Result<Vec<Team>, sqlx::Error> {
    sqlx::query_as::<_, Team>(
        r#"
        SELECT id, org_id, name, created_at, updated_at
        FROM teams
        WHERE org_id = $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(org_id)
    .fetch_all(pool)
    .await
}

/// Add a wallet as a member of an organization. Defaults role to "member".
pub async fn add_member(
    pool: &PgPool,
    org_id: Uuid,
    req: AddMemberRequest,
) -> Result<OrgMember, sqlx::Error> {
    let id = Uuid::new_v4();
    let now = Utc::now();
    let role = req.role.unwrap_or(OrgRole::Member);

    sqlx::query_as::<_, OrgMember>(
        r#"
        INSERT INTO org_members (id, org_id, wallet_address, role, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, org_id, wallet_address, role, created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(org_id)
    .bind(&req.wallet_address)
    .bind(role)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await
}

/// List all members of an organization.
pub async fn list_members(pool: &PgPool, org_id: Uuid) -> Result<Vec<OrgMember>, sqlx::Error> {
    sqlx::query_as::<_, OrgMember>(
        r#"
        SELECT id, org_id, wallet_address, role, created_at, updated_at
        FROM org_members
        WHERE org_id = $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(org_id)
    .fetch_all(pool)
    .await
}

/// Assign a wallet address to a team. Requires both `team_id` and `org_id`
/// so the query itself enforces team-belongs-to-org — defense in depth on
/// top of the route-handler check. If `team_id` is not in `org_id`, the
/// `INSERT ... SELECT WHERE EXISTS` returns zero rows and the function
/// surfaces a `RowNotFound` error.
pub async fn assign_wallet(
    pool: &PgPool,
    org_id: Uuid,
    team_id: Uuid,
    req: &AssignWalletRequest,
) -> Result<TeamWallet, sqlx::Error> {
    let id = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query_as::<_, TeamWallet>(
        r#"
        INSERT INTO team_wallets (id, team_id, wallet_address, created_at)
        SELECT $1, $2, $3, $4
        WHERE EXISTS (SELECT 1 FROM teams WHERE id = $2 AND org_id = $5)
        RETURNING id, team_id, wallet_address, created_at
        "#,
    )
    .bind(id)
    .bind(team_id)
    .bind(&req.wallet_address)
    .bind(now)
    .bind(org_id)
    .fetch_one(pool)
    .await
}

/// List all wallets assigned to a team, scoped to the org. The
/// `EXISTS (SELECT 1 FROM teams ...)` guard means a `team_id` that doesn't
/// belong to `org_id` returns an empty list rather than data from another
/// tenant — defense in depth on top of the route-handler check.
pub async fn list_team_wallets(
    pool: &PgPool,
    org_id: Uuid,
    team_id: Uuid,
) -> Result<Vec<TeamWallet>, sqlx::Error> {
    sqlx::query_as::<_, TeamWallet>(
        r#"
        SELECT tw.id, tw.team_id, tw.wallet_address, tw.created_at
        FROM team_wallets tw
        WHERE tw.team_id = $1
          AND EXISTS (SELECT 1 FROM teams WHERE id = $1 AND org_id = $2)
        ORDER BY tw.created_at ASC
        "#,
    )
    .bind(team_id)
    .bind(org_id)
    .fetch_all(pool)
    .await
}

/// Create a new API key for an organization.
///
/// Generates a plaintext key, stores only its SHA-256 hash, and returns the
/// plaintext key once. The plaintext is never persisted.
pub async fn create_api_key(
    pool: &PgPool,
    org_id: Uuid,
    req: CreateApiKeyRequest,
) -> Result<ApiKeyCreated, sqlx::Error> {
    let id = Uuid::new_v4();
    let now = Utc::now();
    let role = req.role.unwrap_or(OrgRole::Member);

    let key = generate_api_key();
    let key_hash = hash_api_key(&key);
    // Display prefix is derived from the row UUID, not the key bytes — see
    // `key_prefix_for_display` for why. This is the H3 fix.
    let key_prefix = key_prefix_for_display(id);

    let expires_at = req
        .expires_in_days
        .map(|days| now + chrono::Duration::days(i64::from(days)));

    sqlx::query(
        r#"
        INSERT INTO api_keys (id, org_id, key_prefix, key_hash, name, role, expires_at, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(id)
    .bind(org_id)
    .bind(&key_prefix)
    .bind(&key_hash)
    .bind(&req.name)
    .bind(role)
    .bind(expires_at)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(ApiKeyCreated {
        id,
        key,
        key_prefix,
        name: req.name,
        role,
        expires_at,
    })
}

/// Verify an API key by hashing it and looking up the hash.
///
/// Returns `Some((api_key_row, org_id))` when valid, `None` when not found,
/// expired, or revoked. Updates `last_used_at` fire-and-forget.
pub async fn verify_api_key(
    pool: &PgPool,
    key: &str,
) -> Result<Option<(ApiKey, Uuid)>, sqlx::Error> {
    let key_hash = hash_api_key(key);
    let now = Utc::now();

    let row = sqlx::query_as::<_, ApiKey>(
        r#"
        SELECT id, org_id, key_prefix, name, role, last_used_at, expires_at, revoked_at, created_at
        FROM api_keys
        WHERE key_hash = $1
          AND revoked_at IS NULL
          AND (expires_at IS NULL OR expires_at > $2)
        "#,
    )
    .bind(&key_hash)
    .bind(now)
    .fetch_optional(pool)
    .await?;

    if let Some(api_key) = row {
        let org_id = api_key.org_id;
        let key_id = api_key.id;

        // Fire-and-forget last_used_at update — never block the caller.
        let pool_clone = pool.clone();
        tokio::spawn(async move {
            let result = sqlx::query(r#"UPDATE api_keys SET last_used_at = $1 WHERE id = $2"#)
                .bind(Utc::now())
                .bind(key_id)
                .execute(&pool_clone)
                .await;

            if let Err(e) = result {
                tracing::warn!(key_id = %key_id, error = %e, "failed to update api key last_used_at");
            }
        });

        Ok(Some((api_key, org_id)))
    } else {
        Ok(None)
    }
}

/// List all (non-revoked) API keys for an organization. Does not return key hashes.
pub async fn list_api_keys(pool: &PgPool, org_id: Uuid) -> Result<Vec<ApiKey>, sqlx::Error> {
    sqlx::query_as::<_, ApiKey>(
        r#"
        SELECT id, org_id, key_prefix, name, role, last_used_at, expires_at, revoked_at, created_at
        FROM api_keys
        WHERE org_id = $1
          AND revoked_at IS NULL
        ORDER BY created_at ASC
        "#,
    )
    .bind(org_id)
    .fetch_all(pool)
    .await
}

/// Revoke an API key. Returns `true` if a row was updated, `false` if not found
/// or already revoked.
pub async fn revoke_api_key(
    pool: &PgPool,
    key_id: Uuid,
    org_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let now = Utc::now();

    let result = sqlx::query(
        r#"
        UPDATE api_keys
        SET revoked_at = $1
        WHERE id = $2 AND org_id = $3 AND revoked_at IS NULL
        "#,
    )
    .bind(now)
    .bind(key_id)
    .bind(org_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_api_key_has_correct_prefix_and_length() {
        let key = generate_api_key();
        // Prefix: "solvela_k_" (10 chars) + 64 hex chars from 32 bytes = 74 total.
        // Bumped from 16 -> 32 bytes (128 -> 256 bits) per H3 fix.
        assert!(
            key.starts_with("solvela_k_"),
            "key should start with 'solvela_k_', got: {key}"
        );
        assert_eq!(
            key.len(),
            74,
            "key should be 74 chars long (10 prefix + 64 hex), got: {}",
            key.len()
        );
    }

    /// Regression: the displayable key prefix must be derived from the row
    /// UUID, NOT the random key bytes. Without this, anyone with read
    /// access to the api_keys table could see the first 16 bits of every
    /// key and reduce brute-force search by that much.
    #[test]
    fn key_prefix_for_display_does_not_leak_key_material() {
        let key = generate_api_key();
        let id = Uuid::new_v4();
        let prefix = key_prefix_for_display(id);

        // Prefix is deterministic from the UUID alone — same UUID always
        // produces the same prefix.
        assert_eq!(prefix, key_prefix_for_display(id));

        // Prefix shape: "solvela_k_" + first 8 chars of UUID simple form.
        assert!(prefix.starts_with("solvela_k_"));
        assert_eq!(prefix.len(), "solvela_k_".len() + 8);

        // The hex chars after the literal "solvela_k_" must come from the
        // UUID, not from the random key. Compare against the first 8 chars
        // of the UUID's simple form.
        let key_random_part = &key["solvela_k_".len()..];
        let prefix_random_part = &prefix["solvela_k_".len()..];
        assert_ne!(
            &key_random_part[..8.min(key_random_part.len())],
            prefix_random_part,
            "prefix must NOT match the first 8 chars of the random key body"
        );
        assert_eq!(prefix_random_part, &id.simple().to_string()[..8]);
    }

    #[test]
    fn hash_api_key_is_deterministic_and_64_chars() {
        let key = "solvela_k_abc123testkey";
        let hash1 = hash_api_key(key);
        let hash2 = hash_api_key(key);

        assert_eq!(hash1, hash2, "hash_api_key must be deterministic");
        assert_eq!(
            hash1.len(),
            64,
            "SHA-256 hex digest must be 64 chars, got: {}",
            hash1.len()
        );

        // Different inputs must produce different hashes
        let hash_other = hash_api_key("solvela_k_different");
        assert_ne!(hash1, hash_other, "different keys must hash differently");
    }
}

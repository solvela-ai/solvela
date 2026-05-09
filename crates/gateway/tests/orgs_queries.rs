//! Integration tests for `gateway::orgs::queries` against a real Postgres.
//!
//! Each `#[sqlx::test]` creates a fresh, isolated database, runs the workspace
//! migrations against it, and tears it down afterwards. Requires the workspace
//! `DATABASE_URL` env var to point at a Postgres where the connecting user can
//! `CREATE DATABASE` (the docker-compose `solvela` superuser satisfies this).

use sqlx::PgPool;
use uuid::Uuid;

use gateway::orgs::models::{
    AddMemberRequest, AssignWalletRequest, CreateApiKeyRequest, CreateOrgRequest,
    CreateTeamRequest, OrgRole,
};
use gateway::orgs::queries::{
    add_member, assign_wallet, create_api_key, create_org, create_team, generate_api_key, get_org,
    hash_api_key, list_api_keys, list_members, list_orgs_for_wallet, list_team_wallets, list_teams,
    revoke_api_key, verify_api_key,
};

const OWNER_WALLET: &str = "DZNuWFpYwzEAcyaMQzqgbxA4d1f4Yq8EU2YbLPLYxNTw";
const SECOND_WALLET: &str = "GbZ6oTmEa9PEd6FGQEmTm3GbXCQkAGqNGGJYXrW8oQte";

fn make_create_request(slug: &str) -> CreateOrgRequest {
    CreateOrgRequest {
        name: format!("Org {slug}"),
        slug: slug.to_string(),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_org_inserts_org_and_owner_member(pool: PgPool) {
    let org = create_org(&pool, make_create_request("acme"), OWNER_WALLET.to_string())
        .await
        .expect("create_org should succeed");

    assert_eq!(org.name, "Org acme");
    assert_eq!(org.slug, "acme");
    assert_eq!(org.owner_wallet, OWNER_WALLET);

    let members = list_members(&pool, org.id)
        .await
        .expect("list_members should succeed");
    assert_eq!(members.len(), 1, "owner must be auto-enrolled");
    assert_eq!(members[0].wallet_address, OWNER_WALLET);
    assert_eq!(members[0].role, OrgRole::Owner);
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_org_rolls_back_on_duplicate_slug(pool: PgPool) {
    create_org(&pool, make_create_request("dup"), OWNER_WALLET.to_string())
        .await
        .expect("first insert succeeds");

    let result = create_org(&pool, make_create_request("dup"), SECOND_WALLET.to_string()).await;
    assert!(result.is_err(), "duplicate slug must reject");

    // Transactional guarantee: no orphan org_members row from the failed second attempt.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM org_members")
        .fetch_one(&pool)
        .await
        .expect("count members");
    assert_eq!(count, 1, "rolled-back tx must leave no orphan members");
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_org_returns_some_for_known_id_and_none_for_unknown(pool: PgPool) {
    let org = create_org(&pool, make_create_request("acme"), OWNER_WALLET.to_string())
        .await
        .expect("create");

    let found = get_org(&pool, org.id).await.expect("query");
    assert!(found.is_some(), "should find existing org");
    assert_eq!(found.unwrap().id, org.id);

    let missing = get_org(&pool, Uuid::new_v4()).await.expect("query");
    assert!(missing.is_none(), "unknown id returns None");
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_orgs_for_wallet_returns_orgs_in_creation_order(pool: PgPool) {
    let _org_a = create_org(
        &pool,
        make_create_request("first"),
        OWNER_WALLET.to_string(),
    )
    .await
    .expect("create first");

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let _org_b = create_org(
        &pool,
        make_create_request("second"),
        OWNER_WALLET.to_string(),
    )
    .await
    .expect("create second");

    let orgs = list_orgs_for_wallet(&pool, OWNER_WALLET)
        .await
        .expect("list");
    assert_eq!(orgs.len(), 2);
    assert!(
        orgs[0].created_at <= orgs[1].created_at,
        "ordering must be ASC by created_at"
    );

    let none = list_orgs_for_wallet(&pool, "unknown_wallet")
        .await
        .expect("list empty");
    assert!(none.is_empty(), "non-member wallet sees no orgs");
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_and_list_teams(pool: PgPool) {
    let org = create_org(&pool, make_create_request("acme"), OWNER_WALLET.to_string())
        .await
        .expect("create org");

    let team_a = create_team(
        &pool,
        org.id,
        CreateTeamRequest {
            name: "Engineering".to_string(),
        },
    )
    .await
    .expect("create team");
    assert_eq!(team_a.org_id, org.id);

    let team_b = create_team(
        &pool,
        org.id,
        CreateTeamRequest {
            name: "Marketing".to_string(),
        },
    )
    .await
    .expect("create second team");
    assert_ne!(team_a.id, team_b.id);

    let teams = list_teams(&pool, org.id).await.expect("list teams");
    assert_eq!(teams.len(), 2);

    let empty = list_teams(&pool, Uuid::new_v4()).await.expect("list empty");
    assert!(empty.is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn add_member_defaults_role_to_member_when_unset(pool: PgPool) {
    let org = create_org(&pool, make_create_request("acme"), OWNER_WALLET.to_string())
        .await
        .expect("create");

    let member = add_member(
        &pool,
        org.id,
        AddMemberRequest {
            wallet_address: SECOND_WALLET.to_string(),
            role: None,
        },
    )
    .await
    .expect("add_member");
    assert_eq!(member.role, OrgRole::Member);
    assert_eq!(member.wallet_address, SECOND_WALLET);
}

#[sqlx::test(migrations = "../../migrations")]
async fn add_member_honors_explicit_admin_role(pool: PgPool) {
    let org = create_org(&pool, make_create_request("acme"), OWNER_WALLET.to_string())
        .await
        .expect("create");

    let member = add_member(
        &pool,
        org.id,
        AddMemberRequest {
            wallet_address: SECOND_WALLET.to_string(),
            role: Some(OrgRole::Admin),
        },
    )
    .await
    .expect("add_member");
    assert_eq!(member.role, OrgRole::Admin);
}

#[sqlx::test(migrations = "../../migrations")]
async fn add_member_rejects_duplicate_wallet_in_same_org(pool: PgPool) {
    let org = create_org(&pool, make_create_request("acme"), OWNER_WALLET.to_string())
        .await
        .expect("create");

    // Owner is already a member; re-adding the same wallet must violate UNIQUE.
    let result = add_member(
        &pool,
        org.id,
        AddMemberRequest {
            wallet_address: OWNER_WALLET.to_string(),
            role: None,
        },
    )
    .await;
    assert!(result.is_err(), "duplicate (org, wallet) must reject");
}

#[sqlx::test(migrations = "../../migrations")]
async fn assign_and_list_team_wallets(pool: PgPool) {
    let org = create_org(&pool, make_create_request("acme"), OWNER_WALLET.to_string())
        .await
        .expect("create org");
    let team = create_team(
        &pool,
        org.id,
        CreateTeamRequest {
            name: "Engineering".to_string(),
        },
    )
    .await
    .expect("create team");

    assign_wallet(
        &pool,
        org.id,
        team.id,
        &AssignWalletRequest {
            wallet_address: SECOND_WALLET.to_string(),
        },
    )
    .await
    .expect("assign");

    let wallets = list_team_wallets(&pool, org.id, team.id)
        .await
        .expect("list wallets");
    assert_eq!(wallets.len(), 1);
    assert_eq!(wallets[0].wallet_address, SECOND_WALLET);
    assert_eq!(wallets[0].team_id, team.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_api_key_returns_plaintext_once_and_persists_hash(pool: PgPool) {
    let org = create_org(&pool, make_create_request("acme"), OWNER_WALLET.to_string())
        .await
        .expect("create org");

    let created = create_api_key(
        &pool,
        org.id,
        CreateApiKeyRequest {
            name: "ci-runner".to_string(),
            role: Some(OrgRole::Admin),
            expires_in_days: Some(30),
        },
    )
    .await
    .expect("create_api_key");

    assert!(created.key.starts_with("solvela_k_"));
    // 74 = "solvela_k_" (10) + 64 hex chars from 32 random bytes (256-bit
    // entropy per H3 fix; was 42 = 10 + 32 hex from 16 bytes pre-fix).
    assert_eq!(created.key.len(), 74);
    assert_eq!(created.role, OrgRole::Admin);
    assert!(
        created.expires_at.is_some(),
        "expires_in_days should populate expires_at"
    );

    let stored_hash: Option<String> =
        sqlx::query_scalar("SELECT key_hash FROM api_keys WHERE id = $1")
            .bind(created.id)
            .fetch_optional(&pool)
            .await
            .expect("query hash");
    assert_eq!(stored_hash, Some(hash_api_key(&created.key)));

    // The plaintext itself does not appear anywhere in the api_keys row.
    let any_plaintext: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM api_keys WHERE key_hash = $1 OR key_prefix = $1 OR name = $1",
    )
    .bind(&created.key)
    .fetch_one(&pool)
    .await
    .expect("query plaintext leak");
    assert_eq!(
        any_plaintext, 0,
        "raw plaintext must never appear in stored columns"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_api_key_without_expiry_leaves_expires_at_null(pool: PgPool) {
    let org = create_org(&pool, make_create_request("acme"), OWNER_WALLET.to_string())
        .await
        .expect("create org");

    let created = create_api_key(
        &pool,
        org.id,
        CreateApiKeyRequest {
            name: "permanent".to_string(),
            role: None,
            expires_in_days: None,
        },
    )
    .await
    .expect("create_api_key");

    assert!(created.expires_at.is_none());
    assert_eq!(created.role, OrgRole::Member, "role defaults to member");
}

#[sqlx::test(migrations = "../../migrations")]
async fn verify_api_key_returns_some_for_valid_key(pool: PgPool) {
    let org = create_org(&pool, make_create_request("acme"), OWNER_WALLET.to_string())
        .await
        .expect("create org");
    let created = create_api_key(
        &pool,
        org.id,
        CreateApiKeyRequest {
            name: "ci".to_string(),
            role: None,
            expires_in_days: None,
        },
    )
    .await
    .expect("create");

    let verified = verify_api_key(&pool, &created.key)
        .await
        .expect("verify")
        .expect("must verify a freshly-created key");
    assert_eq!(verified.0.id, created.id);
    assert_eq!(verified.1, org.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn verify_api_key_returns_none_for_unknown_key(pool: PgPool) {
    let unknown = generate_api_key();
    let result = verify_api_key(&pool, &unknown).await.expect("verify");
    assert!(result.is_none(), "unknown key returns None");
}

#[sqlx::test(migrations = "../../migrations")]
async fn verify_api_key_returns_none_for_revoked_key(pool: PgPool) {
    let org = create_org(&pool, make_create_request("acme"), OWNER_WALLET.to_string())
        .await
        .expect("create org");
    let created = create_api_key(
        &pool,
        org.id,
        CreateApiKeyRequest {
            name: "to-revoke".to_string(),
            role: None,
            expires_in_days: None,
        },
    )
    .await
    .expect("create");

    let updated = revoke_api_key(&pool, created.id, org.id)
        .await
        .expect("revoke");
    assert!(updated, "first revoke returns true");

    let result = verify_api_key(&pool, &created.key).await.expect("verify");
    assert!(result.is_none(), "revoked key must not verify");

    let second = revoke_api_key(&pool, created.id, org.id)
        .await
        .expect("second revoke");
    assert!(!second, "second revoke returns false");
}

#[sqlx::test(migrations = "../../migrations")]
async fn verify_api_key_returns_none_for_expired_key(pool: PgPool) {
    let org = create_org(&pool, make_create_request("acme"), OWNER_WALLET.to_string())
        .await
        .expect("create org");

    // Insert an expired key directly so we don't have to wait.
    let key = generate_api_key();
    let key_hash = hash_api_key(&key);
    sqlx::query(
        r#"INSERT INTO api_keys (id, org_id, key_prefix, key_hash, name, role, expires_at, created_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
    )
    .bind(Uuid::new_v4())
    .bind(org.id)
    .bind(&key[..14])
    .bind(&key_hash)
    .bind("expired")
    .bind(OrgRole::Member)
    .bind(chrono::Utc::now() - chrono::Duration::days(1))
    .bind(chrono::Utc::now() - chrono::Duration::days(2))
    .execute(&pool)
    .await
    .expect("insert expired key");

    let result = verify_api_key(&pool, &key).await.expect("verify");
    assert!(result.is_none(), "expired key must not verify");
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_api_keys_filters_revoked(pool: PgPool) {
    let org = create_org(&pool, make_create_request("acme"), OWNER_WALLET.to_string())
        .await
        .expect("create org");

    let active = create_api_key(
        &pool,
        org.id,
        CreateApiKeyRequest {
            name: "active".to_string(),
            role: None,
            expires_in_days: None,
        },
    )
    .await
    .expect("create active");

    let to_revoke = create_api_key(
        &pool,
        org.id,
        CreateApiKeyRequest {
            name: "to-revoke".to_string(),
            role: None,
            expires_in_days: None,
        },
    )
    .await
    .expect("create to-revoke");

    revoke_api_key(&pool, to_revoke.id, org.id)
        .await
        .expect("revoke");

    let listed = list_api_keys(&pool, org.id).await.expect("list");
    assert_eq!(listed.len(), 1, "revoked keys are filtered out");
    assert_eq!(listed[0].id, active.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn revoke_api_key_returns_false_for_unknown_id(pool: PgPool) {
    let org = create_org(&pool, make_create_request("acme"), OWNER_WALLET.to_string())
        .await
        .expect("create org");

    let updated = revoke_api_key(&pool, Uuid::new_v4(), org.id)
        .await
        .expect("revoke unknown");
    assert!(!updated, "revoking a non-existent key returns false");
}

#[sqlx::test(migrations = "../../migrations")]
async fn revoke_api_key_scopes_to_org(pool: PgPool) {
    let org_a = create_org(&pool, make_create_request("a"), OWNER_WALLET.to_string())
        .await
        .expect("create a");
    let org_b = create_org(&pool, make_create_request("b"), SECOND_WALLET.to_string())
        .await
        .expect("create b");

    let created = create_api_key(
        &pool,
        org_a.id,
        CreateApiKeyRequest {
            name: "for-a".to_string(),
            role: None,
            expires_in_days: None,
        },
    )
    .await
    .expect("create");

    // Org B must NOT be able to revoke org A's key.
    let cross = revoke_api_key(&pool, created.id, org_b.id)
        .await
        .expect("attempt cross-org revoke");
    assert!(
        !cross,
        "revoke must scope to (org_id, key_id) — cross-org revoke must not flip the row"
    );

    // The key still verifies.
    let verified = verify_api_key(&pool, &created.key).await.expect("verify");
    assert!(
        verified.is_some(),
        "key targeted by a cross-org revoke must remain valid"
    );
}

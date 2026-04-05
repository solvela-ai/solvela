use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub owner_wallet: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Team {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct OrgMember {
    pub id: Uuid,
    pub org_id: Uuid,
    pub wallet_address: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TeamWallet {
    pub id: Uuid,
    pub team_id: Uuid,
    pub wallet_address: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ApiKey {
    pub id: Uuid,
    pub org_id: Uuid,
    pub key_prefix: String,
    pub name: String,
    pub role: String,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

// Request types (no FromRow needed — these are input types)
#[derive(Debug, Clone, Deserialize)]
pub struct CreateOrgRequest {
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateTeamRequest {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddMemberRequest {
    pub wallet_address: String,
    pub role: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssignWalletRequest {
    pub wallet_address: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateApiKeyRequest {
    pub name: String,
    pub role: Option<String>,
    pub expires_in_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiKeyCreated {
    pub id: Uuid,
    pub key: String,
    pub key_prefix: String,
    pub name: String,
    pub role: String,
    pub expires_at: Option<DateTime<Utc>>,
}

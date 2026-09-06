//! The Portal's own PostgreSQL: the two things that are neither configuration nor context data.
//! Preferences are what a person likes about the UI (UI-09); the key rows are what is left of an
//! API key once its secret has been hashed and forgotten (PF-36). Everything else lives in Git or
//! in the broker.

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;
use time::OffsetDateTime;

/// Connects and brings the schema up to date. Migrations are embedded, so a fresh database
/// needs no step outside the binary; running them twice is a no-op.
pub async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(5))
        .connect(url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

/// The stored document of one subject, `None` before the first save.
pub async fn load_preferences(
    pool: &PgPool,
    subject: &str,
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT preferences FROM user_preferences WHERE subject = $1",
    )
    .bind(subject)
    .fetch_optional(pool)
    .await
}

/// Replaces the subject's document whole (UI-10: immediate, no Git involved).
pub async fn save_preferences(
    pool: &PgPool,
    subject: &str,
    preferences: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO user_preferences (subject, preferences, updated_at) VALUES ($1, $2, now()) \
         ON CONFLICT (subject) DO UPDATE SET preferences = EXCLUDED.preferences, updated_at = now()",
    )
    .bind(subject)
    .bind(preferences)
    .execute(pool)
    .await
    .map(|_| ())
}

/// One API key of a ServiceAccount, as the database keeps it (PF-36).
///
/// There is no secret here and there is nowhere to put one: `secret_hash` is the Argon2id PHC
/// string the Context Gateway verifies against, and the raw token exists only in the answer to
/// the request that minted it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct KeyRow {
    pub key_id: String,
    pub project: String,
    pub account: String,
    pub credential: String,
    pub secret_hash: String,
    pub created_at: OffsetDateTime,
    pub created_by: String,
    pub expires_at: Option<OffsetDateTime>,
    pub last_used_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
}

/// Stores a freshly minted key. The caller has already hashed the secret and thrown it away.
pub async fn insert_key(pool: &PgPool, row: &KeyRow) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO service_account_keys (key_id, project, account, credential, secret_hash, \
         created_at, created_by, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(&row.key_id)
    .bind(&row.project)
    .bind(&row.account)
    .bind(&row.credential)
    .bind(&row.secret_hash)
    .bind(row.created_at)
    .bind(&row.created_by)
    .bind(row.expires_at)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Every key of one account, newest first. Revoked keys stay in the list: an operator has to be
/// able to see that a key existed and when it stopped working.
pub async fn list_keys(
    pool: &PgPool,
    project: &str,
    account: &str,
) -> Result<Vec<KeyRow>, sqlx::Error> {
    sqlx::query_as::<_, KeyRow>(
        "SELECT key_id, project, account, credential, secret_hash, created_at, created_by, \
         expires_at, last_used_at, revoked_at FROM service_account_keys \
         WHERE project = $1 AND account = $2 ORDER BY created_at DESC",
    )
    .bind(project)
    .bind(account)
    .fetch_all(pool)
    .await
}

/// One key of one account. The project and the account are part of the lookup, so a key id from
/// another project is a miss rather than a read of someone else's row.
pub async fn get_key(
    pool: &PgPool,
    project: &str,
    account: &str,
    key_id: &str,
) -> Result<Option<KeyRow>, sqlx::Error> {
    sqlx::query_as::<_, KeyRow>(
        "SELECT key_id, project, account, credential, secret_hash, created_at, created_by, \
         expires_at, last_used_at, revoked_at FROM service_account_keys \
         WHERE project = $1 AND account = $2 AND key_id = $3",
    )
    .bind(project)
    .bind(account)
    .bind(key_id)
    .fetch_optional(pool)
    .await
}

/// Moves a key's expiry, which is how a rotation ends the predecessor's overlap window (PF-38).
/// A key that already expires earlier keeps its own expiry: rotation never extends a key.
pub async fn expire_key_at(
    pool: &PgPool,
    key_id: &str,
    at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE service_account_keys SET expires_at = LEAST(COALESCE(expires_at, $2), $2) \
         WHERE key_id = $1",
    )
    .bind(key_id)
    .bind(at)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Revokes a key now (PF-38). The row is kept with `revoked_at` set, so the audit trail survives;
/// `false` means no such key, which the caller turns into a 404.
pub async fn revoke_key(
    pool: &PgPool,
    project: &str,
    account: &str,
    key_id: &str,
    at: OffsetDateTime,
) -> Result<bool, sqlx::Error> {
    sqlx::query(
        "UPDATE service_account_keys SET revoked_at = COALESCE(revoked_at, $4), \
         expires_at = LEAST(COALESCE(expires_at, $4), $4) \
         WHERE project = $1 AND account = $2 AND key_id = $3",
    )
    .bind(project)
    .bind(account)
    .bind(key_id)
    .bind(at)
    .execute(pool)
    .await
    .map(|result| result.rows_affected() > 0)
}

//! The Portal's own PostgreSQL: the things that are neither configuration nor context data.
//! Preferences are what a person likes about the UI (UI-09); the key rows are what is left of an
//! API key once its secret has been hashed and forgotten (PF-36); a builder run and its event
//! stream are a conversation, which no manifest can hold (AG-43, AG-45). Everything else lives
//! in Git or in the broker.

use crate::agents::run::{AgentRun, AgentRunEvent};
// The four agent-run reads interpolate a `const` column list into their statement and nothing
// else: no caller value ever reaches the SQL text, every value is a bound parameter. That is
// what the wrapper asserts.
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::AssertSqlSafe;
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

/// What one `SyncSource` run left behind (MF-30).
///
/// The row is the loop's whole memory: which revision of the source this repository carries,
/// when the last run happened, the proposal a run opened and nobody has answered yet, and
/// whether an operator switched syncing off. Nothing about the source itself is here — the
/// manifest is in Git and the credential is in the secret store.
#[derive(Debug, Clone, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct SyncStateRow {
    pub namespace: String,
    pub name: String,
    pub observed_revision: Option<String>,
    pub last_run_at: Option<i64>,
    /// The branch of the open proposal, which is `jcctl::sync::proposal_name`.
    pub open_proposal: Option<String>,
    /// The source revision that proposal carries; the branch holds only its first characters.
    pub open_revision: Option<String>,
    /// Where a reviewer answers it.
    pub merge_request: Option<String>,
    /// Why the last run did not finish, cleared by the next run that does.
    pub last_error: Option<String>,
    pub paused: bool,
}

/// The state of one source, `None` before its first run.
pub async fn load_sync_state(
    pool: &PgPool,
    namespace: &str,
    name: &str,
) -> Result<Option<SyncStateRow>, sqlx::Error> {
    sqlx::query_as::<_, SyncStateRow>(
        "SELECT namespace, name, observed_revision, last_run_at, open_proposal, open_revision, \
         merge_request, last_error, paused FROM sync_source_state \
         WHERE namespace = $1 AND name = $2",
    )
    .bind(namespace)
    .bind(name)
    .fetch_optional(pool)
    .await
}

/// Replaces the row whole: a run's state is one value, and writing half of it is how a
/// restarted Portal would re-propose a revision it had already proposed.
pub async fn save_sync_state(pool: &PgPool, row: &SyncStateRow) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO sync_source_state (namespace, name, observed_revision, last_run_at, \
         open_proposal, open_revision, merge_request, last_error, paused, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now()) \
         ON CONFLICT (namespace, name) DO UPDATE SET \
         observed_revision = EXCLUDED.observed_revision, last_run_at = EXCLUDED.last_run_at, \
         open_proposal = EXCLUDED.open_proposal, open_revision = EXCLUDED.open_revision, \
         merge_request = EXCLUDED.merge_request, last_error = EXCLUDED.last_error, \
         paused = EXCLUDED.paused, updated_at = now()",
    )
    .bind(&row.namespace)
    .bind(&row.name)
    .bind(&row.observed_revision)
    .bind(row.last_run_at)
    .bind(&row.open_proposal)
    .bind(&row.open_revision)
    .bind(&row.merge_request)
    .bind(&row.last_error)
    .bind(row.paused)
    .execute(pool)
    .await
    .map(|_| ())
}

/// The columns of `agent_runs` in the order [`AgentRun`](crate::agents::run::AgentRun) declares
/// them, with the four timestamps already rendered as the RFC 3339 strings the API answers.
///
/// Postgres does that rendering because `time` is compiled here with `formatting` and no parser:
/// reading a `timestamptz` into the struct would need one. `to_char` of a NULL column is NULL, so
/// the three optional timestamps stay optional.
const AGENT_RUN_COLUMNS: &str = "id, project, app_name, endpoint_name, endpoint_slug, profile, \
     app_class, visibility, prompt, prompt_digest, data_needs, allows_write, branch, path_prefix, \
     status, ticket_hash, workspace, merge_request, preview_url, files, steps, tokens_used, created_by, \
     to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS created_at, \
     to_char(started_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS started_at, \
     to_char(finished_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS finished_at, \
     to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS expires_at, \
     error";

/// The same treatment for one event of the stream.
const AGENT_EVENT_COLUMNS: &str = "run_id, seq, kind, payload, \
     to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS created_at";

/// Records a run at the moment it is admitted, ticket hash included (AG-43).
///
/// The two timestamps arrive as the RFC 3339 text the caller minted, and the cast is written into
/// the statement so the parameter stays a string on this side.
pub async fn insert_agent_run(pool: &PgPool, run: &AgentRun) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO agent_runs (id, project, app_name, endpoint_name, endpoint_slug, profile, \
         app_class, visibility, prompt, prompt_digest, data_needs, allows_write, branch, \
         path_prefix, status, ticket_hash, steps, tokens_used, created_by, created_at, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, \
         $19, $20::text::timestamptz, $21::text::timestamptz)",
    )
    .bind(&run.id)
    .bind(&run.project)
    .bind(&run.app_name)
    .bind(&run.endpoint_name)
    .bind(&run.endpoint_slug)
    .bind(&run.profile)
    .bind(&run.app_class)
    .bind(&run.visibility)
    .bind(&run.prompt)
    .bind(&run.prompt_digest)
    .bind(&run.data_needs)
    .bind(run.allows_write)
    .bind(&run.branch)
    .bind(&run.path_prefix)
    .bind(&run.status)
    .bind(&run.ticket_hash)
    .bind(run.steps)
    .bind(run.tokens_used)
    .bind(&run.created_by)
    .bind(&run.created_at)
    .bind(&run.expires_at)
    .execute(pool)
    .await
    .map(|_| ())
}

/// One run by id, `None` when there is no such row.
pub async fn load_agent_run(pool: &PgPool, id: &str) -> Result<Option<AgentRun>, sqlx::Error> {
    sqlx::query_as::<_, AgentRun>(AssertSqlSafe(format!(
        "SELECT {AGENT_RUN_COLUMNS} FROM agent_runs WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// The newest runs of one project.
pub async fn list_agent_runs(
    pool: &PgPool,
    project: &str,
    limit: i64,
) -> Result<Vec<AgentRun>, sqlx::Error> {
    sqlx::query_as::<_, AgentRun>(AssertSqlSafe(format!(
        "SELECT {AGENT_RUN_COLUMNS} FROM agent_runs WHERE project = $1 \
         ORDER BY created_at DESC LIMIT $2"
    )))
    .bind(project)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Moves a run to another state. `started_at` is stamped by the first state after `queued` and
/// `finished_at` by the state the run stops in, so neither is ever moved twice.
pub async fn update_agent_run_status(
    pool: &PgPool,
    id: &str,
    status: &str,
    error: Option<&str>,
    finished: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_runs SET status = $2, error = COALESCE($3, error), \
         started_at = CASE WHEN started_at IS NULL AND $2 <> 'queued' THEN now() ELSE started_at END, \
         finished_at = CASE WHEN $4 AND finished_at IS NULL THEN now() ELSE finished_at END \
         WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(error)
    .bind(finished)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Forgets a run's ticket hash, which is what a cancellation does to its workspace's credential:
/// every later proxy call verifies against a hash no ticket can produce (AG-46).
pub async fn clear_agent_run_ticket(pool: &PgPool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_runs SET ticket_hash = '' WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Adds one step's token use to the run's budget counter (AG-44).
pub async fn add_agent_run_usage(
    pool: &PgPool,
    id: &str,
    tokens: i64,
    steps: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_runs SET tokens_used = tokens_used + $2, steps = steps + $3 WHERE id = $1",
    )
    .bind(id)
    .bind(tokens)
    .bind(steps)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Records where the built application can be looked at (AP-46).
pub async fn set_agent_run_preview_url(
    pool: &PgPool,
    id: &str,
    url: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_runs SET preview_url = $2 WHERE id = $1")
        .bind(id)
        .bind(url)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Keeps the files a kit pass wrote (AP-56).
pub async fn set_agent_run_files(
    pool: &PgPool,
    id: &str,
    files: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_runs SET files = $2 WHERE id = $1")
        .bind(id)
        .bind(files)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Records the merge request a publish opened (AP-55).
pub async fn set_agent_run_merge_request(
    pool: &PgPool,
    id: &str,
    number: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_runs SET merge_request = $2 WHERE id = $1")
        .bind(id)
        .bind(number)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Appends one event and answers with the sequence number it got (AG-45).
///
/// The number is allocated inside the statement, so a reader that asked for everything after
/// `seq` never sees a gap fill in behind it. Two writers can still pick the same number — the
/// Portal's own status events and the proxy's relay are separate callers — and the primary key
/// is what catches that; the retry is the whole handling, because the loser only needs the next
/// number.
pub async fn append_agent_run_event(
    pool: &PgPool,
    run_id: &str,
    kind: &str,
    payload: &serde_json::Value,
) -> Result<AgentRunEvent, sqlx::Error> {
    let statement = format!(
        "INSERT INTO agent_run_events (run_id, seq, kind, payload) \
         SELECT $1, COALESCE(MAX(seq), 0) + 1, $2, $3 FROM agent_run_events WHERE run_id = $1 \
         RETURNING {AGENT_EVENT_COLUMNS}"
    );
    let mut last = None;
    for _ in 0..3 {
        match sqlx::query_as::<_, AgentRunEvent>(AssertSqlSafe(statement.clone()))
            .bind(run_id)
            .bind(kind)
            .bind(payload)
            .fetch_one(pool)
            .await
        {
            Ok(event) => return Ok(event),
            Err(err) if is_unique_violation(&err) => last = Some(err),
            Err(err) => return Err(err),
        }
    }
    Err(last.unwrap_or(sqlx::Error::RowNotFound))
}

/// Everything a stream missed, oldest first (`Last-Event-ID`, AG-45).
pub async fn load_agent_run_events(
    pool: &PgPool,
    run_id: &str,
    after_seq: i64,
) -> Result<Vec<AgentRunEvent>, sqlx::Error> {
    sqlx::query_as::<_, AgentRunEvent>(AssertSqlSafe(format!(
        "SELECT {AGENT_EVENT_COLUMNS} FROM agent_run_events WHERE run_id = $1 AND seq > $2 \
         ORDER BY seq"
    )))
    .bind(run_id)
    .bind(after_seq)
    .fetch_all(pool)
    .await
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db) if db.code().as_deref() == Some("23505"))
}

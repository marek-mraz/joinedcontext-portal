//! What a builder run and its conversation are, between the request that started it and the
//! answer that ends it (AG-43, AG-45).
//!
//! Postgres when the Portal has one, and a map when it does not, which is the shape
//! [`crate::sync::state::States`] already uses for the other loop that has to remember
//! something. A Portal without a database is the single-container development shape: a run
//! still runs and still streams, and the cost of a restart is that every in-flight run's
//! ticket is gone with it. That is the safe direction — the workspace's next proxy call is
//! refused rather than served against a run nobody can see any more — and the Job's own
//! deadline removes the pod.

use std::collections::HashMap;

use tokio::sync::RwLock;

use crate::agents::run::{AgentRun, AgentRunEvent, AgentRunStatus};
use crate::db;

/// Why a run could not be read or written. The API turns it into one 503: a builder run is not
/// something to answer half of.
#[derive(Debug, thiserror::Error)]
#[error("the agent run store did not answer: {0}")]
pub struct StoreError(#[from] sqlx::Error);

#[derive(Debug, Default)]
struct Memory {
    runs: HashMap<String, AgentRun>,
    events: HashMap<String, Vec<AgentRunEvent>>,
}

/// Every run of this Portal, by id.
#[derive(Debug)]
pub struct AgentStore {
    db: Option<sqlx::PgPool>,
    memory: RwLock<Memory>,
}

impl AgentStore {
    /// Backed by the database when there is one, by a map when there is not.
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        Self {
            db,
            memory: RwLock::new(Memory::default()),
        }
    }

    /// Whether a restart keeps what this remembers, and so whether a run survives a rollout.
    pub fn is_durable(&self) -> bool {
        self.db.is_some()
    }

    pub async fn create_run(&self, run: &AgentRun) -> Result<(), StoreError> {
        if let Some(pool) = &self.db {
            db::insert_agent_run(pool, run).await?;
            return Ok(());
        }
        let mut memory = self.memory.write().await;
        memory.runs.insert(run.id.clone(), run.clone());
        Ok(())
    }

    pub async fn get_run(&self, id: &str) -> Result<Option<AgentRun>, StoreError> {
        if let Some(pool) = &self.db {
            return Ok(db::load_agent_run(pool, id).await?);
        }
        Ok(self.memory.read().await.runs.get(id).cloned())
    }

    pub async fn list_runs(&self, project: &str, limit: i64) -> Result<Vec<AgentRun>, StoreError> {
        if let Some(pool) = &self.db {
            return Ok(db::list_agent_runs(pool, project, limit).await?);
        }
        let memory = self.memory.read().await;
        let mut runs: Vec<AgentRun> = memory
            .runs
            .values()
            .filter(|run| run.project == project)
            .cloned()
            .collect();
        // Created in the same second sorts by id, so a page is stable between two calls.
        runs.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        runs.truncate(limit.max(0) as usize);
        Ok(runs)
    }

    /// Moves a run to `next`, refusing a transition the lifecycle does not have (AG-43).
    ///
    /// The refusal is the answer, not a log line: a cancel of a published run and a second
    /// publish of the same run both arrive here, and both are conflicts rather than writes.
    pub async fn set_status(
        &self,
        id: &str,
        next: AgentRunStatus,
        error: Option<&str>,
    ) -> Result<AgentRun, StatusChangeError> {
        let run = self
            .get_run(id)
            .await
            .map_err(StatusChangeError::Store)?
            .ok_or(StatusChangeError::Unknown)?;
        let current = AgentRunStatus::parse(&run.status).ok_or(StatusChangeError::Unknown)?;
        if !current.allows_transition_to(next) {
            return Err(StatusChangeError::Refused {
                from: current,
                to: next,
            });
        }

        if let Some(pool) = &self.db {
            db::update_agent_run_status(pool, id, next.as_str(), error, next.is_terminal())
                .await
                .map_err(|err| StatusChangeError::Store(StoreError(err)))?;
            return self
                .get_run(id)
                .await
                .map_err(StatusChangeError::Store)?
                .ok_or(StatusChangeError::Unknown);
        }

        let mut memory = self.memory.write().await;
        let stored = memory.runs.get_mut(id).ok_or(StatusChangeError::Unknown)?;
        stored.status = next.as_str().to_owned();
        if let Some(message) = error {
            stored.error = Some(message.to_owned());
        }
        if stored.started_at.is_none() && next != AgentRunStatus::Queued {
            stored.started_at = Some(now_rfc3339());
        }
        if next.is_terminal() && stored.finished_at.is_none() {
            stored.finished_at = Some(now_rfc3339());
        }
        Ok(stored.clone())
    }

    /// Forgets the run's ticket hash. Every later proxy call for it verifies against a hash no
    /// ticket can produce, so a cancelled workspace cannot reach the model, the data or the
    /// forge again (AG-46).
    pub async fn invalidate_ticket(&self, id: &str) -> Result<(), StoreError> {
        if let Some(pool) = &self.db {
            db::clear_agent_run_ticket(pool, id).await?;
            return Ok(());
        }
        if let Some(run) = self.memory.write().await.runs.get_mut(id) {
            run.ticket_hash.clear();
        }
        Ok(())
    }

    /// Appends one event and answers with the sequence number it got.
    pub async fn append_event(
        &self,
        run_id: &str,
        kind: &str,
        payload: serde_json::Value,
    ) -> Result<AgentRunEvent, StoreError> {
        if let Some(pool) = &self.db {
            return Ok(db::append_agent_run_event(pool, run_id, kind, &payload).await?);
        }
        let mut memory = self.memory.write().await;
        let stream = memory.events.entry(run_id.to_owned()).or_default();
        let event = AgentRunEvent {
            run_id: run_id.to_owned(),
            seq: stream.len() as i64 + 1,
            kind: kind.to_owned(),
            payload,
            created_at: now_rfc3339(),
        };
        stream.push(event.clone());
        Ok(event)
    }

    /// Everything after `after_seq`, oldest first: what a reconnecting stream missed.
    pub async fn events_since(
        &self,
        run_id: &str,
        after_seq: i64,
    ) -> Result<Vec<AgentRunEvent>, StoreError> {
        if let Some(pool) = &self.db {
            return Ok(db::load_agent_run_events(pool, run_id, after_seq).await?);
        }
        Ok(self
            .memory
            .read()
            .await
            .events
            .get(run_id)
            .map(|stream| {
                stream
                    .iter()
                    .filter(|event| event.seq > after_seq)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Adds one step's token use to the run's counters (AG-44).
    pub async fn record_usage(
        &self,
        run_id: &str,
        tokens: i64,
        steps: i32,
    ) -> Result<(), StoreError> {
        if let Some(pool) = &self.db {
            db::add_agent_run_usage(pool, run_id, tokens, steps).await?;
            return Ok(());
        }
        if let Some(run) = self.memory.write().await.runs.get_mut(run_id) {
            run.tokens_used += tokens;
            run.steps += steps;
        }
        Ok(())
    }

    pub async fn set_preview_url(&self, run_id: &str, url: &str) -> Result<(), StoreError> {
        if let Some(pool) = &self.db {
            db::set_agent_run_preview_url(pool, run_id, url).await?;
            return Ok(());
        }
        if let Some(run) = self.memory.write().await.runs.get_mut(run_id) {
            run.preview_url = Some(url.to_owned());
        }
        Ok(())
    }

    pub async fn set_merge_request(&self, run_id: &str, number: i32) -> Result<(), StoreError> {
        if let Some(pool) = &self.db {
            db::set_agent_run_merge_request(pool, run_id, number).await?;
            return Ok(());
        }
        if let Some(run) = self.memory.write().await.runs.get_mut(run_id) {
            run.merge_request = Some(number);
        }
        Ok(())
    }
}

/// Why a state change did not happen.
#[derive(Debug, thiserror::Error)]
pub enum StatusChangeError {
    #[error("no such run, or a state this Portal does not know")]
    Unknown,
    #[error("a run in '{from}' does not move to '{to}'", from = from.as_str(), to = to.as_str())]
    Refused {
        from: AgentRunStatus,
        to: AgentRunStatus,
    },
    #[error(transparent)]
    Store(StoreError),
}

pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, project: &str, created_at: &str) -> AgentRun {
        AgentRun {
            id: id.to_owned(),
            project: project.to_owned(),
            app_name: "city-bikes-overview".to_owned(),
            endpoint_name: "helsinki-bikes".to_owned(),
            endpoint_slug: "si6epqkx364lprho5uaigutk274r5grb".to_owned(),
            profile: "app-builder".to_owned(),
            app_class: "static".to_owned(),
            visibility: "project".to_owned(),
            prompt: "a live bike availability dashboard".to_owned(),
            prompt_digest: "sha256:abc".to_owned(),
            data_needs: serde_json::json!([]),
            allows_write: false,
            branch: format!("agent/app-city-bikes-overview/{id}"),
            path_prefix: "projects/helsinki/apps/city-bikes-overview/".to_owned(),
            status: AgentRunStatus::Queued.as_str().to_owned(),
            ticket_hash: "$argon2id$stub".to_owned(),
            workspace: None,
            merge_request: None,
            preview_url: None,
            steps: 0,
            tokens_used: 0,
            created_by: "demo.steward@hel.fi".to_owned(),
            created_at: created_at.to_owned(),
            started_at: None,
            finished_at: None,
            expires_at: "2026-09-12T10:35:30Z".to_owned(),
            error: None,
        }
    }

    #[tokio::test]
    async fn a_run_is_read_back_and_listed_inside_its_own_project() {
        let store = AgentStore::new(None);
        assert!(!store.is_durable());
        store
            .create_run(&run("run-1", "helsinki", "2026-09-12T10:15:30Z"))
            .await
            .expect("create");
        store
            .create_run(&run("run-2", "helsinki", "2026-09-12T10:16:30Z"))
            .await
            .expect("create");
        store
            .create_run(&run("run-3", "bb", "2026-09-12T10:17:30Z"))
            .await
            .expect("create");

        let listed = store.list_runs("helsinki", 10).await.expect("list");
        assert_eq!(
            listed.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["run-2", "run-1"],
            "newest first, and another project's run is not in this project's list"
        );
        assert_eq!(store.list_runs("helsinki", 1).await.expect("list").len(), 1);
        assert!(store.get_run("run-4").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn the_lifecycle_is_what_the_store_allows() {
        let store = AgentStore::new(None);
        store
            .create_run(&run("run-1", "helsinki", "2026-09-12T10:15:30Z"))
            .await
            .expect("create");

        let started = store
            .set_status("run-1", AgentRunStatus::Starting, None)
            .await
            .expect("queued moves to starting");
        assert_eq!(started.status, "starting");
        assert!(
            started.started_at.is_some(),
            "the first state after queued stamps started_at"
        );
        assert!(started.finished_at.is_none());

        let err = store
            .set_status("run-1", AgentRunStatus::Published, None)
            .await
            .expect_err("starting does not jump to published");
        assert!(matches!(err, StatusChangeError::Refused { .. }));

        let cancelled = store
            .set_status("run-1", AgentRunStatus::Cancelled, Some("user cancelled"))
            .await
            .expect("any live state may be cancelled");
        assert_eq!(cancelled.error.as_deref(), Some("user cancelled"));
        assert!(
            cancelled.finished_at.is_some(),
            "a terminal state stamps finished_at"
        );

        for next in [AgentRunStatus::Building, AgentRunStatus::Cancelled] {
            let err = store
                .set_status("run-1", next, None)
                .await
                .expect_err("a cancelled run is over, a second cancel included");
            assert!(matches!(err, StatusChangeError::Refused { .. }));
        }
    }

    #[tokio::test]
    async fn cancelling_leaves_a_ticket_hash_nothing_can_match() {
        let store = AgentStore::new(None);
        store
            .create_run(&run("run-1", "helsinki", "2026-09-12T10:15:30Z"))
            .await
            .expect("create");

        store.invalidate_ticket("run-1").await.expect("invalidate");
        let after = store.get_run("run-1").await.expect("get").expect("run");
        assert!(
            after.ticket_hash.is_empty(),
            "an empty hash is not a PHC string, so no ticket ever verifies against it"
        );
    }

    #[tokio::test]
    async fn the_stream_numbers_events_and_replays_only_what_was_missed() {
        let store = AgentStore::new(None);
        let first = store
            .append_event("run-1", "status", serde_json::json!({"status": "starting"}))
            .await
            .expect("append");
        let second = store
            .append_event("run-1", "thought", serde_json::json!({"text": "reading"}))
            .await
            .expect("append");
        assert_eq!((first.seq, second.seq), (1, 2));

        let missed = store.events_since("run-1", 1).await.expect("since");
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].seq, 2);
        assert!(
            store
                .events_since("run-1", 2)
                .await
                .expect("since")
                .is_empty(),
            "a stream that is up to date replays nothing"
        );
        assert!(
            store
                .events_since("other-run", 0)
                .await
                .expect("since")
                .is_empty(),
            "one run's stream is not another's"
        );
    }

    #[tokio::test]
    async fn usage_accumulates_across_steps() {
        let store = AgentStore::new(None);
        store
            .create_run(&run("run-1", "helsinki", "2026-09-12T10:15:30Z"))
            .await
            .expect("create");
        store.record_usage("run-1", 4_120, 1).await.expect("usage");
        store.record_usage("run-1", 1_880, 1).await.expect("usage");
        let after = store.get_run("run-1").await.expect("get").expect("run");
        assert_eq!((after.tokens_used, after.steps), (6_000, 2));
    }
}

//! Periodic reaper for expired agent builder runs (AG-66, AP-68).
//!
//! Runs exceeding their wall-clock lease (`expires_at`) are automatically
//! transitioned to `expired`. The reaper terminates the associated Kubernetes
//! Job, invalidates the ticket in the store, and appends a status event.

use std::time::Duration;

use crate::agents::run::AgentRunStatus;
use crate::agents::store::now_rfc3339;
use crate::state::AppState;

/// Reaps runs whose `expires_at` has passed and are still in a non-terminal state.
/// Returns the number of runs transitioned to expired.
pub async fn reap_expired(state: &AppState) -> usize {
    let now = now_rfc3339();
    let expired = match state.agents.list_expired(&now).await {
        Ok(runs) => runs,
        Err(err) => {
            tracing::error!(error = %err, "reaper: failed to list expired runs");
            return 0;
        }
    };

    let mut reaped = 0;
    for run in expired {
        match crate::api::agent_runs::end_run(
            state,
            &run,
            AgentRunStatus::Expired,
            "lease expired unattended",
        )
        .await
        {
            Ok(_) => {
                reaped += 1;
                tracing::info!(run_id = %run.id, "reaped expired agent run");
            }
            Err(err) => {
                tracing::warn!(run_id = %run.id, error = %err, "failed to reap expired agent run");
            }
        }
    }
    reaped
}

/// Spawns the periodic background reaper loop (runs every 30 seconds, skips missed ticks).
pub fn spawn_periodic(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            reap_expired(&state).await;
        }
    });
}

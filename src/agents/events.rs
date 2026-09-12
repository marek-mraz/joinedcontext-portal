//! Server-Sent Events broadcasting and stream subscriptions (AG-45).

use crate::agents::run::AgentRunEvent;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

#[derive(Clone, Default)]
pub struct AgentEventHub {
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<AgentRunEvent>>>>,
}

impl AgentEventHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn broadcast(&self, event: &AgentRunEvent) {
        let mut map = self.channels.write().await;
        let sender = map.entry(event.run_id.clone()).or_insert_with(|| {
            let (tx, _) = broadcast::channel(128);
            tx
        });
        let _ = sender.send(event.clone());
    }

    pub async fn subscribe(&self, run_id: &str) -> broadcast::Receiver<AgentRunEvent> {
        let mut map = self.channels.write().await;
        let sender = map.entry(run_id.to_string()).or_insert_with(|| {
            let (tx, _) = broadcast::channel(128);
            tx
        });
        sender.subscribe()
    }
}

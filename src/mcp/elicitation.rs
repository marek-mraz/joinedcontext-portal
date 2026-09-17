//! The question a Yellow or Red MCP call asks before it runs (AG-63, T-0836).
//!
//! An agent may propose and may ask to remove, but it may not decide alone: a call whose
//! operation takes the Yellow or the Red lane, or carries `destructiveHint`, answers an
//! elicitation first. The host shows it to the person, the person opens the Portal and decides,
//! and the client repeats the call carrying the answer. Only then does the operation run, and
//! the answer is written to the project's activity, so what a person allowed is on the record
//! (AG-56).
//!
//! The route is one stateless POST, so the elicitation cannot be a server→client request on an
//! open stream: it is the answer to the first call and an argument of the second. What that
//! costs is one extra round trip; what it buys is that the words the person saw and the call
//! that runs are the same call, bound by id, subject and arguments.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::auth::session::now_unix;

/// How long a person has to answer before the id is no longer accepted.
pub const EXPIRES_IN_SECONDS: i64 = 600;

/// One question waiting for its answer.
struct Pending {
    /// The subject of the token that asked: nobody else may answer it.
    owner: String,
    operation: String,
    project: String,
    /// The arguments the question was asked about: a different call needs a new answer.
    digest: String,
    asked_at: i64,
}

/// What the caller sent back with the second call.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    /// The person allowed it: the operation runs.
    Accepted,
    /// The person refused, or the host could not ask: nothing runs.
    Declined,
    /// No such question for this caller, or it was answered, expired, or asked about something
    /// else. Nothing runs, and the caller asks again.
    Unknown,
}

/// Every question this replica is waiting on.
///
/// ponytail: per-replica map, like the Portal's other in-process state; a question lives ten
/// minutes, so a restart costs one repeated ask, never a lost decision.
#[derive(Clone, Default)]
pub struct McpElicitations {
    pending: Arc<Mutex<HashMap<String, Pending>>>,
}

impl std::fmt::Debug for McpElicitations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpElicitations")
    }
}

impl McpElicitations {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a question and returns its id.
    pub fn ask(&self, owner: &str, operation: &str, project: &str, digest: &str) -> String {
        self.forget_expired();
        let id = format!(
            "eli-{}",
            digest_short(&format!("{owner}{operation}{}", now_nanos()))
        );
        if let Ok(mut map) = self.pending.lock() {
            map.insert(
                id.clone(),
                Pending {
                    owner: owner.to_owned(),
                    operation: operation.to_owned(),
                    project: project.to_owned(),
                    digest: digest.to_owned(),
                    asked_at: now_unix(),
                },
            );
        }
        id
    }

    /// Reads the answer the client sent, and spends the question: an id answers once.
    pub fn answer(
        &self,
        owner: &str,
        operation: &str,
        project: &str,
        digest: &str,
        sent: &Value,
    ) -> Answer {
        let Some(id) = sent
            .get("elicitationId")
            .or_else(|| sent.get("id"))
            .and_then(Value::as_str)
        else {
            return Answer::Unknown;
        };
        let Ok(mut map) = self.pending.lock() else {
            return Answer::Unknown;
        };
        let Some(pending) = map.get(id) else {
            return Answer::Unknown;
        };
        // The question, the caller and the call all have to be the one that was asked about.
        if pending.owner != owner
            || pending.operation != operation
            || pending.project != project
            || pending.digest != digest
            || now_unix() - pending.asked_at > EXPIRES_IN_SECONDS
        {
            return Answer::Unknown;
        }
        map.remove(id);
        match sent
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("accept")
        {
            "accept" | "accepted" => Answer::Accepted,
            _ => Answer::Declined,
        }
    }

    fn forget_expired(&self) {
        let now = now_unix();
        if let Ok(mut map) = self.pending.lock() {
            map.retain(|_, pending| now - pending.asked_at <= EXPIRES_IN_SECONDS);
        }
    }
}

/// The question as the client receives it: what would happen, and where the person decides.
///
/// `refusal` is the gate's own answer when one already holds — a proposal whose check has not
/// run, for instance. It is carried beside the question rather than instead of it: the three
/// fields the REST route answers (`error`, `check`, `reason`) sit at the top of
/// `structuredContent`, so a client that reads `verdict_required` on the route reads it here
/// too, and the URL the person needs is still there (PF-57, ADR-N-021, T-0947).
pub fn document(id: &str, message: &str, url: &str, refusal: Option<Value>) -> Value {
    let mut structured = serde_json::Map::new();
    if let Some(Value::Object(fields)) = refusal {
        structured.extend(fields);
    }
    structured.insert(
        "elicitation".to_owned(),
        json!({
            "elicitationId": id,
            "mode": "url",
            "url": url,
            "message": message,
            "expiresIn": EXPIRES_IN_SECONDS,
        }),
    );
    json!({
        "isError": false,
        "status": "input_required",
        "content": [{ "type": "text", "text": message }],
        "structuredContent": Value::Object(structured),
    })
}

/// A stable short digest of the arguments a question was asked about.
pub fn digest_of(value: &Value) -> String {
    digest_short(&serde_json::to_string(&canonical(value)).unwrap_or_default())
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: std::collections::BTreeMap<_, _> =
                map.iter().map(|(k, v)| (k.clone(), canonical(v))).collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn digest_short(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))[..16].to_owned()
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments() -> Value {
        json!({ "kind": "Endpoint", "name": "helsinki-bikes-ops", "confirm": "helsinki-bikes-ops" })
    }

    #[test]
    fn the_answer_belongs_to_the_caller_the_operation_and_the_arguments_asked_about() {
        let asks = McpElicitations::new();
        let digest = digest_of(&arguments());
        let id = asks.ask("sub-1", "jc_resource_delete", "helsinki", &digest);
        let sent = json!({ "elicitationId": id, "action": "accept" });

        // Another subject, another operation, another project, other arguments: no answer.
        assert_eq!(
            asks.answer("sub-2", "jc_resource_delete", "helsinki", &digest, &sent),
            Answer::Unknown
        );
        assert_eq!(
            asks.answer("sub-1", "jc_resource_propose", "helsinki", &digest, &sent),
            Answer::Unknown
        );
        assert_eq!(
            asks.answer("sub-1", "jc_resource_delete", "espoo", &digest, &sent),
            Answer::Unknown
        );
        assert_eq!(
            asks.answer(
                "sub-1",
                "jc_resource_delete",
                "helsinki",
                &digest_of(&json!({ "kind": "Endpoint", "name": "something-else" })),
                &sent
            ),
            Answer::Unknown
        );

        // The one it was asked of answers once, and the id is spent.
        assert_eq!(
            asks.answer("sub-1", "jc_resource_delete", "helsinki", &digest, &sent),
            Answer::Accepted
        );
        assert_eq!(
            asks.answer("sub-1", "jc_resource_delete", "helsinki", &digest, &sent),
            Answer::Unknown
        );
    }

    #[test]
    fn a_refusal_is_a_refusal_and_a_missing_id_answers_nothing() {
        let asks = McpElicitations::new();
        let digest = digest_of(&arguments());
        let id = asks.ask("sub-1", "jc_change_approve", "helsinki", &digest);
        assert_eq!(
            asks.answer(
                "sub-1",
                "jc_change_approve",
                "helsinki",
                &digest,
                &json!({ "elicitationId": id, "action": "decline" })
            ),
            Answer::Declined
        );
        assert_eq!(
            asks.answer(
                "sub-1",
                "jc_change_approve",
                "helsinki",
                &digest,
                &json!({ "action": "accept" })
            ),
            Answer::Unknown
        );
    }

    #[test]
    fn the_same_arguments_digest_the_same_however_they_were_written() {
        assert_eq!(
            digest_of(&json!({ "a": 1, "b": { "c": 2, "d": 3 } })),
            digest_of(&json!({ "b": { "d": 3, "c": 2 }, "a": 1 }))
        );
    }
}

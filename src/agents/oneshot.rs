//! The kit pass: a static application from one model call (Architecture/19 §1.2, AP-56…AP-60,
//! AG-53, AG-54).
//!
//! A `static` run has no workspace. The Portal itself reads a few entities per type through
//! the proxy, hands the model the prompt, the samples and the kit's schema, and applies the
//! SEARCH/REPLACE blocks the answer carries to one file, `spec.json`. A valid specification
//! becomes the preview document a minute after the run was created; every chat message is one
//! more pass over the same file. The proxy is still the one holding the model key and the
//! endpoint: the driver speaks to it with the run's ticket, like a workspace would.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::agents::kit;
use crate::agents::patch;
use crate::agents::profile::Profile;
use crate::agents::run::{AgentRun, AgentRunEvent, AgentRunStatus};
use crate::agents::store::now_rfc3339;
use crate::state::AppState;

/// Entities read per type as the model's sample of the data (AP-57).
const SAMPLES_PER_TYPE: u32 = 5;
/// Output tokens one pass may spend: a specification is a few hundred, a repair fewer.
const OUTPUT_BUDGET: u32 = 6000;
/// One model call, wall clock.
const CALL_TIMEOUT: Duration = Duration::from_secs(180);
/// The name the driver signs its own chat lines with; a message by anyone else is a pass.
pub const AGENT: &str = "agent";

static SYSTEM: LazyLock<String> = LazyLock::new(|| {
    format!(
        r#"# SYSTEM INSTRUCTION: ONE-SHOT DASHBOARD SPECIFICATION — SEARCH/REPLACE FORMAT

You fill in one file, `spec.json`, for a prebuilt dashboard kit. The kit already holds the code:
a data loader for NGSI-LD entities, filters, a stats row, a map, a table, SVG charts and a
detail card. You write only the specification that says which entity types to read, which
attributes, and which views to draw. THIS CALL WRITES `spec.json` AND NOTHING ELSE: a block for
any other path is refused.

You answer ONCE per call; a script applies your answer mechanically. There is no tool, no
follow-up question, no second file.

## THE SPECIFICATION

`spec.json` must satisfy this JSON Schema exactly (no unknown fields):

```json
{schema}
```

Rules the schema cannot say, checked before anything is shown:
- `sources[].name` is unique; every `source` in a filter or view names one of them; a filter or
  view without `source` reads the first source.
- Every attribute a filter or view names (`attrs`, `attr`, `columns`, `x`, `y`, `sort.attr`,
  `location`, `label`, `color`) is in that source's `attrs`, or is `id` / `type`.
- `stats.items[].attr` is required unless `agg` is `count`.
- A map needs a GeoProperty; name it in `attrs` (usually `location`).
- `limit` is between 1 and 5000; default 1000.

## WHAT MAKES A GOOD DASHBOARD

- Read the samples: use attribute names exactly as they appear there, never invented ones.
- Start with `stats` (count, and one or two averages or sums that matter), then a `map` when
  the entities have a location, then a `table` with the attributes a person would scan, then a
  `chart` or two for the distributions that answer the prompt, then `detail`.
- Filters: a `search` over the name-like attributes, a `select` over a categorical attribute,
  a `range` over the number the prompt cares about.
- Titles in the language of the prompt; short.
- Numbers only in `range`, `sum`, `avg`, `min`, `max`, `chart.y` (unless `agg` is `count`).

## THE FORMAT RULES

1. Write the file path on its own line before each `<<<<<<< SEARCH` block.
2. To CREATE or REWRITE the whole file, leave the SEARCH block completely empty. For a first
   pass, and for any change that touches more than a few lines, rewrite the whole file: it is
   short, and a whole-file rewrite is never ambiguous.
3. For a small edit, SEARCH holds at least 3 consecutive lines copied exactly from the current
   file, unique in the file; REPLACE holds the new lines only.
4. Put the whole output in a single markdown code block. Before the code block, write one or
   two plain sentences for the person reading the chat: what the dashboard shows and what you
   changed. After the block, nothing.
5. If something the prompt asks for cannot be drawn with these views, say so in the sentences
   before the block and build what can be.

## THE SYNTAX

```text
spec.json
<<<<<<< SEARCH
=======
{{
  "title": "…",
  "sources": [ … ],
  "filters": [ … ],
  "views": [ … ]
}}
>>>>>>> REPLACE
```
"#,
        schema = kit::schema_json()
    )
});

/// What one run needs to be driven; everything is copied out of the run and the settings so the
/// task owns what it reads.
struct Driver {
    state: AppState,
    http: reqwest::Client,
    run_id: String,
    project: String,
    prompt: String,
    data_needs: Value,
    bearer: String,
    proxy_base: String,
    model: String,
    provider: String,
    ttl: Duration,
    /// Passes that produced a preview; the `v` of the preview URL.
    passes: AtomicU32,
}

/// Starts the pass in the background. Returns at once; the run's stream is where the outcome
/// goes (AP-60).
pub fn spawn(
    state: AppState,
    run: &AgentRun,
    ticket: &str,
    profile: &Profile,
    proxy_base: &str,
    ttl_secs: i64,
) {
    let http = reqwest::Client::builder()
        .timeout(CALL_TIMEOUT)
        .build()
        .unwrap_or_default();
    let driver = Driver {
        state,
        http,
        run_id: run.id.clone(),
        project: run.project.clone(),
        prompt: run.prompt.clone(),
        data_needs: run.data_needs.clone(),
        bearer: format!("jcr_{}.{ticket}", run.id),
        proxy_base: proxy_base.trim_end_matches('/').to_owned(),
        model: profile.model_name.clone(),
        provider: profile.model_provider.clone(),
        ttl: Duration::from_secs(ttl_secs.max(1) as u64),
        passes: AtomicU32::new(0),
    };
    tokio::spawn(async move {
        let run_id = driver.run_id.clone();
        if let Err(message) = driver.drive().await {
            tracing::warn!(run_id = %run_id, %message, "the kit pass failed");
            driver.fail(&message).await;
        }
    });
}

impl Driver {
    async fn drive(&self) -> Result<(), String> {
        // The stream is subscribed before anything moves, so a message sent while the first
        // pass runs is not lost between the preview and the loop.
        let mut inbox = self.state.agent_events.subscribe(&self.run_id).await;
        let deadline = tokio::time::Instant::now() + self.ttl;

        self.status(AgentRunStatus::Starting).await?;
        let types = self.types();
        self.thought(&format!(
            "Reading {SAMPLES_PER_TYPE} entities of {} through the endpoint.",
            types.join(", ")
        ))
        .await?;
        let samples = self.samples(&types).await;
        self.status(AgentRunStatus::Building).await?;

        let mut files: BTreeMap<String, String> = BTreeMap::new();
        let mut conversation: Vec<(String, String)> = Vec::new();
        let outcome = self
            .pass(&samples, &mut files, &conversation, &self.prompt)
            .await?;
        let Some(prose) = outcome else {
            return Err("the first pass produced no specification the kit can render".to_owned());
        };
        conversation.push((self.prompt.clone(), prose));
        // The lifecycle has no edge from building to previewing; the validation of the
        // specification is the run's test, and it says so.
        self.status(AgentRunStatus::Testing).await?;
        self.status(AgentRunStatus::Previewing).await?;

        // Every message is one more pass; the run stays `previewing` throughout (AP-60).
        loop {
            let event = tokio::select! {
                event = inbox.recv() => event,
                () = tokio::time::sleep_until(deadline) => {
                    self.expire().await;
                    return Ok(());
                }
            };
            let event = match event {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            };
            match event.kind.as_str() {
                "status" if is_terminal(&event) => return Ok(()),
                "message" if sent_by_person(&event) => {
                    let text = event
                        .payload
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    match self.pass(&samples, &mut files, &conversation, &text).await {
                        Ok(Some(prose)) => conversation.push((text, prose)),
                        // A pass that failed keeps the last preview; the chat says why.
                        Ok(None) => {}
                        Err(message) => {
                            self.thought(&format!("The pass failed: {message}")).await?
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// One model call, its blocks applied, the specification checked, the preview refreshed.
    /// `Ok(Some(prose))` is a pass the person can see; `Ok(None)` is one the chat explained
    /// and the preview ignored; `Err` is the proxy or the store not answering.
    async fn pass(
        &self,
        samples: &Value,
        files: &mut BTreeMap<String, String>,
        conversation: &[(String, String)],
        instruction: &str,
    ) -> Result<Option<String>, String> {
        let user = self.pack(samples, files, conversation, instruction, None);
        let answer = self.complete(&user).await?;
        let (mut prose, mut errors) = self.apply(files, &answer).await?;
        if !errors.is_empty() {
            // One repair call: the model is shown what did not validate and answers again.
            self.thought(&format!(
                "The specification does not validate; asking for a repair:\n{}",
                errors.join("\n")
            ))
            .await?;
            let user = self.pack(samples, files, conversation, instruction, Some(&errors));
            let answer = self.complete(&user).await?;
            (prose, errors) = self.apply(files, &answer).await?;
        }
        if !errors.is_empty() {
            self.thought(&format!(
                "Still not a specification the kit can render:\n{}",
                errors.join("\n")
            ))
            .await?;
            return Ok(None);
        }
        // The rows travel inside the preview document, because the sandboxed frame has no
        // session to read them with; they are read here, once per pass, through the proxy.
        // ponytail: a snapshot per pass. Live rows in the preview need a data route with a
        // preview-scoped bearer and CORS for a null origin; the published app reads live.
        let spec = files
            .get(kit::SPEC_FILE)
            .and_then(|text| kit::parse(text).ok())
            .ok_or_else(|| "spec.json vanished between validation and storage".to_owned())?;
        let data = self.rows(&spec).await;
        files.insert(kit::DATA_FILE.to_owned(), data.to_string());
        let stored: serde_json::Map<String, Value> = files
            .iter()
            .map(|(path, content)| (path.clone(), Value::String(content.clone())))
            .collect();
        self.state
            .agents
            .set_files(&self.run_id, Value::Object(stored))
            .await
            .map_err(|err| err.to_string())?;
        let prose = if prose.trim().is_empty() {
            "The dashboard is ready.".to_owned()
        } else {
            prose.trim().to_owned()
        };
        self.thought(&prose).await?;
        // The URL carries the number of the pass, so a browser reloads the frame once per pass
        // and never from cache.
        let pass = self.passes.fetch_add(1, Ordering::SeqCst) + 1;
        let url = format!(
            "/api/v1/projects/{}/agent-runs/{}/preview?v={pass}",
            self.project, self.run_id
        );
        self.state
            .agents
            .set_preview_url(&self.run_id, &url)
            .await
            .map_err(|err| err.to_string())?;
        self.event("preview", json!({ "previewUrl": url })).await?;
        Ok(Some(prose))
    }

    /// Applies an answer to the files and says what the kit thinks of `spec.json`.
    async fn apply(
        &self,
        files: &mut BTreeMap<String, String>,
        answer: &str,
    ) -> Result<(String, Vec<String>), String> {
        let (blocks, prose) = patch::parse(answer);
        let (applied, refused) = patch::apply(files, &blocks, &[kit::SPEC_FILE]);
        self.event(
            "tool",
            json!({
                "tool": "apply_patch",
                "command": format!("{} block(s) for spec.json", blocks.len()),
                "exitCode": if refused.is_empty() { 0 } else { 1 },
                "applied": applied,
                "refused": refused,
            }),
        )
        .await?;
        // A refused block is on the log; it asks for a repair only when the specification
        // itself is missing or wrong, so a stray extra file never costs a second call (AP-58).
        let mut errors: Vec<String> = Vec::new();
        match files.get(kit::SPEC_FILE) {
            None if blocks.is_empty() => errors.push(
                "the answer carried no SEARCH/REPLACE block; write spec.json as one block with an empty SEARCH".to_owned(),
            ),
            None => errors.push("spec.json was not written".to_owned()),
            Some(text) => {
                if let Err(problems) = kit::parse(text) {
                    errors.extend(problems);
                }
            }
        }
        if !errors.is_empty() {
            errors.extend(
                refused
                    .iter()
                    .map(|refused| format!("{}: {}", refused.path, refused.reason)),
            );
        }
        Ok((prose, errors))
    }

    /// The user message of one call: everything the model needs beyond the fixed system
    /// prompt, current file first so a small edit can copy its lines.
    fn pack(
        &self,
        samples: &Value,
        files: &BTreeMap<String, String>,
        conversation: &[(String, String)],
        instruction: &str,
        errors: Option<&[String]>,
    ) -> String {
        let mut pack = String::new();
        pack.push_str("## THE APPLICATION\n\n");
        pack.push_str(&self.prompt);
        pack.push_str("\n\n## THE DATA THE ENDPOINT PUBLISHES\n\n");
        pack.push_str("Data needs (types and attributes the person asked for):\n```json\n");
        pack.push_str(&serde_json::to_string_pretty(&self.data_needs).unwrap_or_default());
        pack.push_str("\n```\n\nSample entities per type (`options=keyValues`):\n```json\n");
        pack.push_str(&serde_json::to_string_pretty(samples).unwrap_or_default());
        pack.push_str("\n```\n\n## THE CURRENT FILES\n\n");
        if files.is_empty() {
            pack.push_str("(none yet: write spec.json with an empty SEARCH block)\n");
        }
        for (path, content) in files {
            pack.push_str(&format!("### {path}\n```json\n{content}\n```\n"));
        }
        if !conversation.is_empty() {
            pack.push_str("\n## THE CONVERSATION SO FAR\n\n");
            for (asked, answered) in conversation {
                pack.push_str(&format!("Person: {asked}\nYou: {answered}\n\n"));
            }
        }
        pack.push_str("\n## THIS CALL\n\n");
        match errors {
            Some(errors) => {
                pack.push_str(
                    "The last answer was applied and the specification does not validate. \
                     Fix every problem below and answer with the corrected spec.json:\n",
                );
                for error in errors {
                    pack.push_str(&format!("- {error}\n"));
                }
                pack.push_str(&format!(
                    "\nThe instruction being fulfilled: {instruction}\n"
                ));
            }
            None if files.is_empty() => {
                pack.push_str("Write spec.json for the application above.\n");
            }
            None => {
                pack.push_str(&format!(
                    "The person says: {instruction}\n\nChange spec.json accordingly.\n"
                ));
            }
        }
        pack
    }

    /// One call through the proxy, in the body the profile's provider reads (AG-53).
    async fn complete(&self, user: &str) -> Result<String, String> {
        let (path, body) = if self.provider == "anthropic" {
            (
                "/v1/llm/messages",
                json!({
                    "model": self.model,
                    "max_tokens": OUTPUT_BUDGET,
                    "system": *SYSTEM,
                    "messages": [{ "role": "user", "content": user }],
                }),
            )
        } else {
            (
                "/v1/llm/chat/completions",
                json!({
                    "model": self.model,
                    "max_tokens": OUTPUT_BUDGET,
                    "messages": [
                        { "role": "system", "content": *SYSTEM },
                        { "role": "user", "content": user },
                    ],
                }),
            )
        };
        let response = self
            .http
            .post(format!("{}{path}", self.proxy_base))
            .bearer_auth(&self.bearer)
            .json(&body)
            .send()
            .await
            .map_err(|err| format!("the model call did not go through the proxy: {err}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "the proxy answered {status} to the model call: {}",
                text.chars().take(300).collect::<String>()
            ));
        }
        let answer: Value = serde_json::from_str(&text)
            .map_err(|err| format!("the model's answer is not JSON: {err}"))?;
        // OpenAI-compatible: choices[0].message.content. Anthropic: content[].text, joined.
        if let Some(content) = answer
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
        {
            return Ok(content.to_owned());
        }
        let joined: String = answer
            .get("content")
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        if joined.is_empty() {
            return Err("the model's answer carried no text".to_owned());
        }
        Ok(joined)
    }

    /// The entity types the data needs name, in order, once each.
    fn types(&self) -> Vec<String> {
        let mut types: Vec<String> = Vec::new();
        for need in self.data_needs.as_array().into_iter().flatten() {
            for entity_type in need
                .get("types")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !types.iter().any(|known| known == entity_type) {
                    types.push(entity_type.to_owned());
                }
            }
        }
        types
    }

    /// Every source's rows, read through the proxy in pages up to the source's limit. A source
    /// that cannot be read is an empty list and a line in the chat: the dashboard still shows.
    async fn rows(&self, spec: &kit::Spec) -> Value {
        let mut data = serde_json::Map::new();
        for source in &spec.sources {
            let limit = source
                .limit
                .unwrap_or(kit::DEFAULT_LIMIT)
                .min(kit::MAX_LIMIT);
            let mut rows: Vec<Value> = Vec::new();
            let mut failure = None;
            while (rows.len() as u32) < limit {
                let page = kit::PAGE.min(limit - rows.len() as u32);
                let mut url = format!(
                    "{}/v1/data/ngsi-ld/v1/entities?type={}&options=keyValues&limit={page}&offset={}",
                    self.proxy_base,
                    urlencoding(&source.entity_type),
                    rows.len()
                );
                if !source.attrs.is_empty() {
                    url.push_str(&format!("&attrs={}", urlencoding(&source.attrs.join(","))));
                }
                if let Some(q) = &source.q {
                    url.push_str(&format!("&q={}", urlencoding(q)));
                }
                match self.read_entities(&url).await {
                    Ok(Value::Array(entities)) => {
                        let got = entities.len() as u32;
                        rows.extend(entities);
                        if got < page {
                            break;
                        }
                    }
                    Ok(_) => {
                        failure = Some("the endpoint did not answer a list".to_owned());
                        break;
                    }
                    Err(reason) => {
                        failure = Some(reason);
                        break;
                    }
                }
            }
            if let Some(reason) = failure {
                let _ = self
                    .thought(&format!(
                        "The rows of {} could not be read: {reason}",
                        source.name
                    ))
                    .await;
            }
            data.insert(source.name.clone(), Value::Array(rows));
        }
        Value::Object(data)
    }

    /// One GET through the proxy with the run's ticket, as JSON.
    async fn read_entities(&self, url: &str) -> Result<Value, String> {
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.bearer)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|err| err.to_string())?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "{status}: {}",
                body.chars().take(200).collect::<String>()
            ));
        }
        serde_json::from_str::<Value>(&body).map_err(|err| err.to_string())
    }

    /// A few entities per type, read through the proxy like the application will (AP-57). A
    /// type that cannot be read is an empty list with the reason beside it: the model still
    /// gets the data needs, and the chat says what was missing.
    async fn samples(&self, types: &[String]) -> Value {
        let mut samples = serde_json::Map::new();
        for entity_type in types {
            let url = format!(
                "{}/v1/data/ngsi-ld/v1/entities?type={}&limit={SAMPLES_PER_TYPE}&options=keyValues",
                self.proxy_base,
                urlencoding(entity_type)
            );
            match self.read_entities(&url).await {
                Ok(entities) => {
                    samples.insert(entity_type.clone(), entities);
                }
                Err(reason) => {
                    let _ = self
                        .thought(&format!(
                            "No sample of {entity_type} could be read: {reason}"
                        ))
                        .await;
                    samples.insert(entity_type.clone(), json!([]));
                }
            }
        }
        Value::Object(samples)
    }

    async fn status(&self, status: AgentRunStatus) -> Result<(), String> {
        self.state
            .agents
            .set_status(&self.run_id, status, None)
            .await
            .map_err(|err| err.to_string())?;
        self.event(
            "status",
            json!({ "status": status.as_str(), "timestamp": now_rfc3339() }),
        )
        .await
    }

    async fn thought(&self, text: &str) -> Result<(), String> {
        self.event("thought", json!({ "text": text })).await
    }

    async fn event(&self, kind: &str, payload: Value) -> Result<(), String> {
        let event = self
            .state
            .agents
            .append_event(&self.run_id, kind, payload)
            .await
            .map_err(|err| err.to_string())?;
        self.state.agent_events.broadcast(&event).await;
        Ok(())
    }

    async fn fail(&self, message: &str) {
        let _ = self
            .state
            .agents
            .set_status(&self.run_id, AgentRunStatus::Failed, Some(message))
            .await;
        let _ = self
            .event(
                "status",
                json!({ "status": AgentRunStatus::Failed.as_str(), "error": message, "timestamp": now_rfc3339() }),
            )
            .await;
    }

    async fn expire(&self) {
        let message = "the run's wall clock ran out";
        let _ = self
            .state
            .agents
            .set_status(&self.run_id, AgentRunStatus::Expired, Some(message))
            .await;
        let _ = self
            .event(
                "status",
                json!({ "status": AgentRunStatus::Expired.as_str(), "error": message, "timestamp": now_rfc3339() }),
            )
            .await;
    }
}

fn is_terminal(event: &AgentRunEvent) -> bool {
    event
        .payload
        .get("status")
        .and_then(Value::as_str)
        .and_then(AgentRunStatus::parse)
        .is_some_and(|status| status.is_terminal())
}

fn sent_by_person(event: &AgentRunEvent) -> bool {
    event
        .payload
        .get("sentBy")
        .and_then(Value::as_str)
        .is_some_and(|by| by != AGENT)
}

/// Percent-encodes one query value; entity types are URIs or short names, both fit.
fn urlencoding(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_prompt_carries_the_schema_and_the_one_allowed_path() {
        assert!(SYSTEM.contains("\"title\""), "the schema is inlined");
        assert!(SYSTEM.contains("spec.json\n<<<<<<< SEARCH"));
        assert!(!SYSTEM.contains("{schema}"));
    }

    #[test]
    fn types_are_listed_once_in_order_and_urls_are_encoded() {
        let driver_types = |needs: Value| {
            let mut types = Vec::new();
            for need in needs.as_array().into_iter().flatten() {
                for t in need["types"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    if !types.iter().any(|k: &String| k == t) {
                        types.push(t.to_owned());
                    }
                }
            }
            types
        };
        assert_eq!(
            driver_types(json!([{ "types": ["A", "B"] }, { "types": ["B", "C"] }])),
            vec!["A", "B", "C"]
        );
        assert_eq!(
            urlencoding("https://uri.fiware.org/ns/data-models#Bike Station"),
            "https%3A%2F%2Furi.fiware.org%2Fns%2Fdata-models%23Bike%20Station"
        );
    }

    #[test]
    fn a_message_by_the_agent_itself_starts_no_pass() {
        let event = |by: &str| AgentRunEvent {
            run_id: "r".into(),
            seq: 1,
            kind: "message".into(),
            payload: json!({ "text": "x", "sentBy": by }),
            created_at: String::new(),
        };
        assert!(sent_by_person(&event("demo.steward")));
        assert!(!sent_by_person(&event(AGENT)));
        let status = AgentRunEvent {
            kind: "status".into(),
            payload: json!({ "status": "failed" }),
            ..event("x")
        };
        assert!(is_terminal(&status));
    }
}

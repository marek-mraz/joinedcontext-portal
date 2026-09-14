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
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::agents::access::Access;
use crate::agents::code;
use crate::agents::endpoints;
use crate::agents::kit;
use crate::agents::patch;
use crate::agents::preview;
use crate::agents::profile::Profile;
use crate::agents::run::{AgentRun, AgentRunEvent, AgentRunStatus};
use crate::agents::store::now_rfc3339;
use crate::agents::{data_query, fields, kpi, kpi_pipeline, share, verification};
use crate::auth::session::Identity;
use crate::git::gitea::{Author, FileWrite, GitError};
use crate::state::AppState;

/// Entities read per type as the model's sample of the data (AP-57).
const SAMPLES_PER_TYPE: u32 = 5;
/// Output tokens one pass may spend: a specification is a few hundred, a page of the model's
/// own (the escape hatch) ten thousand and more. A cut answer is refused whole, below.
const OUTPUT_BUDGET: u32 = 24000;
/// Output tokens one call of a code run may spend: a whole application with its tests is tens
/// of thousands, and a cut answer is refused whole all the same.
const CODE_OUTPUT_BUDGET: u32 = 64000;

/// What the conversation records as the instruction of the completing call.
const COMPLETE_TURN: &str = "Complete the application.";

/// The output ceiling of a code run's first version (SDK-13): the answer is asked to stay near
/// 6,000 tokens, which a model writes in about 35 seconds; the ceiling leaves room for its
/// reasoning so a slightly longer answer is not cut.
const FIRST_VERSION_BUDGET: u32 = 20000;
/// One model call, wall clock: the budget above at a hundred tokens a second, with room.
const CALL_TIMEOUT: Duration = Duration::from_secs(480);
/// The name the driver signs its own chat lines with; a message by anyone else is a pass.
pub const AGENT: &str = "agent";
/// The smallest output budget worth a second call when the key's credit runs short.
const MIN_CREDIT_BUDGET: u32 = 4000;

/// Why one model call gave no answer.
#[derive(Debug)]
enum CallError {
    /// The provider answered with no text.
    Empty,
    /// The key's credit does not cover the output budget; the provider's own figure when it
    /// gave one.
    Credit {
        affordable: Option<u32>,
    },
    Failed(String),
}

impl CallError {
    /// What the chat says. Never the provider's body: it carries links to the key's settings.
    fn said(self, budget: u32) -> String {
        match self {
            Self::Empty => "the model's answer carried no text".to_owned(),
            Self::Credit {
                affordable: Some(afford),
            } => format!(
                "the model provider's credit for this Portal has run out (it covers about \
                 {afford} more output tokens, this step asks for up to {budget}); an \
                 administrator tops up the key, then send the message again"
            ),
            Self::Credit { affordable: None } => "the model provider's credit for this Portal \
                 has run out; an administrator tops up the key, then send the message again"
                .to_owned(),
            Self::Failed(reason) => reason,
        }
    }
}

/// The output tokens a 402 says the key can still pay for: OpenRouter writes "can only afford
/// 58556".
fn affordable_tokens(body: &str) -> Option<u32> {
    let said = provider_said(body);
    let (_, rest) = said.split_once("can only afford ")?;
    rest.split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

static LINK: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"https?://\S+").expect("valid regex"));

/// The provider's error message, without links, at most 300 characters.
fn provider_said(body: &str) -> String {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.to_owned());
    LINK.replace_all(&message, "(link removed)")
        .chars()
        .take(300)
        .collect()
}

/// Kit capabilities JSON loaded directly from sdk/kit.json (AP-65).
pub static KIT_CAPABILITIES: &str = include_str!("../../sdk/kit.json");

/// The system prompt of a conversation turn: prose or one tool call, never a file (AG-67).
const CONVERSATION_SYSTEM: &str =
    "You are the joinedcontext Portal assistant. Answer the person in \
     plain prose, or with exactly one tool call when the user message describes it. Never write \
     files or SEARCH/REPLACE blocks.";

static SYSTEM: LazyLock<String> = LazyLock::new(|| {
    format!(
        r#"# SYSTEM INSTRUCTION: ONE-SHOT DASHBOARD SPECIFICATION — SEARCH/REPLACE FORMAT

You fill in one file, `spec.json`, for a prebuilt dashboard kit. The kit already holds the code:
a data loader for NGSI-LD entities, filters, a stats row, a map, a table, SVG charts, a
detail card, a form window and pages. You write only the specification that says which entity types to read, which
attributes, and which views to draw. THIS CALL WRITES `spec.json`, AND `index.html` ONLY WHEN
THE VIEWS ARE NOT ENOUGH (below): a block for any other path is refused.

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
  `location`, `label`, `color`, `fields`) is in that source's `attrs`, or is `id` / `type`.
- A `form` opens as a window when a point on the map or a row in the table is picked; its
  inputs are the `fields` (every attribute when absent), each drawn from the endpoint's field
  schema in the user message (an enum is a select, a number keeps its bounds, a pattern and a
  required mark are enforced); a save is one write through the endpoint with the person's own
  access, and a `New` button creates an entity. A form is allowed only when the user message
  says the application may write; `fields` name only attributes the field schema lists.
- `page` on a view puts it on a named tab; views without `page` stay on every tab. Use pages
  only when the person asks for several pages or screens.
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

## WHEN THE PERSON ASKS TO EDIT, UPDATE OR MANAGE ENTITIES

Pair a `table` and a `form` on the same source: the table lists the entities with the
attributes a person scans, the form's `fields` are the attributes the person is meant to
change (the note, the status, the count; never `id`, `type` or a GeoProperty), and a row
picked in the table opens in the form. Put a `stats` row above when a count matters. When the
user message says the application may NOT write, build the read-only screens, add no form,
and say in the sentences before the block that editing needs write access in the data needs.

## WHEN THE VIEWS ARE NOT ENOUGH

Something the views cannot draw (a 3D scene, a bespoke chart, an animation, a free layout, a
custom widget) is not a refusal: write `index.html` as well, a complete page, and it replaces
the kit's rendering. `spec.json` stays: its `sources` say which rows the page gets, its views
are the fallback. The contract of the page:
- The rows are there before the page's own scripts run: `window.kit = {{ slug, spec, data }}`,
  where `data[sourceName]` is the array of entities in keyValues form (`id`, `type`, the attrs;
  a GeoProperty is a GeoJSON geometry, e.g. `location.coordinates` = `[lon, lat]`).
- Libraries only from `https://cdn.jsdelivr.net`, `https://cdnjs.cloudflare.com` or
  `https://unpkg.com`, by `<script src>` / `<link>` with a pinned version. A basemap: MapLibre
  GL from the CDN with the style `https://tiles.openfreemap.org/styles/liberty`. 3D: three.js
  or deck.gl from the CDN. Nothing else on the network; no fetch to the platform.
- One file, inline CSS and JS, no build step, no modules that import from elsewhere.
- To go back to the views, rewrite `index.html` as an empty file.
Prefer the views whenever they can do it: they are faster, filtered and consistent.

## WHEN THE PERSON ASKS TO SHARE OR PUBLISH DATA

A request to share, publish, open or expose data with somebody (a team, a project, a partner,
the public) is not a dashboard change. Answer with one or two plain sentences and then ONE
fenced JSON block, nothing else, in this shape:

```json
{{
  "tool": "propose_endpoint",
  "contextSpace": "<the space the data lives in>",
  "name": "<a short lowercase dns-1123 name for the endpoint>",
  "title": "<a title in the language of the request>",
  "audience": "project-list",
  "allowedProjects": ["<the project or team named, as a lowercase dns-1123 name>"],
  "representations": ["ngsi-ld", "geojson"],
  "hiddenAttributes": ["<attributes the person wants hidden, exact names from the samples>"],
  "entityTypes": ["<the types shared, exact names from the samples>"]
}}
```

`audience` is "project-list" unless the person says the whole organization ("organization") or
everyone ("public"). The platform mints the slug, renders the manifests and opens the form;
the person submits. Write no SEARCH/REPLACE block in that answer.

## WHEN THE PERSON ASKS FOR AN INDICATOR, A KPI OR ONE NUMBER OVER THE DATA

"What is the average PM10", "how many stations are closed", "define a KPI for free bikes": the
platform computes it, not you. Answer with one or two plain sentences and then ONE fenced JSON
block, nothing else, in this shape:

```json
{{
  "tool": "compute_kpi",
  "name": "<a short lowercase name with dashes, e.g. average-pm10>",
  "title": "<a title in the language of the request>",
  "type": "<the entity type, exact name from the samples>",
  "attribute": "<the attribute folded, exact name from the samples; empty for count>",
  "agg": "avg | sum | count | min | max",
  "unit": "<a UN/CEFACT common code when the value has a unit, e.g. GQ for µg/m³, C62 for a count>",
  "q": "<an NGSI-LD filter narrowing the entities, or omit it>"
}}
```

The platform reads the entities through the endpoint, computes the value, renders the
`KeyPerformanceIndicator` entity with its formula and provenance, and shows it to the person,
who writes it into the project's indicator space themselves. Write no SEARCH/REPLACE block in
that answer.

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

/// What a drafting tool of the conversation left: the answer for the person, or what the model
/// reads back to look at the data again and try once more (AG-76).
enum Worked {
    Done(String),
    Again(String),
}

/// A draft or a read the model asked for, as the results section names it.
fn drafted(tool: &str, input: Option<Value>) -> data_query::QueryCall {
    let input = input.unwrap_or(Value::Null);
    let endpoint = ["sourceEndpoint", "endpoint"]
        .iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .unwrap_or("")
        .to_owned();
    data_query::QueryCall {
        endpoint,
        name: tool.to_owned(),
        arguments: input,
    }
}

/// What one run needs to be driven; everything is copied out of the run and the settings so the
/// task owns what it reads.
struct Driver {
    state: AppState,
    http: reqwest::Client,
    run_id: String,
    project: String,
    prompt: String,
    data_needs: Value,
    /// The endpoint the application reads and, with a write need, writes through (AP-62).
    endpoint_slug: String,
    /// Every endpoint the run reads, the primary (`endpoint_slug`) first (AP-44).
    endpoints: Vec<endpoints::RunEndpoint>,
    /// Whether the data needs carry a write operation: what makes a `form` view allowed.
    allows_write: bool,
    bearer: String,
    proxy_base: String,
    model: String,
    provider: String,
    ttl: Duration,
    /// Passes that produced a preview; the `v` of the preview URL.
    passes: AtomicU32,
    /// The endpoint's `schema/index.json`, read once before the first pass: which types it
    /// serves, so a data need naming anything else never reaches the chat or the model.
    schema_index: OnceLock<Value>,
    /// Where every pass is committed when the Portal has a forge: the run's branch, the
    /// application's folder.
    branch: String,
    path_prefix: String,
    created_by: String,
    /// The person who started the run: every tool call runs as them, never wider (AG-70).
    identity: Identity,
    /// The profile's access block, checked with `identity` before each tool call (AG-70).
    access: Access,
    kind: String,
    unattended: bool,
    continues: Option<String>,
}

/// Starts the pass in the background. Returns at once; the run's stream is where the outcome
/// goes (AP-60).
pub fn spawn(
    state: AppState,
    run: &AgentRun,
    identity: &Identity,
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
        endpoint_slug: run.endpoint_slug.clone(),
        endpoints: endpoints::of_run(run),
        allows_write: run.allows_write,
        bearer: format!("jcr_{}.{ticket}", run.id),
        proxy_base: proxy_base.trim_end_matches('/').to_owned(),
        model: profile.model_name.clone(),
        provider: profile.model_provider.clone(),
        ttl: Duration::from_secs(ttl_secs.max(1) as u64),
        passes: AtomicU32::new(0),
        schema_index: OnceLock::new(),
        branch: run.branch.clone(),
        path_prefix: run.path_prefix.clone(),
        created_by: run.created_by.clone(),
        identity: identity.clone(),
        access: profile.access.clone(),
        kind: run.kind.clone(),
        unattended: run.unattended,
        continues: run.continues.clone(),
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

        if self.kind == "conversation" {
            return self.drive_conversation(&mut inbox, deadline).await;
        }
        // Unreadable, the data needs pass as given, less the abstract base class (see `types`).
        if let Ok(index) = self.schema_index().await {
            let _ = self.schema_index.set(index);
        }
        // An application is code on the App SDK (ADR-N-022); a dashboard and an analysis stay
        // on the kit until it is retired (T-0681).
        if self.kind == "application" {
            return self.drive_code(&mut inbox, deadline).await;
        }

        self.status(AgentRunStatus::Starting).await?;
        let types = self.types();
        if !types.is_empty() {
            self.thought(&format!(
                "Reading {SAMPLES_PER_TYPE} entities of {} through the endpoint.",
                types.join(", ")
            ))
            .await?;
        }
        let samples = self.samples(&types).await;
        let catalog = self.find(&self.prompt).await?;
        self.status(AgentRunStatus::Building).await?;

        let mut files: BTreeMap<String, String> = BTreeMap::new();
        let mut conversation: Vec<(String, String)> = Vec::new();
        let outcome = self
            .pass(
                &samples,
                &mut files,
                &conversation,
                &self.prompt,
                catalog.as_ref(),
            )
            .await?;
        let Some(prose) = outcome else {
            return Err("the first pass produced no specification the kit can render".to_owned());
        };
        conversation.push((self.prompt.clone(), prose));
        // The lifecycle has no edge from building to previewing; the validation of the
        // specification is the run's test, and it says so.
        self.status(AgentRunStatus::Testing).await?;
        self.status(AgentRunStatus::Previewing).await?;
        if self.unattended {
            self.status(AgentRunStatus::AwaitingApproval).await?;
        }

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
                    let catalog = self.find(&text).await?;
                    match self
                        .pass(&samples, &mut files, &conversation, &text, catalog.as_ref())
                        .await
                    {
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

    /// A code run (Architecture/20 §4.1): one call writes the application over the template,
    /// what does not build goes back once, every generated version is checked against what the
    /// frame observed (SDK-28), and every message after the first run is one more pass over the
    /// same files. The template is the model's context, never the preview (SDK-14).
    async fn drive_code(
        &self,
        inbox: &mut broadcast::Receiver<AgentRunEvent>,
        deadline: tokio::time::Instant,
    ) -> Result<(), String> {
        self.status(AgentRunStatus::Starting).await?;
        let mut files = preview::template_files();
        match self.jc_types().await {
            Ok(types) => {
                files.insert(code::TYPES.to_owned(), types);
            }
            Err(reason) => {
                self.thought(&format!(
                    "The row types of the endpoint were not rendered, so the template's \
                     placeholder types stand: {reason}"
                ))
                .await?
            }
        }
        // What the run branch holds, so every commit carries what changed since the last one.
        let mut committed = BTreeMap::new();
        self.thought("Writing the application for your request.")
            .await?;

        let types = self.types();
        if !types.is_empty() {
            self.thought(&format!(
                "Reading {SAMPLES_PER_TYPE} entities of {} through the endpoint.",
                types.join(", ")
            ))
            .await?;
        }
        let samples = self.samples(&types).await;
        self.status(AgentRunStatus::Building).await?;

        let mut conversation: Vec<(String, String)> = Vec::new();
        let mut instruction = self.prompt.clone();
        // The generated version in the frame, none until one transpiles.
        let mut shown: Option<Shown> = None;
        // Verification passes since the last instruction (SDK-28).
        let mut verified = 0;
        // A frame that reports an error but never an observation (an older page, a crash
        // before the application settles) is still checked, on the errors alone.
        let mut fallback: Option<tokio::time::Instant> = None;
        match self
            .code_pass(
                &samples,
                &mut files,
                &conversation,
                &instruction,
                None,
                false,
            )
            .await?
        {
            CodePass::Built { prose, .. } => {
                let (title, prose) = title_of(&prose);
                if let Some(title) = title {
                    if let Err(err) = self.state.agents.set_title(&self.run_id, &title).await {
                        tracing::warn!(run = %self.run_id, error = %err, "title not recorded");
                    }
                    self.event("title", json!({ "title": title })).await?;
                }
                shown = Some(
                    self.publish_code(&files, &prose, Some(&mut committed), true)
                        .await?,
                );
                conversation.push((instruction.clone(), prose));
                // The first version is small so it is on screen fast; the rest follows while the
                // person looks at it (SDK-13).
                self.thought("Completing the application: the other pages, functions and tests.")
                    .await?;
                match self
                    .code_pass(
                        &samples,
                        &mut files,
                        &conversation,
                        &instruction,
                        Some(Fix::Complete),
                        true,
                    )
                    .await
                {
                    Ok(CodePass::Built { prose }) => {
                        shown = Some(
                            self.publish_code(&files, &prose, Some(&mut committed), false)
                                .await?,
                        );
                        conversation.push((COMPLETE_TURN.to_owned(), prose));
                    }
                    Ok(CodePass::Unchanged(prose)) => self.thought(&prose).await?,
                    Ok(CodePass::Failed) => {}
                    Err(reason) => {
                        self.thought(&format!(
                            "The first version stays on screen; completing it failed: {reason}"
                        ))
                        .await?
                    }
                }
            }
            CodePass::Unchanged(prose) => {
                self.thought(&prose).await?;
                conversation.push((instruction.clone(), prose));
            }
            // The run keeps the files it starts from, so the next message and the function
            // route read them, though the frame shows none of them.
            CodePass::Failed => {
                let stored: serde_json::Map<String, Value> = files
                    .iter()
                    .map(|(path, content)| (path.clone(), Value::String(content.clone())))
                    .collect();
                self.state
                    .agents
                    .set_files(&self.run_id, Value::Object(stored))
                    .await
                    .map_err(|err| err.to_string())?;
            }
        }
        self.status(AgentRunStatus::Testing).await?;
        self.status(AgentRunStatus::Previewing).await?;
        if self.unattended {
            self.status(AgentRunStatus::AwaitingApproval).await?;
        }

        let mut queued: std::collections::VecDeque<AgentRunEvent> =
            std::collections::VecDeque::new();
        loop {
            let event = match queued.pop_front() {
                Some(event) => Some(event),
                None => tokio::select! {
                    event = inbox.recv() => match event {
                        Ok(event) => Some(event),
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return Ok(()),
                    },
                    () = tokio::time::sleep_until(deadline) => {
                        self.expire().await;
                        return Ok(());
                    }
                    () = sleep_until_some(fallback) => None,
                },
            };
            let Some(event) = event else {
                // No observation came after the error: check the version on the errors alone.
                fallback = None;
                let unobserved = shown.as_mut().filter(|shown| !shown.observed);
                if let Some(on_screen) = unobserved.map(|shown| {
                    shown.observed = true;
                    shown.clone()
                }) {
                    let check = Check {
                        samples: &samples,
                        conversation: &conversation,
                        instruction: &instruction,
                    };
                    if let Some(next) = self
                        .verify(
                            check,
                            &mut files,
                            &mut committed,
                            &on_screen,
                            None,
                            &mut verified,
                        )
                        .await?
                    {
                        shown = Some(next);
                    }
                }
                continue;
            };
            match event.kind.as_str() {
                "status" if is_terminal(&event) => return Ok(()),
                "preview_error" => {
                    if shown.as_ref().is_some_and(|shown| !shown.observed) && fallback.is_none() {
                        fallback = Some(tokio::time::Instant::now() + OBSERVATION_WAIT);
                    }
                }
                "preview_observation" => {
                    let version = event
                        .payload
                        .get("version")
                        .and_then(Value::as_u64)
                        .and_then(|v| u32::try_from(v).ok());
                    let Some(on_screen) = shown
                        .as_mut()
                        .filter(|shown| Some(shown.version) == version && !shown.observed)
                    else {
                        continue;
                    };
                    on_screen.observed = true;
                    fallback = None;
                    let on_screen = on_screen.clone();
                    // A person's message waiting behind the observation comes first; the
                    // check of a version the message is about to replace is dropped.
                    while let Ok(next) = inbox.try_recv() {
                        queued.push_back(next);
                    }
                    if queued
                        .iter()
                        .any(|next| next.kind == "message" && sent_by_person(next))
                    {
                        continue;
                    }
                    let check = Check {
                        samples: &samples,
                        conversation: &conversation,
                        instruction: &instruction,
                    };
                    if let Some(next) = self
                        .verify(
                            check,
                            &mut files,
                            &mut committed,
                            &on_screen,
                            Some(&event.payload),
                            &mut verified,
                        )
                        .await?
                    {
                        shown = Some(next);
                    }
                }
                "message" if sent_by_person(&event) => {
                    fallback = None;
                    verified = 0;
                    let text = event
                        .payload
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    instruction = text.clone();
                    self.thought("Working on your message…").await?;
                    match self
                        .code_pass(
                            &samples,
                            &mut files,
                            &conversation,
                            &text,
                            None,
                            shown.is_some(),
                        )
                        .await
                    {
                        Ok(CodePass::Built { prose, .. }) => {
                            shown = Some(
                                self.publish_code(
                                    &files,
                                    &prose,
                                    Some(&mut committed),
                                    shown.is_none(),
                                )
                                .await?,
                            );
                            conversation.push((text, prose));
                        }
                        Ok(CodePass::Unchanged(prose)) => {
                            self.thought(&prose).await?;
                            conversation.push((text, prose));
                        }
                        Ok(CodePass::Failed) => {}
                        Err(message) => {
                            self.thought(&format!("The pass failed: {message}")).await?
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// SDK-28: the version on screen checked against the run with no model call, and a
    /// verification pass when the check finds something, at most [`MAX_VERIFICATIONS`] after
    /// each instruction. The version a pass publishes is returned; it is checked in its turn
    /// when its own observation arrives.
    async fn verify(
        &self,
        check: Check<'_>,
        files: &mut BTreeMap<String, String>,
        committed: &mut BTreeMap<String, String>,
        on_screen: &Shown,
        observation: Option<&Value>,
        verified: &mut u32,
    ) -> Result<Option<Shown>, String> {
        let since = self
            .state
            .agents
            .events_since(&self.run_id, on_screen.seq)
            .await
            .map_err(|err| err.to_string())?;
        let found = verification::check(check.samples, &since, observation);
        if found.problems.is_empty() {
            if observation.is_some() {
                self.thought(&found.summary()).await?;
            }
            return Ok(None);
        }
        let list = found
            .problems
            .iter()
            .map(|problem| format!("- {problem}"))
            .collect::<Vec<_>>()
            .join("\n");
        if *verified >= MAX_VERIFICATIONS {
            self.thought(&format!(
                "The preview still shows problems after {MAX_VERIFICATIONS} verification \
                 passes; say what to change:\n{list}"
            ))
            .await?;
            return Ok(None);
        }
        *verified += 1;
        self.thought(&format!("Checking the preview found:\n{list}"))
            .await?;
        let mut asked = found.problems.clone();
        if let Some(observation) = observation {
            asked.push(verification::rendered(observation));
        }
        match self
            .code_pass(
                check.samples,
                files,
                check.conversation,
                check.instruction,
                Some(Fix::Preview(&asked)),
                true,
            )
            .await
        {
            Ok(CodePass::Built { prose, .. }) => Ok(Some(
                self.publish_code(files, &prose, Some(committed), false)
                    .await?,
            )),
            Ok(CodePass::Unchanged(prose)) => {
                self.thought(&prose).await?;
                Ok(None)
            }
            Ok(CodePass::Failed) => Ok(None),
            // The version on screen stands; a failed call is said, not a failed run.
            Err(message) => {
                self.thought(&format!("The verification pass failed: {message}"))
                    .await?;
                Ok(None)
            }
        }
    }

    /// One pass of a code run: a call, its blocks applied, the project checked, and one repair
    /// call when it does not build (SDK-13, SDK-14). A pass that still does not build leaves the
    /// files as they were and says why.
    ///
    /// `found` is what a verification of the version on screen found, when the pass is one;
    /// `on_screen` says whether a generated version is in the frame, which is what a pass that
    /// still does not build leaves there.
    async fn code_pass(
        &self,
        samples: &Value,
        files: &mut BTreeMap<String, String>,
        conversation: &[(String, String)],
        instruction: &str,
        ask: Option<Fix<'_>>,
        on_screen: bool,
    ) -> Result<CodePass, String> {
        let before = files.clone();
        let (mut prose, mut errors) = self
            .code_step(samples, files, conversation, instruction, ask)
            .await?;
        if !errors.is_empty() {
            self.thought(&format!(
                "The application does not build; asking for a repair:\n{}",
                errors.join("\n")
            ))
            .await?;
            (prose, errors) = self
                .code_step(
                    samples,
                    files,
                    conversation,
                    instruction,
                    Some(Fix::Build(&errors)),
                )
                .await?;
        }
        if !errors.is_empty() {
            *files = before;
            let said = if on_screen {
                format!(
                    "The application still does not build, so the preview keeps the version \
                     before this request:\n{}",
                    errors.join("\n")
                )
            } else {
                format!(
                    "The application could not be built:\n{}\nSend a message to try again.",
                    errors.join("\n")
                )
            };
            self.thought(&said).await?;
            return Ok(CodePass::Failed);
        }
        if *files == before {
            return Ok(CodePass::Unchanged(or_else(
                prose,
                "The application is unchanged.",
            )));
        }
        Ok(CodePass::Built {
            prose: or_else(prose, "The application is ready."),
        })
    }

    /// One call over the project: the blocks applied where SDK-11 allows, then what keeps the
    /// files from building. A refused block goes back only beside a problem, so a stray path
    /// never costs a second call.
    async fn code_step(
        &self,
        samples: &Value,
        files: &mut BTreeMap<String, String>,
        conversation: &[(String, String)],
        instruction: &str,
        fix: Option<Fix<'_>>,
    ) -> Result<(String, Vec<String>), String> {
        let first = fix.is_none() && conversation.is_empty() && instruction == self.prompt;
        let user = self
            .code_pack(samples, files, conversation, instruction, fix)
            .await;
        let budget = if first {
            FIRST_VERSION_BUDGET
        } else {
            CODE_OUTPUT_BUDGET
        };
        let answer = self.complete_within(&code::SYSTEM, &user, budget).await?;
        let (blocks, prose, unread) = patch::parse(&answer);
        let (applied, refused) = patch::apply_where(files, &blocks, code::writable, code::REFUSAL);
        self.event(
            "tool",
            json!({
                "tool": "apply_patch",
                "command": format!("{} block(s)", blocks.len() + unread),
                "exitCode": if refused.is_empty() && unread == 0 { 0 } else { 1 },
                "applied": applied,
                "refused": with_unread(&refused, unread),
            }),
        )
        .await?;
        let mut problems = code::problems(files);
        // The prose already says what those blocks did, so a lost one is a repair, not a note.
        if unread > 0 {
            problems.push(unread_problem(unread));
        }
        if blocks.is_empty() && conversation.is_empty() {
            problems.push(
                "the answer carried no SEARCH/REPLACE block; write the application as blocks"
                    .to_owned(),
            );
        }
        if !problems.is_empty() {
            problems.extend(
                refused
                    .iter()
                    .map(|refused| format!("{}: {}", refused.path, refused.reason)),
            );
        }
        Ok((prose, problems))
    }

    /// The user message of one code call (SDK-13): what every run of this Portal shares comes
    /// first, so the provider's prompt cache pays for it, then the endpoint's types and data,
    /// then the request.
    async fn code_pack(
        &self,
        samples: &Value,
        files: &BTreeMap<String, String>,
        conversation: &[(String, String)],
        instruction: &str,
        fix: Option<Fix<'_>>,
    ) -> String {
        let mut pack = String::from("## THE SDK\n\n");
        pack.push_str(code::SDK_API.trim());
        pack.push_str("\n\nWhat `@joinedcontext/sdk` exports:\n```ts\n");
        pack.push_str(code::SDK_EXPORTS.trim());
        pack.push_str("\n```\n\n## THE FILES OF THE PROJECT\n\n");
        let types = files.get_key_value(code::TYPES);
        for (path, content) in files
            .iter()
            .filter(|(path, _)| path.as_str() != code::TYPES)
            .chain(types)
        {
            let fence = path.rsplit('.').next().unwrap_or("text");
            pack.push_str(&format!("### {path}\n```{fence}\n{content}\n```\n"));
        }
        pack.push_str(
            "\n## THE DATA\n\nData needs (types and attributes the person asked for):\n```json\n",
        );
        pack.push_str(&serde_json::to_string_pretty(&self.served_needs()).unwrap_or_default());
        pack.push_str("\n```\n\n");
        pack.push_str(&endpoints::pack_section(&self.endpoints, &self.data_needs));
        if self.allows_write {
            let schema = fields::for_endpoint(
                &self.state,
                &self.project,
                &self.endpoint_slug,
                &self.types(),
            )
            .await;
            pack.push_str(
                "The application MAY write: its data needs carry a write operation, so a form \
                 saves through the endpoint with the person's own access. The attributes per \
                 type as the space's DataModel declares them (JSON Schema properties):\n```json\n",
            );
            match schema {
                Some(schema) => {
                    pack.push_str(&serde_json::to_string_pretty(&schema).unwrap_or_default())
                }
                None => pack.push_str("{}"),
            }
            pack.push_str("\n```\n");
        } else {
            pack.push_str(
                "The application may NOT write: its data needs carry no write operation. Add no \
                 form and no save; when the request asks to edit, say that editing needs write \
                 access in the data needs.\n",
            );
        }
        pack.push_str(
            "\nFive entities per type, as the SDK's rows (`options=keyValues`):\n```json\n",
        );
        pack.push_str(&serde_json::to_string_pretty(samples).unwrap_or_default());
        pack.push_str("\n```\n");
        if !conversation.is_empty() {
            pack.push_str("\n## THE CONVERSATION SO FAR\n\n");
            for (asked, answered) in conversation {
                pack.push_str(&format!("Person: {asked}\nYou: {answered}\n\n"));
            }
        }
        pack.push_str(&format!(
            "\n## THE REQUEST\n\n{}\n\n## THIS CALL\n\n",
            self.prompt
        ));
        match fix {
            Some(Fix::Build(errors)) => {
                pack.push_str(
                    "The last answer was applied and the project does not build. Fix every \
                     problem below, each named with its file and line, and answer with the blocks \
                     that fix them:\n",
                );
                for error in errors {
                    pack.push_str(&format!("- {error}\n"));
                }
                pack.push_str(&format!("\nThe request being fulfilled: {instruction}\n"));
            }
            Some(Fix::Preview(found)) => {
                pack.push_str(
                    "The application builds and is on screen. Checking its preview against the \
                     data found the problems below; the last item is what the preview rendered, \
                     page by page. Fix every problem at its cause, change nothing else, and \
                     answer with the blocks that fix them:\n",
                );
                for problem in found {
                    pack.push_str(&format!("- {problem}\n"));
                }
                pack.push_str(&format!("\nThe request being fulfilled: {instruction}\n"));
            }
            Some(Fix::Complete) => pack.push_str(
                "The first version of the application is on screen: the files above are it. \
                 Complete the application for the request: the other pages, filters, charts, \
                 maps, exports and functions the request and the data call for, and a test \
                 beside every page, component and function the application has. Keep the design \
                 and the page of the first version; change them only where completing needs it. \
                 Answer with SEARCH/REPLACE blocks over the files above.\n",
            ),
            None if conversation.is_empty() && instruction == self.prompt => pack.push_str(
                "Write the FIRST VERSION of the application for the request above, which goes on \
                 screen at once: `src/App.tsx`, the design (`src/design-tokens.json`, \
                 `src/app.css`) and the one page the request is most about, complete and working \
                 with the real data. No tests, no other page and no function unless that page \
                 needs it: a second call adds them while the person already looks at this \
                 version. Keep the whole answer under 6,000 tokens. Begin your sentences with \
                 the application's name in bold, two to five words that say what it shows, for \
                 example **Helsinki Traffic Alerts Map**; never a file name or an id.\n",
            ),
            None => pack.push_str(&format!(
                "The person says: {instruction}\n\nChange the application accordingly, with tests \
                 for what you change.\n"
            )),
        }
        pack
    }

    /// `src/jc-types.ts` of the run (SDK-10): the LinkML the endpoint projects, read through the
    /// proxy like a sample, rendered by Model Tools.
    async fn jc_types(&self) -> Result<String, String> {
        let index = match self.schema_index.get() {
            Some(index) => index.clone(),
            None => self.schema_index().await?,
        };
        let needed = self.types();
        let models = index
            .get("models")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let serves_needed = |model: &Value| {
            model["types"].as_array().is_some_and(|types| {
                types
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|t| needed.iter().any(|n| n == t))
            })
        };
        // ponytail: per endpoint, the first model publishing a needed type; an endpoint over two
        // models whose types the application both needs gets the other model's types as the
        // placeholder's.
        let mut chosen: Vec<&Value> = Vec::new();
        for at in 0..self.endpoints.len().max(1) {
            let of_endpoint =
                |model: &&Value| model["endpoint"].as_u64().unwrap_or(0) as usize == at;
            if let Some(model) = models
                .iter()
                .filter(of_endpoint)
                .filter(|model| model["unread"].as_bool() != Some(true))
                .find(|model| serves_needed(model))
            {
                chosen.push(model);
            }
        }
        if chosen.is_empty() {
            chosen.extend(models.first());
        }
        if chosen.is_empty() {
            return Err("the endpoint publishes no model".to_owned());
        }
        let mut rendered = Vec::new();
        for model in chosen {
            let major = model["version"]
                .as_u64()
                .ok_or("the endpoint's model has no version")?;
            let at = model["endpoint"].as_u64().unwrap_or(0) as usize;
            let types = async {
                let source = self
                    .read_text(&format!(
                        "{}/schema/v{major}/model.linkml.yaml",
                        self.data_base(at)
                    ))
                    .await?;
                crate::tools::model_tools::typescript(&self.state, &source).await
            }
            .await;
            match types {
                Ok(types) => rendered.push(types),
                // The primary's types are the file; another endpoint's that cannot be rendered
                // leaves its rows untyped rather than every row.
                Err(reason) if at == 0 => return Err(reason),
                Err(_) => {}
            }
        }
        if rendered.is_empty() {
            return Err("no model of the run's endpoints could be rendered".to_owned());
        }
        Ok(merge_declarations(&rendered))
    }

    /// Stores the files, says `prose`, commits what changed since `committed` when given, and
    /// points the frame at the new version (SDK-15, SDK-17).
    async fn publish_code(
        &self,
        files: &BTreeMap<String, String>,
        prose: &str,
        committed: Option<&mut BTreeMap<String, String>>,
        first_version: bool,
    ) -> Result<Shown, String> {
        let stored: serde_json::Map<String, Value> = files
            .iter()
            .map(|(path, content)| (path.clone(), Value::String(content.clone())))
            .collect();
        self.state
            .agents
            .set_files(&self.run_id, Value::Object(stored))
            .await
            .map_err(|err| err.to_string())?;
        self.thought(prose).await?;
        if let Some(committed) = committed {
            self.commit_files(files, committed, prose).await?;
        }
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
        if first_version {
            self.first_version().await;
        }
        let event = self.append("preview", json!({ "previewUrl": url })).await?;
        Ok(Shown {
            version: pass,
            seq: event.seq,
            observed: false,
        })
    }

    /// What changed since `committed`, as one commit on the run's branch (SDK-17, AP-24). A
    /// Portal without a forge keeps the run in its store alone; a forge that refuses is said in
    /// the chat and the version stands.
    async fn commit_files(
        &self,
        files: &BTreeMap<String, String>,
        committed: &mut BTreeMap<String, String>,
        message: &str,
    ) -> Result<(), String> {
        let Some(gitea) = self.state.gitea.clone() else {
            return Ok(());
        };
        if self.branch.is_empty() {
            return Ok(());
        }
        let uploads: Vec<(String, String)> = files
            .iter()
            .filter(|(path, content)| committed.get(*path) != Some(*content))
            .map(|(path, content)| (format!("{}{path}", self.path_prefix), content.clone()))
            .collect();
        let gone: Vec<String> = committed
            .keys()
            .filter(|path| !files.contains_key(*path))
            .map(|path| format!("{}{path}", self.path_prefix))
            .collect();
        if uploads.is_empty() && gone.is_empty() {
            return Ok(());
        }
        let outcome: Result<String, GitError> = async {
            if gitea.branch_head(&self.branch).await.is_err() {
                let base = gitea.default_branch().await?;
                gitea.create_branch(&self.branch, &base).await?;
            }
            let mut deletes = Vec::new();
            for path in gone {
                if let Some(file) = gitea.get_file(&path, &self.branch).await? {
                    deletes.push((path, file.sha));
                }
            }
            gitea
                .change_files(
                    &self.branch,
                    &commit_message(message),
                    Author {
                        name: &self.created_by,
                        email: "agent@joinedcontext.local",
                    },
                    &uploads,
                    &deletes,
                )
                .await
        }
        .await;
        match outcome {
            Ok(sha) => {
                *committed = files.clone();
                self.event(
                    "commit",
                    json!({ "sha": sha, "message": commit_subject(message) }),
                )
                .await
            }
            Err(err) => {
                self.thought(&format!("The commit did not land in the forge: {err}"))
                    .await
            }
        }
    }

    /// The run's first version and the timings it closes (AG-66).
    async fn first_version(&self) {
        if let Err(err) = self.state.agents.record_first_version(&self.run_id).await {
            tracing::warn!(run = %self.run_id, error = %err, "first version not recorded");
        }
        if let Ok(Some(r)) = self.state.agents.get_run(&self.run_id).await {
            if let Some(ms) = r.first_frame_ms {
                crate::telemetry::record_run_timing("first_frame", &r.profile, ms);
            }
            if let Some(ms) = r.first_version_ms {
                crate::telemetry::record_run_timing("first_version", &r.profile, ms);
            }
        }
    }

    async fn drive_conversation(
        &self,
        inbox: &mut broadcast::Receiver<AgentRunEvent>,
        deadline: tokio::time::Instant,
    ) -> Result<(), String> {
        self.status(AgentRunStatus::Starting).await?;
        self.status(AgentRunStatus::Interviewing).await?;

        let mut prior_events = Vec::new();
        if let Some(ref prior_id) = self.continues {
            if let Ok(evts) = self.state.agents.events_since(prior_id, 0).await {
                prior_events = evts
                    .into_iter()
                    .filter(|e| e.kind == "message" || e.kind == "thought")
                    .collect();
                if prior_events.len() > 40 {
                    prior_events = prior_events.split_off(prior_events.len() - 40);
                }
            }
        }

        let mut conversation: Vec<(String, String)> = Vec::new();
        let mut current_person = String::new();
        let mut current_assistant = String::new();
        for e in prior_events {
            if e.kind == "message" {
                if !current_person.is_empty() || !current_assistant.is_empty() {
                    conversation.push((
                        std::mem::take(&mut current_person),
                        std::mem::take(&mut current_assistant),
                    ));
                }
                current_person = e
                    .payload
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
            } else if e.kind == "thought" {
                let text = e
                    .payload
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !current_assistant.is_empty() {
                    current_assistant.push_str("\n\n");
                }
                current_assistant.push_str(text);
            }
        }
        if !current_person.is_empty() || !current_assistant.is_empty() {
            conversation.push((current_person, current_assistant));
        }

        if !self.prompt.trim().is_empty() {
            match self.converse(&conversation, &self.prompt).await {
                Ok(prose) => conversation.push((self.prompt.clone(), prose)),
                Err(reason) => {
                    let _ = self.thought(&format!("The answer failed: {reason}")).await;
                }
            }
        }

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
                    match self.converse(&conversation, &text).await {
                        Ok(prose) => conversation.push((text, prose)),
                        Err(reason) => {
                            let _ = self.thought(&format!("The answer failed: {reason}")).await;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) async fn converse(
        &self,
        conversation: &[(String, String)],
        text: &str,
    ) -> Result<String, String> {
        let catalog = self.find(text).await?;
        let base = self.conversation_pack(conversation, text, catalog.as_ref());
        // The endpoints the conversation reads, as they stand for this message, each with the
        // read tools it offers; the model may open more of the project's as it works (AG-75,
        // AG-76).
        let mut chosen = match self.state.agents.get_run(&self.run_id).await {
            Ok(Some(run)) => endpoints::of_run(&run),
            _ => self.endpoints.clone(),
        };
        let mut tools = self.data_tools(&chosen).await;
        let mut results: Vec<(data_query::QueryCall, String)> = Vec::new();
        let mut drafts = 0;
        let answer = loop {
            let section = data_query::section(&chosen, &tools, &self.openable_endpoints(&chosen));
            let user = format!(
                "{}{}",
                base.replacen("\n## THIS TURN", &format!("\n{section}\n## THIS TURN"), 1),
                data_query::results_section(&results)
            );
            let answer = self
                .complete_with_system(CONVERSATION_SYSTEM, &user)
                .await?;

            let calls = data_query::tool_calls(&answer);
            if !calls.is_empty() {
                let left = data_query::MAX_CALLS.saturating_sub(results.len());
                if left == 0 {
                    let prose = "I could not finish from the data within the calls one message \
                                 may make; ask a narrower question."
                        .to_owned();
                    self.thought(&prose).await?;
                    return Ok(prose);
                }
                // Endpoints first, one at a time, so the calls that follow may all run at once.
                let mut ready = Vec::new();
                for call in calls.into_iter().take(left) {
                    ready.push(match call {
                        Ok(call) => match self
                            .open_endpoint(&mut chosen, &mut tools, &call.endpoint)
                            .await
                        {
                            Ok(_) => Ok(call),
                            Err(reason) => Err((call, reason)),
                        },
                        Err(reason) => Err((
                            data_query::QueryCall {
                                endpoint: String::new(),
                                name: String::new(),
                                arguments: json!({}),
                            },
                            reason,
                        )),
                    });
                }
                let (chosen_now, tools_now) = (&chosen, &tools);
                let answered =
                    futures_util::future::join_all(ready.into_iter().map(|call| async move {
                        match call {
                            Ok(call) => {
                                let text = self.query_endpoint(chosen_now, &call, tools_now).await;
                                text.map(|text| (call, text))
                            }
                            Err((call, reason)) => Ok((call, format!("error: {reason}"))),
                        }
                    }))
                    .await;
                for said in answered {
                    results.push(said?);
                }
                continue;
            }

            let last = drafts + 1 >= data_query::MAX_DRAFTS;
            if let Some(call) = kpi_pipeline::tool_call(&answer) {
                let input = call
                    .as_ref()
                    .ok()
                    .and_then(|c| serde_json::to_value(c).ok());
                match self
                    .kpi_pipeline(call, &answer, &mut chosen, &mut tools, last)
                    .await?
                {
                    Worked::Done(prose) => return Ok(prose),
                    Worked::Again(reason) => {
                        drafts += 1;
                        results.push((drafted("draft_kpi_pipeline", input), reason));
                        continue;
                    }
                }
            }
            if let Some(call) = kpi::tool_call(&answer) {
                let input = call
                    .as_ref()
                    .ok()
                    .and_then(|c| serde_json::to_value(c).ok());
                match self
                    .kpi(call, &answer, &mut chosen, &mut tools, last)
                    .await?
                {
                    Worked::Done(prose) => return Ok(prose),
                    Worked::Again(reason) => {
                        drafts += 1;
                        results.push((drafted("compute_kpi", input), reason));
                        continue;
                    }
                }
            }
            break answer;
        };

        if let Some(call) = share::tool_call(&answer) {
            return self.share(call, &answer).await;
        }
        if let Some(call) = share::edit_call(&answer) {
            return self.edit_endpoint(call, &answer).await;
        }
        if let Some(call) = space_complete_tool_call(&answer) {
            return self.space_complete(call, &answer).await;
        }
        if let Some(route) = navigate_tool_call(&answer) {
            let mut prose = share::prose_of(&answer);
            if prose.is_empty() {
                prose = "You can start that work from the Assistant page.".to_owned();
            }
            self.thought(&prose).await?;
            if route.starts_with('/') && !route.starts_with("//") {
                self.event(
                    "navigate",
                    json!({
                        "route": route,
                    }),
                )
                .await?;
            }
            return Ok(prose);
        }

        let prose = if answer.trim().is_empty() {
            "I am ready to help.".to_owned()
        } else {
            answer.trim().to_owned()
        };
        self.thought(&prose).await?;
        Ok(prose)
    }

    fn conversation_pack(
        &self,
        conversation: &[(String, String)],
        text: &str,
        catalog: Option<&Value>,
    ) -> String {
        let mut pack = String::new();
        pack.push_str(
            "You are the joinedcontext Portal assistant. Answer in plain prose, or answer with \
             exactly one of the tool calls below. Never write files or SEARCH/REPLACE blocks.\n\n",
        );
        if let Some(catalog) = catalog {
            pack.push_str(
                "## WHAT THE CATALOG SEARCH FOUND\n\nThe project's endpoints, spaces and \
                 data models matching the person's words, with the caller's access verdict and \
                 the freshness of the pipeline feeding each, read from the platform. Name them by \
                 their `name`; invent no other.\n```json\n",
            );
            pack.push_str(&serde_json::to_string_pretty(catalog).unwrap_or_default());
            pack.push_str("\n```\n\n");
        }
        pack.push_str(&format!(
            r#"## WHEN THE PERSON ASKS TO SHARE OR PUBLISH DATA

A request to share, publish, open or expose data with somebody (a team, a project, a partner,
the public). Answer with one or two plain sentences and then ONE fenced JSON block, nothing else, in this shape:

```json
{{
  "tool": "propose_endpoint",
  "contextSpace": "<the space the data lives in>",
  "name": "<a short lowercase dns-1123 name for the endpoint>",
  "title": "<a title in the language of the request>",
  "audience": "project-list",
  "allowedProjects": ["<the project or team named, as a lowercase dns-1123 name>"],
  "representations": ["ngsi-ld", "geojson"],
  "hiddenAttributes": ["<attributes the person wants hidden>"],
  "entityTypes": ["<the types shared>"]
}}
```

`audience` is "project-list" unless the person says the whole organization ("organization") or
everyone ("public"). The platform mints the slug, renders the manifests and opens the form;
the person submits.

## WHEN THE PERSON ASKS FOR AN INDICATOR, A KPI OR ONE NUMBER OVER THE DATA

"What is the average PM10", "how many stations are closed", "define a KPI for free bikes": the
platform computes it, not you. Answer with one or two plain sentences and then ONE fenced JSON
block, nothing else, in this shape:

```json
{{
  "tool": "compute_kpi",
  "name": "<a short lowercase name with dashes, e.g. average-pm10>",
  "title": "<a title in the language of the request>",
  "type": "<the entity type, exact name as the endpoint's schema says>",
  "attribute": "<the attribute folded, exact name as the schema says; empty for count>",
  "agg": "avg | sum | count | min | max",
  "unit": "<a UN/CEFACT common code when the value has a unit, e.g. GQ for µg/m³, C62 for a count>",
  "q": "<an NGSI-LD filter narrowing the entities, or omit it>",
  "endpoint": "<the endpoint read, from WORKING WITH THE DATA; required when the conversation reads none>"
}}
```

Read the type's schema or a page of its entities first when you do not know its exact attribute
names. The platform reads the entities through the endpoint, computes the value, renders the
`KeyPerformanceIndicator` entity with its formula and provenance, and shows it to the person. When
the read fails or finds no number, you get the reason back and try again.

## WHEN THE PERSON ASKS TO KEEP AN INDICATOR UPDATED

"Keep the average of free bikes updated every 15 minutes", "recompute it on every change",
"save the indicators into transportation-kpi", "Keep the indicator … updated", "create a KPI
pipeline", "a pipeline from one context space (or context broker) to another computing KPIs": a
pipeline, not one number. The source is an endpoint of the project; the target is an indicator
space. Never ask for a URL or for which data: find it yourself, and when the person names no
metric, pick one the data plainly supports (a count of the type, or the sum or average of its
main numeric attribute), say which in one sentence, and draft it. A person who asked for a KPI pipeline earlier in the
conversation and now names the metric ("total available city bikes") or says "just find it"
wants that pipeline. Work it
as an agent: find the endpoint that holds the data (the catalog search, WORKING WITH THE DATA),
read the type's schema and a page of entities, then draft. Answer with one or two plain
sentences and then ONE fenced JSON block, nothing else:

```json
{{
  "tool": "draft_kpi_pipeline",
  "name": "<a short lowercase name with dashes, e.g. bikes-available-avg>",
  "title": "<a title in the language of the request>",
  "type": "<the entity type>",
  "attribute": "<the attribute folded; empty for count>",
  "agg": "avg | sum | count | min | max",
  "unit": "<a UN/CEFACT common code, e.g. C62 for a count>",
  "q": "<an NGSI-LD filter narrowing the entities, or omit it>",
  "sourceEndpoint": "<the endpoint read, from WORKING WITH THE DATA; required when the conversation reads none>",
  "targetSpace": "<the indicator space written, ending with -kpi; omit for {project}-kpi>",
  "every": "<a period of at least a minute such as 15m or 1h; omit when onChange>",
  "onChange": false,
  "watchedAttributes": ["<attributes whose change recomputes it, when onChange; the folded one by default>"]
}}
```

Give `every` or `"onChange": true`, never both. When the person names no period, use "15m". The
platform drafts the Bento pipeline, drafts the indicator space when it does not exist, and tests
the pipeline on a page of the source. A refused plan or a red test comes back to you with the
reason and the page it ran on: look at the data again and draft it anew. A green draft opens its
form; the person proposes it.

## WHEN THE PERSON ASKS TO COMPLETE A CONTEXT SPACE

A message that gives the URL of a data feed is an integration, whether or not it says so:
"integrate https://…", "load this feed", a URL with a description of the data, a URL with a
specification pasted below it (a field list, a JSON Schema, a vendor's documentation). Answer
with one or two plain sentences and then ONE fenced JSON block:

```json
{{
  "tool": "space_complete",
  "space": "<a context space name: lowercase letters, digits and dashes, from what the data is>",
  "url": "<the URL exactly as the person wrote it>",
  "typeName": "<the entity type in PascalCase, singular, from the description or the specification, e.g. WeatherObserved, BikeHireDockingStation>",
  "description": "<one paragraph: what the data is and what its fields mean, from the description and the specification; omit it when the person gave neither>"
}}
```

The platform probes the URL, infers the data model under that type, drafts the context space,
the data source, the pipeline and its endpoint, checks each, and opens them for the person to
review and propose. You never propose them yourself.

A request that names completing a context space, loading files or integrating a feed, without a
URL, cannot run from the chat: say in one or two sentences that the person drops the folder or
the files on the Complete this space page, and open it with ONE fenced JSON block. Data that is
already in the project is never an integration: "find it", "use what is there", a pipeline or an
indicator over existing data is worked from WORKING WITH THE DATA.

```json
{{
  "tool": "navigate",
  "route": "/projects/{project}/spaces/complete"
}}
```

## WHEN THE PERSON ASKS TO BUILD AN APPLICATION OR A DASHBOARD

To build an application or a dashboard, explain in one or two plain sentences that they can start it from the Assistant page, and navigate them there with ONE fenced JSON block:

```json
{{
  "tool": "navigate",
  "route": "/projects/{project}/assistant"
}}
```
"#,
            project = self.project
        ));
        let endpoints = self.project_endpoints();
        if !endpoints.is_empty() {
            pack.push_str(&format!(
                r#"
## WHEN THE PERSON ASKS TO CHANGE AN ENDPOINT

A request to change an endpoint that exists: make it public, add or remove a format, hide or show
an attribute, let another project read it, change its title or its rate limit. Name only an
endpoint from this list, the project's endpoints as they are now:

```json
{summaries}
```

Answer with one or two plain sentences and then ONE fenced JSON block, nothing else, carrying
only the fields that change:

```json
{{
  "tool": "edit_endpoint",
  "name": "<an endpoint name from the list>",
  "title": "<the new title>",
  "audience": "public | organization | project-list",
  "allowedProjects": ["<every project that may read it, when the audience is project-list>"],
  "addRepresentations": ["<formats to add: {representations}>"],
  "removeRepresentations": ["<formats to remove>"],
  "hiddenAttributes": ["<every attribute hidden after the change>"],
  "requestsPerMinute": 600
}}
```

The platform opens the endpoint's form with the change filled in; the person reviews it and
proposes it.
"#,
                summaries = serde_json::to_string_pretty(&share::endpoint_summaries(&endpoints))
                    .unwrap_or_default(),
                representations = share::REPRESENTATIONS.join(", "),
            ));
        }

        if !conversation.is_empty() {
            pack.push_str("\n## THE CONVERSATION SO FAR\n\n");
            for (asked, answered) in conversation {
                pack.push_str(&format!("Person: {asked}\nYou: {answered}\n\n"));
            }
        }

        pack.push_str(&format!("\n## THIS TURN\n\nPerson: {text}\n"));
        self.narrowed(&pack)
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
        catalog: Option<&Value>,
    ) -> Result<Option<String>, String> {
        if !files.is_empty() {
            self.thought("Working on your message…").await?;
        }
        let user = self
            .pack(samples, files, conversation, instruction, catalog, None)
            .await;
        let answer = self.complete(&user).await?;
        // A share request is answered with a tool call, not a file (EP-72): rendered, shown,
        // and handed to the endpoint form; the dashboard stays as it was.
        if let Some(call) = share::tool_call(&answer) {
            return self.share(call, &answer).await.map(Some);
        }
        // An indicator is computed here, shown, and written by the person (PF-55): the
        // dashboard stays as it was.
        if let Some(call) = kpi::tool_call(&answer) {
            let (mut chosen, mut tools) = (self.endpoints.clone(), Vec::new());
            return match self
                .kpi(call, &answer, &mut chosen, &mut tools, true)
                .await?
            {
                Worked::Done(prose) | Worked::Again(prose) => Ok(Some(prose)),
            };
        }
        // A space complete request is answered with a tool call (T-0642)
        if let Some(call) = space_complete_tool_call(&answer) {
            return self.space_complete(call, &answer).await.map(Some);
        }
        let before = files.clone();
        let (mut prose, mut errors) = self.apply(files, &answer).await?;
        if !errors.is_empty() {
            // One repair call: the model is shown what did not validate and answers again.
            self.thought(&format!(
                "The specification does not validate; asking for a repair:\n{}",
                errors.join("\n")
            ))
            .await?;
            let user = self
                .pack(
                    samples,
                    files,
                    conversation,
                    instruction,
                    catalog,
                    Some(&errors),
                )
                .await;
            let answer = self.complete(&user).await?;
            (prose, errors) = self.apply(files, &answer).await?;
        }
        if !errors.is_empty() {
            self.thought(&format!(
                "Still not a specification the kit can render:\n{}",
                errors.join("\n")
            ))
            .await?;
            *files = before;
            return Ok(None);
        }
        // An answer that changes no file is a turn of the conversation, not a version: nothing
        // is stored or committed and the frame keeps what it shows (AP-60).
        if !before.is_empty() && *files == before {
            let prose = if prose.trim().is_empty() {
                "The dashboard is unchanged.".to_owned()
            } else {
                prose.trim().to_owned()
            };
            self.thought(&prose).await?;
            return Ok(Some(prose));
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
        self.commit(&prose).await?;
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
        if pass == 1 {
            self.first_version().await;
        }
        self.event("preview", json!({ "previewUrl": url })).await?;
        Ok(Some(prose))
    }

    /// Applies an answer to the files and says what the kit thinks of `spec.json`.
    async fn apply(
        &self,
        files: &mut BTreeMap<String, String>,
        answer: &str,
    ) -> Result<(String, Vec<String>), String> {
        let (blocks, prose, unread) = patch::parse(answer);
        let mut allowed = vec![kit::SPEC_FILE, kit::PAGE_FILE];
        if self.kind == "analysis" {
            allowed.push("report.md");
        }
        let (applied, refused) = patch::apply(files, &blocks, &allowed);
        self.event(
            "tool",
            json!({
                "tool": "apply_patch",
                "command": format!("{} block(s)", blocks.len() + unread),
                "exitCode": if refused.is_empty() && unread == 0 { 0 } else { 1 },
                "applied": applied,
                "refused": with_unread(&refused, unread),
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
            Some(text) => match kit::parse(text) {
                Err(problems) => errors.extend(problems),
                Ok(spec) => errors.extend(self.form_errors(&spec)),
            },
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
    async fn pack(
        &self,
        samples: &Value,
        files: &BTreeMap<String, String>,
        conversation: &[(String, String)],
        instruction: &str,
        catalog: Option<&Value>,
        errors: Option<&[String]>,
    ) -> String {
        let mut pack = String::new();
        pack.push_str("## THE APPLICATION\n\n");
        pack.push_str(&self.prompt);
        if let Some(catalog) = catalog {
            // What the search found is what the model may name (AG-58): the endpoints and
            // spaces of the project, with the verdict and the freshness the platform read.
            pack.push_str(
                "\n\n## WHAT THE CATALOG SEARCH FOUND\n\nThe project's endpoints, spaces and \
                 data models matching the person's words, with the caller's access verdict and \
                 the freshness of the pipeline feeding each, read from the platform. Name them by \
                 their `name`; invent no other.\n```json\n",
            );
            pack.push_str(&serde_json::to_string_pretty(catalog).unwrap_or_default());
            pack.push_str("\n```");
        }
        pack.push_str("\n\n## THE DATA THE ENDPOINT PUBLISHES\n\n");
        pack.push_str("Data needs (types and attributes the person asked for):\n```json\n");
        pack.push_str(&serde_json::to_string_pretty(&self.served_needs()).unwrap_or_default());
        pack.push_str("\n```\n\nSample entities per type (`options=keyValues`):\n```json\n");
        pack.push_str(&serde_json::to_string_pretty(samples).unwrap_or_default());
        pack.push_str("\n```\n\n## THE FIELDS A FORM MAY EDIT\n\n");
        if self.allows_write {
            // The field schema of AP-61: the same one the preview inlines for the kit's form.
            let schema = fields::for_endpoint(
                &self.state,
                &self.project,
                &self.endpoint_slug,
                &self.types(),
            )
            .await;
            pack.push_str(
                "This application MAY write: its data needs carry a write operation, so a `form` \
                 view saves through the endpoint. The attributes per type as the space's \
                 DataModel declares them (JSON Schema properties: type, enum, minimum, maximum, \
                 pattern; `required` lists the mandatory ones):\n```json\n",
            );
            match schema {
                Some(schema) => {
                    pack.push_str(&serde_json::to_string_pretty(&schema).unwrap_or_default())
                }
                None => pack.push_str(
                    "{}\n(the space has no inline DataModel: take the kinds from the samples)",
                ),
            }
            pack.push_str("\n```");
        } else {
            pack.push_str(
                "This application may NOT write: its data needs carry no write operation. Add \
                 no `form` view; when the person asks to edit, say that editing needs write \
                 access in the data needs.",
            );
        }
        pack.push_str("\n\n## KIT CAPABILITIES\n\nThe views, options and export formats supported by the kit (name only what is here):\n```json\n");
        pack.push_str(KIT_CAPABILITIES.trim());
        pack.push_str("\n```\n");
        if self.kind == "analysis" {
            pack.push_str(
                "\n\n## ANALYSIS REPORT\n\nBeside `spec.json`, also write `report.md` as a SEARCH/REPLACE block. \
                 `report.md` is an in-depth markdown summary of what the data shows, including key metrics, \
                 distributions, and statistical findings using the numbers from the rows.\n",
            );
        }

        pack.push_str("\n\n## THE CURRENT FILES\n\n");
        // Only what the model may write. The rows of the preview live beside the specification
        // in the same map, and a hundred kilobytes of them in the prompt is a minute of reading.
        let visible: Vec<(&String, &String)> = files
            .iter()
            .filter(|(path, _)| path.as_str() != kit::DATA_FILE)
            .collect();
        if visible.is_empty() {
            pack.push_str("(none yet: write spec.json with an empty SEARCH block)\n");
        }
        for (path, content) in visible {
            let fence = if path.ends_with(".html") {
                "html"
            } else if path.ends_with(".md") {
                "markdown"
            } else {
                "json"
            };
            pack.push_str(&format!("### {path}\n```{fence}\n{content}\n```\n"));
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
                    "The person says: {instruction}\n\nChange spec.json accordingly. If what is \
                     asked cannot be drawn with the views (3D, a bespoke chart, an animation, a \
                     free layout, a custom widget), write index.html as well, as the section \
                     WHEN THE VIEWS ARE NOT ENOUGH says. Only what needs the platform to write \
                     (a save to the endpoint, a login, a file upload) is out of reach: say so \
                     plainly in the sentences before the block, and do the nearest thing.\n"
                ));
            }
        }
        pack
    }

    /// One call through the proxy, asked twice when the first answer carries no text: a
    /// provider answers empty now and then, and a second call is cheaper than a failed run.
    async fn complete(&self, user: &str) -> Result<String, String> {
        self.complete_with_system(&self.narrowed(&SYSTEM), user)
            .await
    }

    async fn complete_with_system(&self, system: &str, user: &str) -> Result<String, String> {
        self.complete_within(system, user, OUTPUT_BUDGET).await
    }

    /// A provider that says the key's credit covers fewer output tokens than asked is asked
    /// once more within what it covers: most answers are far shorter than the budget, and a
    /// run should not stop over a ceiling it would not have reached.
    async fn complete_within(
        &self,
        system: &str,
        user: &str,
        budget: u32,
    ) -> Result<String, String> {
        let first = match self.complete_once(system, user, budget).await {
            Err(CallError::Empty) => self.complete_once(system, user, budget).await,
            Err(CallError::Credit {
                affordable: Some(afford),
            }) if afford >= MIN_CREDIT_BUDGET && afford < budget => {
                self.complete_once(system, user, afford - afford / 20).await
            }
            other => other,
        };
        first.map_err(|err| err.said(budget))
    }

    /// One call through the proxy, in the body the profile's provider reads (AG-53).
    async fn complete_once(
        &self,
        system: &str,
        user: &str,
        budget: u32,
    ) -> Result<String, CallError> {
        let (path, body) = if self.provider == "anthropic" {
            (
                "/v1/llm/messages",
                json!({
                    "model": self.model,
                    "max_tokens": budget,
                    "system": system,
                    "messages": [{ "role": "user", "content": user }],
                }),
            )
        } else {
            (
                "/v1/llm/chat/completions",
                json!({
                    "model": self.model,
                    "max_tokens": budget,
                    "messages": [
                        { "role": "system", "content": system },
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
            .map_err(|err| {
                CallError::Failed(format!(
                    "the model call did not go through the proxy: {err}"
                ))
            })?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::PAYMENT_REQUIRED {
            return Err(CallError::Credit {
                affordable: affordable_tokens(&text),
            });
        }
        if !status.is_success() {
            return Err(CallError::Failed(format!(
                "the proxy answered {status} to the model call: {}",
                provider_said(&text)
            )));
        }
        let answer: Value = serde_json::from_str(&text)
            .map_err(|err| CallError::Failed(format!("the model's answer is not JSON: {err}")))?;
        // An answer the budget cut is not applied at all: half a page is worse than none.
        let cut = answer
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| reason == "length")
            || answer
                .get("stop_reason")
                .and_then(Value::as_str)
                .is_some_and(|reason| reason == "max_tokens");
        if cut {
            return Err(CallError::Failed(format!(
                "the answer was cut at the output budget of {budget} tokens and nothing \
                 was applied; ask for less at once"
            )));
        }
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
            return Err(CallError::Empty);
        }
        Ok(joined)
    }

    /// The entity types the data needs name, in order, once each.
    /// What the schema and the kit's own checks cannot say about a `form` (AP-61, AP-62): it
    /// needs an application that may write, and its fields are attributes the data needs
    /// declare for that type, so a form never edits what the endpoint never granted.
    fn form_errors(&self, spec: &kit::Spec) -> Vec<String> {
        let mut errors = Vec::new();
        for (index, view) in spec.views.iter().enumerate() {
            let kit::View::Form { source, fields, .. } = view else {
                continue;
            };
            let path = format!("views[{index}]");
            if !self.allows_write {
                errors.push(format!(
                    "{path}: a form writes through the endpoint and this application may not write (no write operation in its data needs); remove the form"
                ));
                continue;
            }
            let source = spec
                .sources
                .iter()
                .find(|s| Some(s.name.as_str()) == source.as_deref())
                .or(spec.sources.first());
            let Some(source) = source else {
                continue;
            };
            let declared = self.need_attrs(&source.entity_type);
            for (i, field) in fields.iter().flatten().enumerate() {
                if !declared.iter().any(|attr| attr == field) {
                    errors.push(format!(
                        "{path}.fields[{i}]: '{field}' is not an attribute the data needs declare for {}",
                        source.entity_type
                    ));
                }
            }
        }
        errors
    }

    /// The attributes the data needs declare for one type.
    fn need_attrs(&self, entity_type: &str) -> Vec<String> {
        let mut attrs = Vec::new();
        for need in self.data_needs.as_array().into_iter().flatten() {
            let names = need
                .get("types")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str);
            if !names.into_iter().any(|t| t == entity_type) {
                continue;
            }
            for attr in need
                .get("attrs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !attrs.iter().any(|known| known == attr) {
                    attrs.push(attr.to_owned());
                }
            }
        }
        attrs
    }

    /// The types the data needs name that the endpoint serves, in the order they were named.
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
        served_only(types, self.schema_index.get())
    }

    /// The data needs as the model reads them: every type the endpoint does not serve left out,
    /// and a need left with no type dropped.
    fn served_needs(&self) -> Value {
        needs_served(&self.data_needs, self.schema_index.get())
    }

    /// The endpoint's schema index, read through the proxy with the run's ticket (EP-46). A run
    /// over several endpoints gets one index: every endpoint's models, each marked with the
    /// index of its endpoint; an endpoint whose index cannot be read keeps the types its needs
    /// name, so a proxy that does not serve it yet narrows nothing away.
    async fn schema_index(&self) -> Result<Value, String> {
        let mut index = self
            .read_entities(&format!("{}/schema/index.json", self.data_base(0)))
            .await?;
        if self.endpoints.len() < 2 {
            return Ok(index);
        }
        let by_endpoint = endpoints::types_by_endpoint(&self.endpoints, &self.data_needs);
        let mut models: Vec<Value> = Vec::new();
        for at in 0..self.endpoints.len() {
            let read = if at == 0 {
                Ok(index.clone())
            } else {
                self.read_entities(&format!("{}/schema/index.json", self.data_base(at)))
                    .await
            };
            match read {
                Ok(read) => models.extend(
                    read["models"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .cloned()
                        .map(|mut model| {
                            model["endpoint"] = json!(at);
                            model
                        }),
                ),
                Err(_) => models.push(json!({
                    "endpoint": at,
                    "unread": true,
                    "types": by_endpoint.get(at).cloned().unwrap_or_default(),
                })),
            }
        }
        index["models"] = Value::Array(models);
        Ok(index)
    }

    /// Where the data of the run's endpoint at `at` is read through the proxy.
    fn data_base(&self, at: usize) -> String {
        endpoints::data_base(&self.proxy_base, &self.endpoints, at)
    }

    /// Where the entities of one type are read: through the endpoint of the need naming it.
    fn data_base_of(&self, entity_type: &str) -> String {
        self.data_base(endpoints::of_type(
            &self.endpoints,
            &self.data_needs,
            entity_type,
        ))
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
                    "{}/ngsi-ld/v1/entities?type={}&options=keyValues&limit={page}&offset={}",
                    self.data_base_of(&source.entity_type),
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
        let body = self.read_text(url).await?;
        serde_json::from_str::<Value>(&body).map_err(|err| err.to_string())
    }

    /// One GET through the proxy with the run's ticket, as text.
    async fn read_text(&self, url: &str) -> Result<String, String> {
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
        Ok(body)
    }

    /// A few entities per type, read through the proxy like the application will (AP-57). A
    /// type that cannot be read is an empty list with the reason beside it: the model still
    /// gets the data needs, and the chat says what was missing.
    async fn samples(&self, types: &[String]) -> Value {
        let mut samples = serde_json::Map::new();
        for entity_type in types {
            let url = format!(
                "{}/ngsi-ld/v1/entities?type={}&limit={SAMPLES_PER_TYPE}&options=keyValues",
                self.data_base_of(entity_type),
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

    /// The specification committed to the run's branch, so the application exists in the forge
    /// from its first pass and a closed tab loses nothing (AP-24). A Portal without a forge
    /// keeps the run in its store alone; a forge that refuses is said in the chat and the pass
    /// stands.
    async fn commit(&self, message: &str) -> Result<(), String> {
        let Some(gitea) = self.state.gitea.clone() else {
            return Ok(());
        };
        if self.branch.is_empty() {
            return Ok(());
        }
        let Some(spec) = self
            .state
            .agents
            .get_run(&self.run_id)
            .await
            .ok()
            .flatten()
            .and_then(|run| {
                run.files
                    .get(kit::SPEC_FILE)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
        else {
            return Ok(());
        };
        let path = format!("{}src/{}", self.path_prefix, kit::SPEC_FILE);
        let outcome: Result<String, GitError> = async {
            if gitea.branch_head(&self.branch).await.is_err() {
                let base = gitea.default_branch().await?;
                gitea.create_branch(&self.branch, &base).await?;
            }
            let sha = gitea
                .get_file(&path, &self.branch)
                .await?
                .map(|file| file.sha);
            gitea
                .put_file(&FileWrite {
                    path: &path,
                    branch: &self.branch,
                    message,
                    content: &spec,
                    sha: sha.as_deref(),
                    author: Author {
                        name: &self.created_by,
                        email: "agent@joinedcontext.local",
                    },
                })
                .await
        }
        .await;
        match outcome {
            Ok(sha) => {
                self.event(
                    "commit",
                    json!({ "sha": sha, "message": commit_subject(message) }),
                )
                .await
            }
            Err(err) => {
                self.thought(&format!("The commit did not land in the forge: {err}"))
                    .await
            }
        }
    }

    /// The `propose_endpoint` tool: the manifests rendered and published as a step, then a
    /// `navigate` that opens the endpoint form with them (EP-72, UI-45). A request that cannot
    /// be rendered is a failed step the person reads in the chat; nothing is written either way.
    /// The KPI step (T-0583, PF-54, PF-55): the entities read through the proxy like a
    /// sample, the aggregate computed, the indicator rendered and handed over as a `tool`
    /// event; the card the person sees carries the write, with their own session.
    async fn space_complete(&self, input: Value, answer: &str) -> Result<String, String> {
        let started = std::time::Instant::now();
        let millis = |started: std::time::Instant| {
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
        };
        let Some(op) = crate::ops::find("jc_space_complete") else {
            return Err("jc_space_complete not found".into());
        };
        if let Err(reason) = self.granted("jc_space_complete") {
            return self.refused("space_complete", started, input, reason).await;
        }
        let caller = crate::ops::Caller {
            identity: self.identity.clone(),
            via: crate::ops::Via::Session,
        };
        // The operation reads its own fields only: the call's `tool` key goes, and the
        // assistant never proposes, the person does on the page it opens (AG-73).
        let mut input = input;
        if let Some(fields) = input.as_object_mut() {
            fields.remove("tool");
            fields.remove("propose");
        }
        match crate::ops::call(op, &caller, &self.state, &self.project, input.clone()).await {
            Ok(output) => {
                let space_name = output
                    .get("space")
                    .and_then(Value::as_str)
                    .unwrap_or("space");
                self.event(
                    "tool",
                    json!({
                        "tool": "space_complete",
                        "status": "ok",
                        "durationMs": millis(started),
                        "input": input,
                        "output": output,
                    }),
                )
                .await?;
                let mut prose = share::prose_of(answer);
                if prose.is_empty() {
                    prose = format!("Completed drafts for context space '{space_name}'.");
                }
                self.thought(&prose).await?;
                self.event(
                    "navigate",
                    json!({
                        "route": format!("/projects/{}/spaces/complete?space={}", self.project, space_name),
                    }),
                )
                .await?;
                Ok(prose)
            }
            Err(e) => {
                let reason = e.to_string();
                self.event(
                    "tool",
                    json!({
                        "tool": "space_complete",
                        "status": "failed",
                        "durationMs": millis(started),
                        "input": input,
                        "error": reason,
                    }),
                )
                .await?;
                let prose = format!("Completing space failed: {reason}");
                self.thought(&prose).await?;
                Ok(prose)
            }
        }
    }

    async fn kpi(
        &self,
        call: Result<kpi::ComputeKpi, String>,
        answer: &str,
        chosen: &mut Vec<endpoints::RunEndpoint>,
        tools: &mut Vec<Vec<Value>>,
        last: bool,
    ) -> Result<Worked, String> {
        let started = std::time::Instant::now();
        let millis = |started: std::time::Instant| {
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
        };
        let failed = |reason: String| {
            json!({
                "tool": "compute_kpi",
                "status": "failed",
                "durationMs": millis(started),
                "error": reason,
            })
        };
        // A failure the model can correct goes back to it while drafts are left (AG-76).
        let again = |reason: String, prose: String| {
            if last {
                Worked::Done(prose)
            } else {
                Worked::Again(format!("error: {reason}"))
            }
        };
        let params = match call {
            Ok(params) => params,
            Err(reason) => {
                self.event("tool", failed(reason.clone())).await?;
                let prose = format!("The indicator request could not be read: {reason}");
                if last {
                    self.thought(&prose).await?;
                }
                return Ok(again(reason, prose));
            }
        };
        let input = serde_json::to_value(&params).unwrap_or(Value::Null);
        if let Err(reason) = self.granted("jc_kpi_compute") {
            return self
                .refused("compute_kpi", started, input, reason)
                .await
                .map(Worked::Done);
        }
        let named = params
            .endpoint
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let index = match named {
            Some(name) => self.open_endpoint(chosen, tools, name).await,
            None if chosen.is_empty() => Err(
                "the conversation reads no endpoint: name the one the indicator reads (endpoint)"
                    .to_owned(),
            ),
            None => Ok(endpoints::of_type(
                chosen,
                &self.data_needs,
                &params.entity_type,
            )),
        };
        let index = match index {
            Ok(index) => index,
            Err(reason) => {
                self.event("tool", failed(reason.clone())).await?;
                let prose = format!("The indicator has no endpoint to read: {reason}");
                if last {
                    self.thought(&prose).await?;
                }
                return Ok(again(reason, prose));
            }
        };
        let mut url = format!(
            "{}/ngsi-ld/v1/entities?type={}&options=keyValues&limit=1000",
            endpoints::data_base(&self.proxy_base, chosen, index),
            urlencoding(&params.entity_type)
        );
        if !params.attribute.is_empty() {
            url.push_str(&format!("&attrs={}", urlencoding(&params.attribute)));
        }
        if let Some(q) = params.q.as_deref().filter(|q| !q.trim().is_empty()) {
            url.push_str(&format!("&q={}", urlencoding(q)));
        }
        let rows = match self.read_entities(&url).await {
            Ok(Value::Array(rows)) => rows,
            Ok(_) => Vec::new(),
            Err(reason) => {
                self.event("tool", failed(reason.clone())).await?;
                let prose = format!("The entities could not be read: {reason}");
                if last {
                    self.thought(&prose).await?;
                }
                return Ok(again(reason, prose));
            }
        };
        let (value, count) = kpi::compute(&rows, &params.attribute, params.agg);
        let Some(value) = value else {
            let reason = format!(
                "no {} carries a number in '{}' ({} read)",
                params.entity_type,
                params.attribute,
                rows.len()
            );
            self.event("tool", failed(reason.clone())).await?;
            let prose = format!("The indicator has no value: {reason}.");
            if last {
                self.thought(&prose).await?;
            }
            return Ok(again(reason, prose));
        };
        // The endpoint read, for the provenance; the indicator space's endpoint, when the
        // project has one, for the card's write.
        let (endpoint_name, endpoint_space) = match chosen.get(index) {
            Some(read) => (
                read.name.clone(),
                if read.space.is_empty() {
                    self.project.clone()
                } else {
                    read.space.clone()
                },
            ),
            None => (self.project.clone(), self.project.clone()),
        };
        let now = now_rfc3339();
        let provenance = kpi::Provenance {
            org_domain: &crate::api::assistant::org_domain(&self.state, &self.project),
            project: &self.project,
            endpoint_space: &endpoint_space,
            endpoint_name: &endpoint_name,
            run_id: &self.run_id,
            now: &now,
        };
        let indicator = match kpi::entity(&params, value, &provenance) {
            Ok(indicator) => indicator,
            Err(reason) => {
                self.event("tool", failed(reason.clone())).await?;
                let prose = format!("The indicator could not be rendered: {reason}");
                self.thought(&prose).await?;
                return Ok(Worked::Done(prose));
            }
        };
        let target = kpi::kpi_endpoint(&self.state, &self.project);
        self.event(
            "tool",
            json!({
                "tool": "compute_kpi",
                "status": "ok",
                "durationMs": millis(started),
                "input": input,
                "output": {
                    "name": params.name,
                    "title": params.title,
                    "value": value,
                    "unit": params.unit,
                    "formula": kpi::formula(&params),
                    "count": count,
                    "space": jc_core::kpi::kpi_space(&self.project),
                    "endpointSlug": target.as_ref().map(|(slug, _)| slug.clone()),
                    "endpointName": target.as_ref().map(|(_, name)| name.clone()),
                    "entity": indicator.to_json(),
                },
            }),
        )
        .await?;
        let mut prose = share::prose_of(answer);
        if prose.is_empty() {
            prose = format!(
                "{} = {value} over {count} entities; write it into the indicator space from the card.",
                kpi::formula(&params)
            );
        }
        self.thought(&prose).await?;
        Ok(Worked::Done(prose))
    }

    /// An indicator kept up to date (AG-74, PL-45, PL-51): the pipeline and, for an indicator
    /// space the project does not have, its space, endpoint and policies, kept as the person's
    /// drafts; the pipeline tested on a page of the source and its form opened. Nothing is
    /// proposed here.
    async fn kpi_pipeline(
        &self,
        call: Result<kpi_pipeline::DraftKpiPipeline, String>,
        answer: &str,
        chosen: &mut Vec<endpoints::RunEndpoint>,
        tools: &mut Vec<Vec<Value>>,
        last: bool,
    ) -> Result<Worked, String> {
        const TOOL: &str = "draft_kpi_pipeline";
        let started = std::time::Instant::now();
        let millis = |started: std::time::Instant| {
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
        };
        let failed = |input: &Value, reason: &str| {
            json!({
                "tool": TOOL,
                "status": "failed",
                "durationMs": millis(started),
                "input": input,
                "error": reason,
            })
        };
        // A refused plan or a red test goes back to the model while drafts are left (AG-76).
        let again = |reason: String, prose: String| {
            if last {
                Worked::Done(prose)
            } else {
                Worked::Again(reason)
            }
        };
        let params = match call {
            Ok(params) => params,
            Err(reason) => {
                self.event("tool", failed(&Value::Null, &reason)).await?;
                let prose = format!("The pipeline request could not be read: {reason}");
                if last {
                    self.thought(&prose).await?;
                }
                return Ok(again(format!("error: {reason}"), prose));
            }
        };
        let input = serde_json::to_value(&params).unwrap_or(Value::Null);
        if let Err(reason) = self.granted("jc_pipeline_propose") {
            return self
                .refused(TOOL, started, input, reason)
                .await
                .map(Worked::Done);
        }
        // The source is read like every other endpoint of the conversation: opened first when
        // the person may read it, so the test runs on a real page (AG-76).
        if let Some(named) = params
            .source_endpoint
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            if let Err(reason) = self.open_endpoint(chosen, tools, named).await {
                self.event("tool", failed(&input, &reason)).await?;
                let prose = format!("The pipeline could not be drafted: {reason}");
                if last {
                    self.thought(&prose).await?;
                }
                return Ok(again(format!("error: {reason}"), prose));
            }
        }

        let endpoints = self.project_endpoints();
        let listed = |kind: &str| {
            self.state
                .mirror
                .list(&self.project, kind, &crate::store::ListOptions::default())
                .items
        };
        let spaces: Vec<String> = listed("ContextSpace")
            .into_iter()
            .map(|env| env.metadata.name)
            .collect();
        let policies: Vec<Value> = listed("Policy")
            .iter()
            .filter_map(|env| serde_json::to_value(env).ok())
            .collect();
        let run_endpoint = chosen.first().map(|e| e.name.clone());
        let org_domain = crate::api::assistant::org_domain(&self.state, &self.project);
        let slug = share::slug();
        let world = kpi_pipeline::World {
            project: &self.project,
            org_domain: &org_domain,
            endpoints: &endpoints,
            spaces: &spaces,
            policies: &policies,
            run_endpoint: run_endpoint.as_deref(),
            new_slug: &slug,
        };
        let plan = match kpi_pipeline::plan(&params, &world) {
            Ok(plan) => plan,
            Err(reason) => {
                self.event("tool", failed(&input, &reason)).await?;
                let prose = format!("The pipeline could not be drafted: {reason}");
                if last {
                    self.thought(&prose).await?;
                }
                return Ok(again(format!("error: {reason}"), prose));
            }
        };
        let name = params.name.trim().to_owned();

        for drafted in &plan.space_drafts {
            if let Err(err) = self
                .state
                .drafts
                .put(
                    &self.project,
                    &drafted.kind,
                    &drafted.name,
                    drafted.manifest.clone(),
                    None,
                    &self.created_by,
                    "assistant",
                )
                .await
            {
                tracing::warn!(run = %self.run_id, kind = %drafted.kind, error = %err, "draft not kept");
            }
        }
        if let Err(err) = self
            .state
            .drafts
            .put(
                &self.project,
                "Pipeline",
                &name,
                plan.pipeline.clone(),
                None,
                &self.created_by,
                "assistant",
            )
            .await
        {
            tracing::warn!(run = %self.run_id, error = %err, "pipeline draft not kept");
        }

        // The test runs on a page of the source when this run reads that endpoint, and on an
        // empty page otherwise, so the mapping and the indicator's admission are checked either way.
        let source_index = chosen.iter().position(|e| e.name == plan.source_endpoint);
        let sampled = source_index.is_some();
        let page = if let Some(index) = source_index {
            let mut url = format!(
                "{}/ngsi-ld/v1/entities?type={}&limit=100",
                endpoints::data_base(&self.proxy_base, chosen, index),
                urlencoding(params.entity_type.trim())
            );
            if !params.attribute.trim().is_empty() {
                url.push_str(&format!("&attrs={}", urlencoding(params.attribute.trim())));
            }
            if let Some(q) = params.q.as_deref().filter(|q| !q.trim().is_empty()) {
                url.push_str(&format!("&q={}", urlencoding(q)));
            }
            self.read_text(&url)
                .await
                .unwrap_or_else(|_| "[]".to_owned())
        } else {
            "[]".to_owned()
        };
        let verdict = match crate::ops::find("jc_pipeline_test") {
            Some(op) => {
                let caller = crate::ops::Caller {
                    identity: self.identity.clone(),
                    via: crate::ops::Via::Session,
                };
                let test = json!({
                    "pipeline": plan.pipeline,
                    "sample": { "text": page, "format": "text" },
                    "draft": { "kind": "Pipeline", "name": name },
                });
                match crate::ops::call(op, &caller, &self.state, &self.project, test).await {
                    Ok(out) => out.get("verdict").cloned().unwrap_or(Value::Null),
                    Err(err) => json!({ "ok": false, "untested": err.to_string() }),
                }
            }
            None => json!({ "ok": false, "untested": "jc_pipeline_test is not registered" }),
        };

        // A red test the model can act on goes back to it; a test that could not run at all (no
        // runner) is shown on the card, as before.
        let red = verdict.get("ok").and_then(Value::as_bool) == Some(false)
            && verdict.get("untested").is_none();
        if red {
            let findings = verdict
                .get("findings")
                .and_then(Value::as_array)
                .map(|all| {
                    all.iter()
                        .map(|f| {
                            format!(
                                "{} {}",
                                f.get("path").and_then(Value::as_str).unwrap_or(""),
                                f.get("message").and_then(Value::as_str).unwrap_or("")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .unwrap_or_default();
            let reason = format!(
                "the pipeline's test on {} is not green: {findings}. The page it ran on ({} characters): {}",
                if sampled { "a page of the source" } else { "an empty page" },
                page.len(),
                page.chars().take(2_000).collect::<String>()
            );
            self.event(
                "tool",
                json!({
                    "tool": TOOL,
                    "status": "failed",
                    "durationMs": millis(started),
                    "input": input,
                    "error": format!("the test is not green: {findings}"),
                    "output": { "verdict": verdict, "pipeline": plan.pipeline },
                }),
            )
            .await?;
            if !last {
                return Ok(Worked::Again(reason));
            }
            let prose = format!(
                "The pipeline '{name}' is drafted but its test is still not green: {findings}. The draft is kept on the Pipelines page."
            );
            self.thought(&prose).await?;
            return Ok(Worked::Done(prose));
        }

        let drafts: Vec<Value> = plan
            .space_drafts
            .iter()
            .map(|d| json!({ "kind": d.kind, "name": d.name, "plural": d.plural, "manifest": d.manifest }))
            .collect();
        // The runner's token names the slugs it may write through; a new endpoint's slug is not
        // among them until the deployment's runner client says so (Architecture/08, KPI pipelines).
        let audience = plan
            .space_drafts
            .iter()
            .any(|d| d.kind == "Endpoint")
            .then(|| plan.target_slug.clone())
            .flatten();
        self.event(
            "tool",
            json!({
                "tool": TOOL,
                "status": "ok",
                "durationMs": millis(started),
                "input": input,
                "output": {
                    "name": name,
                    "title": params.title,
                    "formula": plan.formula,
                    "trigger": plan.trigger.describe(),
                    "sourceEndpoint": plan.source_endpoint,
                    "sourceSpace": plan.source_space,
                    "targetSpace": plan.target_space,
                    "targetEndpoint": plan.target_endpoint,
                    "targetSlug": plan.target_slug,
                    "sampled": sampled,
                    "verdict": verdict,
                    "pipeline": plan.pipeline,
                    "drafts": drafts,
                    "runnerAudience": audience,
                },
            }),
        )
        .await?;

        let mut prose = share::prose_of(answer);
        if prose.is_empty() {
            prose = format!(
                "Drafted the pipeline '{name}': {} into {}, {}; review it in the form and propose it.",
                plan.formula,
                plan.target_space,
                plan.trigger.describe()
            );
        }
        if !plan.space_drafts.is_empty() {
            prose.push_str(&format!(
                " The indicator space {} does not exist yet: its drafts are on the card; propose them with the pipeline.",
                plan.target_space
            ));
        }
        self.thought(&prose).await?;
        self.event(
            "navigate",
            json!({
                "route": format!("/projects/{}/pipelines", self.project),
                "prefill": plan.prefill,
                "draft": { "kind": "Pipeline", "name": name },
            }),
        )
        .await?;
        Ok(Worked::Done(prose))
    }

    async fn share(
        &self,
        call: Result<share::ProposeEndpoint, String>,
        answer: &str,
    ) -> Result<String, String> {
        let started = std::time::Instant::now();
        let millis = |started: std::time::Instant| {
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
        };
        let params = match call {
            Ok(params) => params,
            Err(reason) => {
                self.event(
                    "tool",
                    json!({
                        "tool": "propose_endpoint",
                        "status": "failed",
                        "durationMs": millis(started),
                        "error": reason,
                    }),
                )
                .await?;
                let prose = format!("The share request could not be read: {reason}");
                self.thought(&prose).await?;
                return Ok(prose);
            }
        };
        let input = serde_json::to_value(&params).unwrap_or(Value::Null);
        if let Err(reason) = self.granted("jc_endpoint_propose") {
            return self
                .refused("propose_endpoint", started, input, reason)
                .await;
        }
        let domain = crate::api::assistant::org_domain(&self.state, &self.project);
        match share::render(&self.project, &domain, &params) {
            Ok(proposal) => {
                let output = serde_json::to_value(&proposal).unwrap_or(Value::Null);
                self.event(
                    "tool",
                    json!({
                        "tool": "propose_endpoint",
                        "status": "ok",
                        "durationMs": millis(started),
                        "input": input,
                        "output": output,
                    }),
                )
                .await?;
                let mut prose = share::prose_of(answer);
                if prose.is_empty() {
                    prose = format!(
                        "Drafted the endpoint '{}'; review it in the form and propose it.",
                        params.name
                    );
                }
                self.thought(&prose).await?;
                let manifest = serde_json::to_value(&proposal.endpoint).unwrap_or_else(|_| {
                    serde_json::to_value(&proposal.prefill).unwrap_or(Value::Null)
                });
                let _ = self
                    .state
                    .drafts
                    .put(
                        &self.project,
                        "Endpoint",
                        &params.name,
                        manifest,
                        None,
                        &self.created_by,
                        "assistant",
                    )
                    .await;
                self.event(
                    "navigate",
                    json!({
                        "route": format!("/projects/{}/endpoints", self.project),
                        "prefill": proposal.prefill,
                        "draft": {
                            "kind": "Endpoint",
                            "name": params.name,
                        },
                    }),
                )
                .await?;
                Ok(prose)
            }
            Err(reason) => {
                self.event(
                    "tool",
                    json!({
                        "tool": "propose_endpoint",
                        "status": "failed",
                        "durationMs": millis(started),
                        "input": input,
                        "error": reason,
                    }),
                )
                .await?;
                let prose = format!("The endpoint could not be drafted: {reason}");
                self.thought(&prose).await?;
                Ok(prose)
            }
        }
    }

    /// The project's endpoints as manifests, from the Portal's mirror of the repository.
    fn project_endpoints(&self) -> Vec<Value> {
        self.state
            .mirror
            .list(
                &self.project,
                "Endpoint",
                &crate::store::ListOptions::default(),
            )
            .items
            .iter()
            .filter_map(|envelope| serde_json::to_value(envelope).ok())
            .collect()
    }

    /// A change to an endpoint that exists (EP-72, AG-56): the edited manifest is kept as the
    /// person's draft and the endpoint form opens on it with the change filled in. Nothing is
    /// proposed here; the person reviews the form and proposes it.
    async fn edit_endpoint(
        &self,
        call: Result<share::EditEndpoint, String>,
        answer: &str,
    ) -> Result<String, String> {
        let started = std::time::Instant::now();
        let millis = |started: std::time::Instant| {
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
        };
        let params = match call {
            Ok(params) => params,
            Err(reason) => {
                self.event(
                    "tool",
                    json!({
                        "tool": "edit_endpoint",
                        "status": "failed",
                        "durationMs": millis(started),
                        "error": reason,
                    }),
                )
                .await?;
                let prose = format!("The change could not be read: {reason}");
                self.thought(&prose).await?;
                return Ok(prose);
            }
        };
        let input = serde_json::to_value(&params).unwrap_or(Value::Null);
        if let Err(reason) = self.granted("jc_endpoint_propose") {
            return self.refused("edit_endpoint", started, input, reason).await;
        }
        let edit = match share::edit(&self.project_endpoints(), &params) {
            Ok(edit) => edit,
            Err(reason) => {
                self.event(
                    "tool",
                    json!({
                        "tool": "edit_endpoint",
                        "status": "failed",
                        "durationMs": millis(started),
                        "input": input,
                        "error": reason,
                    }),
                )
                .await?;
                let prose = format!("The endpoint could not be changed: {reason}");
                self.thought(&prose).await?;
                return Ok(prose);
            }
        };
        let name = params.name.trim().to_owned();
        self.event(
            "tool",
            json!({
                "tool": "edit_endpoint",
                "status": "ok",
                "durationMs": millis(started),
                "input": input,
                "output": { "name": name, "changes": edit.changes },
            }),
        )
        .await?;
        let mut prose = share::prose_of(answer);
        if prose.is_empty() {
            let fields: Vec<&str> = edit.changes.iter().map(|c| c.field.as_str()).collect();
            prose = format!(
                "Opened '{name}' with the new {}; review it in the form and propose it.",
                fields.join(", ")
            );
        }
        self.thought(&prose).await?;
        let _ = self
            .state
            .drafts
            .put(
                &self.project,
                "Endpoint",
                &name,
                edit.endpoint,
                None,
                &self.created_by,
                "assistant",
            )
            .await;
        self.event(
            "navigate",
            json!({
                "route": format!("/projects/{}/endpoints", self.project),
                "prefill": edit.prefill,
                "draft": { "kind": "Endpoint", "name": name },
            }),
        )
        .await?;
        Ok(prose)
    }

    /// The catalog search over the person's words, published as the `search_catalog` tool
    /// step (AG-58, UI-46); `None` when nothing matched, so the prompt stays as it was.
    async fn find(&self, question: &str) -> Result<Option<Value>, String> {
        // The platform runs the search, not the model: outside the run's access it is skipped,
        // and the prompt goes without it (AG-70).
        if self.granted("jc_catalog_search").is_err() {
            return Ok(None);
        }
        let started = std::time::Instant::now();
        let catalog =
            crate::api::assistant::search(&self.state, &self.project, question, None).await;
        let output = serde_json::to_value(&catalog).unwrap_or(Value::Null);
        self.event(
            "tool",
            json!({
                "tool": "search_catalog",
                "status": "ok",
                "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "input": { "q": question },
                "output": output.clone(),
            }),
        )
        .await?;
        Ok((!catalog.items.is_empty()).then_some(output))
    }

    /// The profile and the person who started the run both allow the operation behind a tool
    /// (AG-70).
    fn granted(&self, operation: &str) -> Result<(), String> {
        self.access
            .check(operation, &self.identity, &self.state, &self.project)
    }

    /// A tool call outside the run's access: a failed `tool` event before anything runs, and the
    /// reason in the chat (AG-56, AG-70).
    async fn refused(
        &self,
        tool: &str,
        started: std::time::Instant,
        input: Value,
        reason: String,
    ) -> Result<String, String> {
        self.event(
            "tool",
            json!({
                "tool": tool,
                "status": "failed",
                "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "input": input,
                "error": reason,
            }),
        )
        .await?;
        let prose = format!("That is outside what this assistant may do: {reason}");
        self.thought(&prose).await?;
        Ok(prose)
    }

    /// A prompt without the sections of the tools this run may not call, so the model is shown
    /// only the effective tool set (AG-70).
    fn narrowed(&self, prompt: &str) -> String {
        TOOL_SECTIONS
            .iter()
            .filter(|(_, operation)| self.granted(operation).is_err())
            .fold(prompt.to_owned(), |text, (heading, _)| {
                without_section(&text, heading)
            })
    }

    /// Each endpoint's read tools, from its own `tools/list` through the proxy, so the model
    /// sees exactly what the gateway offers this person there (AG-75). An endpoint that does
    /// not answer offers nothing.
    async fn data_tools(&self, chosen: &[endpoints::RunEndpoint]) -> Vec<Vec<Value>> {
        let lists = (0..chosen.len()).map(|index| self.tools_of(chosen, index));
        futures_util::future::join_all(lists).await
    }

    /// The read tools one endpoint of the conversation offers, from its own `tools/list`.
    async fn tools_of(&self, chosen: &[endpoints::RunEndpoint], index: usize) -> Vec<Value> {
        let url = format!(
            "{}/mcp",
            endpoints::data_base(&self.proxy_base, chosen, index)
        );
        let list = self
            .http
            .post(&url)
            .bearer_auth(&self.bearer)
            .header("accept", "application/json")
            .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
            .send()
            .await;
        match list {
            Ok(response) if response.status().is_success() => response
                .json::<Value>()
                .await
                .map(|list| data_query::read_only_tools(&list))
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// The project's endpoints the conversation does not read yet and the person may open: an
    /// audience that admits the project and a profile that grants reading it (AG-58, AG-70).
    fn openable_endpoints(&self, chosen: &[endpoints::RunEndpoint]) -> Vec<data_query::Openable> {
        self.state
            .mirror
            .list(
                &self.project,
                "Endpoint",
                &crate::store::ListOptions::default(),
            )
            .items
            .into_iter()
            .filter(|env| !chosen.iter().any(|c| c.name == env.metadata.name))
            .filter(|env| {
                env.spec["slug"]
                    .as_str()
                    .is_some_and(|slug| !slug.is_empty())
            })
            .filter(|env| {
                crate::api::assistant::endpoint_access(&env.spec, &self.project).is_allowed()
            })
            .filter(|env| self.access.grants_endpoint(&env.metadata.name, false))
            .map(|env| data_query::Openable {
                title: crate::api::assistant::title_of(&env),
                space: crate::api::assistant::ref_name(&env.spec["contextSpaceRef"])
                    .unwrap_or_default(),
                name: env.metadata.name,
            })
            .collect()
    }

    /// The index of `name` among the conversation's endpoints, opening it first when the person
    /// may read it: stored on the run and shown in the data bar, as if the person had used it
    /// (AG-76). `Err` is what the model reads back.
    async fn open_endpoint(
        &self,
        chosen: &mut Vec<endpoints::RunEndpoint>,
        tools: &mut Vec<Vec<Value>>,
        name: &str,
    ) -> Result<usize, String> {
        if let Some(index) = chosen.iter().position(|e| e.name == name) {
            return Ok(index);
        }
        let openable = self.openable_endpoints(chosen);
        if !openable.iter().any(|o| o.name == name) {
            return Err(format!(
                "'{name}' is not an endpoint the person may read in this project; they may open: {}",
                openable
                    .iter()
                    .map(|o| o.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if chosen.len() >= endpoints::MAX_ENDPOINTS {
            return Err(format!(
                "a conversation reads at most {} endpoints: {}",
                endpoints::MAX_ENDPOINTS,
                chosen
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let opened = endpoints::resolve(&self.state.mirror, &self.project, &[name.to_owned()])
            .map_err(|err| err.to_string())?;
        chosen.extend(opened);
        self.state
            .agents
            .set_endpoints(&self.run_id, chosen)
            .await
            .map_err(|err| err.to_string())?;
        self.event(
            "endpoints",
            json!({ "names": chosen.iter().map(|e| e.name.clone()).collect::<Vec<_>>() }),
        )
        .await?;
        let index = chosen.len() - 1;
        let offered = self.tools_of(chosen, index).await;
        tools.resize(chosen.len() - 1, Vec::new());
        tools.push(offered);
        Ok(index)
    }

    /// One `tools/call` on an endpoint of the conversation, on the log as a `query_endpoint`
    /// step; what the model reads back, errors included, so it can correct itself.
    async fn query_endpoint(
        &self,
        chosen: &[endpoints::RunEndpoint],
        call: &data_query::QueryCall,
        tools: &[Vec<Value>],
    ) -> Result<String, String> {
        let started = std::time::Instant::now();
        let input =
            json!({ "endpoint": call.endpoint, "name": call.name, "arguments": call.arguments });
        let index = chosen.iter().position(|e| e.name == call.endpoint);
        let refusal = match index {
            None => Some(format!(
                "'{}' is not an endpoint of this conversation; the endpoints are: {}",
                call.endpoint,
                chosen
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            Some(i) if !data_query::offers(tools.get(i).map_or(&[], Vec::as_slice), &call.name) => {
                Some(format!(
                    "'{}' is not a read tool endpoint '{}' offers",
                    call.name, call.endpoint
                ))
            }
            Some(_) => None,
        };
        let millis = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Some(reason) = refusal {
            self.event(
                "tool",
                json!({ "tool": "query_endpoint", "status": "failed", "durationMs": millis(), "input": input, "error": reason }),
            )
            .await?;
            return Ok(format!("error: {reason}"));
        }
        let url = format!(
            "{}/mcp",
            endpoints::data_base(&self.proxy_base, chosen, index.unwrap_or(0))
        );
        let answer = match self
            .http
            .post(&url)
            .bearer_auth(&self.bearer)
            .header("accept", "application/json")
            .json(&data_query::rpc(call))
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                if status.is_success() {
                    serde_json::from_str::<Value>(&body).unwrap_or_else(
                        |err| json!({ "error": format!("the endpoint's answer is not JSON: {err}") }),
                    )
                } else {
                    json!({ "error": format!("{status}: {}", body.chars().take(300).collect::<String>()) })
                }
            }
            Err(err) => json!({ "error": format!("the call did not go through the proxy: {err}") }),
        };
        let text = data_query::result_text(&answer);
        let failed = answer.get("error").is_some()
            || answer.pointer("/result/isError").and_then(Value::as_bool) == Some(true);
        let mut payload = json!({
            "tool": "query_endpoint",
            "status": if failed { "failed" } else { "ok" },
            "durationMs": millis(),
            "input": input,
            "output": answer.get("result").cloned().unwrap_or(Value::Null),
        });
        if failed {
            payload["error"] = Value::String(text.clone());
        }
        self.event("tool", payload).await?;
        Ok(text)
    }

    async fn thought(&self, text: &str) -> Result<(), String> {
        self.event("thought", json!({ "text": text })).await
    }

    async fn event(&self, kind: &str, payload: Value) -> Result<(), String> {
        self.append(kind, payload).await.map(|_| ())
    }

    /// [`Self::event`], answering the event as stored, its sequence number included.
    async fn append(&self, kind: &str, payload: Value) -> Result<AgentRunEvent, String> {
        let event = self
            .state
            .agents
            .append_event(&self.run_id, kind, payload)
            .await
            .map_err(|err| err.to_string())?;
        self.state.agent_events.broadcast(&event).await;
        Ok(event)
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

    /// The wall clock ran out while the run was previewing: the run has ended, the preview
    /// stays as built (AP-60). Not a failure, so no `error` travels with the status and the
    /// page shows the state's own words (T-0669).
    async fn expire(&self) {
        let _ = self
            .state
            .agents
            .set_status(&self.run_id, AgentRunStatus::Expired, None)
            .await;
        let _ = self
            .event(
                "status",
                json!({ "status": AgentRunStatus::Expired.as_str(), "timestamp": now_rfc3339() }),
            )
            .await;
    }
}

/// What one pass of a code run came to.
/// What a code call is asked to fix, when it is a repair.
#[derive(Clone, Copy)]
enum Fix<'a> {
    /// The project does not build: transpile errors and refused imports.
    Build(&'a [String]),
    /// The project builds and its preview's check found these (SDK-28).
    Preview(&'a [String]),
    /// The first version is on screen; the rest of the application follows (SDK-13).
    Complete,
}

/// A generated version in the frame: its `v`, the sequence number of its `preview` event, so
/// what happened after it is its own, and whether its observation has arrived.
#[derive(Clone)]
struct Shown {
    version: u32,
    seq: i64,
    observed: bool,
}

/// What a verification pass works over.
#[derive(Clone, Copy)]
struct Check<'a> {
    samples: &'a Value,
    conversation: &'a [(String, String)],
    instruction: &'a str,
}

/// Verification passes after one instruction (SDK-28).
const MAX_VERIFICATIONS: u32 = 3;
/// How long a version that reported an error waits for its observation before it is checked on
/// the errors alone.
const OBSERVATION_WAIT: Duration = Duration::from_secs(15);

/// Sleeps until `at`, or forever when there is nothing to wait for.
async fn sleep_until_some(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

enum CodePass {
    /// The files changed and build.
    Built { prose: String },
    /// The answer changed no file: a turn of the conversation, not a version.
    Unchanged(String),
    /// Still not building after the repair: the files are as they were and the chat says why.
    Failed,
}

/// The application's name the first answer begins with in bold (`**Helsinki Traffic Alerts
/// Map**`), and the prose with the markers removed; no name when the answer does not begin
/// with one of 3 to 60 characters.
pub(crate) fn title_of(prose: &str) -> (Option<String>, String) {
    let trimmed = prose.trim_start();
    if let Some(rest) = trimmed.strip_prefix("**") {
        if let Some(end) = rest.find("**") {
            let name = rest[..end].trim();
            let length = name.chars().count();
            if (3..=60).contains(&length) && !name.contains('\n') {
                return (Some(name.to_owned()), format!("{name}{}", &rest[end + 2..]));
            }
        }
    }
    (None, prose.to_owned())
}

/// The refusals of an answer with its unreadable blocks added, as the step's log shows them.
fn with_unread(refused: &[patch::Refused], unread: usize) -> Vec<patch::Refused> {
    let mut all = refused.to_vec();
    all.extend((0..unread).map(|_| patch::Refused {
        path: String::new(),
        reason: "no path line before <<<<<<< SEARCH, or no >>>>>>> REPLACE; not applied".to_owned(),
    }));
    all
}

/// What a repair call is told about blocks it could not read.
fn unread_problem(unread: usize) -> String {
    format!(
        "{unread} block(s) of the answer could not be read and were not applied: every block \
         needs its file path on the line right before <<<<<<< SEARCH, then =======, then \
         >>>>>>> REPLACE. Send those changes again as complete blocks"
    )
}

/// The first line of a commit: the first sentence of what the model said, short enough for a
/// log line. The whole of it is the commit body.
pub(crate) fn commit_subject(prose: &str) -> String {
    const MAX: usize = 100;
    let first = prose
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let sentence = first
        .char_indices()
        .find(|(i, c)| {
            matches!(c, '.' | '!' | '?')
                && first[i + c.len_utf8()..].starts_with(char::is_whitespace)
        })
        .map_or(first, |(i, c)| &first[..i + c.len_utf8()]);
    if sentence.chars().count() <= MAX {
        return sentence.to_owned();
    }
    let cut: String = sentence.chars().take(MAX - 1).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', ';', ':']))
}

/// The subject, then the whole prose when it says more.
fn commit_message(prose: &str) -> String {
    let subject = commit_subject(prose);
    let prose = prose.trim();
    if prose.is_empty() || prose == subject {
        return if subject.is_empty() {
            "Update the application".to_owned()
        } else {
            subject
        };
    }
    format!("{subject}\n\n{prose}")
}

/// `prose` trimmed, or `fallback` when the model wrote none.
fn or_else(prose: String, fallback: &str) -> String {
    match prose.trim() {
        "" => fallback.to_owned(),
        text => text.to_owned(),
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

/// The abstract base class of every NGSI-LD model (`ngsi-ld-core`): a JSON Schema carries it as a
/// definition, but no entity is ever of this type.
const ABSTRACT_TYPES: [&str; 1] = ["Entity"];

/// `types` less the abstract base class and, when the endpoint's schema index is known and
/// lists any type, less every type the endpoint does not serve (EP-46, AP-44).
/// The `jc-types.ts` of several models as one file: each exported name once, the first model's
/// declaration winning, and the first file's preamble.
fn merge_declarations(files: &[String]) -> String {
    static EXPORT: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^export\s+(?:declare\s+)?(?:interface|type|enum|const|class|function)\s+([A-Za-z_$][\w$]*)")
            .expect("valid regex")
    });
    let Some((first, rest)) = files.split_first() else {
        return String::new();
    };
    if rest.is_empty() {
        return first.clone();
    }
    let mut seen: Vec<String> = Vec::new();
    let mut merged = String::new();
    for (n, file) in files.iter().enumerate() {
        // A chunk is a top-level `export` and the lines up to the next one.
        let mut chunks: Vec<String> = vec![String::new()];
        for line in file.lines() {
            if line.starts_with("export ") {
                chunks.push(String::new());
            }
            let chunk = chunks.last_mut().expect("one chunk");
            chunk.push_str(line);
            chunk.push('\n');
        }
        for (i, chunk) in chunks.into_iter().enumerate() {
            if i == 0 {
                if n == 0 {
                    merged.push_str(&chunk);
                }
                continue;
            }
            let name = EXPORT
                .captures(&chunk)
                .map(|c| c[1].to_owned())
                .unwrap_or_else(|| chunk.clone());
            if !seen.contains(&name) {
                seen.push(name);
                merged.push_str(&chunk);
            }
        }
    }
    merged
}

fn served_only(types: Vec<String>, index: Option<&Value>) -> Vec<String> {
    let served: Vec<&str> = index
        .and_then(|index| index.get("models"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| model.get("types").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    types
        .into_iter()
        .filter(|name| !ABSTRACT_TYPES.contains(&name.as_str()))
        .filter(|name| served.is_empty() || served.contains(&name.as_str()))
        .collect()
}

/// `data_needs` with each need's `types` passed through [`served_only`]; a need whose types
/// all go is dropped.
fn needs_served(data_needs: &Value, index: Option<&Value>) -> Value {
    let needs = data_needs
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|need| {
            let types: Vec<String> = need
                .get("types")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            let kept = served_only(types, index);
            if kept.is_empty() {
                return None;
            }
            let mut need = need.clone();
            need["types"] = json!(kept);
            Some(need)
        });
    Value::Array(needs.collect())
}

/// The prompt section that teaches each tool, by heading, and the operation behind the tool.
const TOOL_SECTIONS: [(&str, &str); 5] = [
    (
        "## WHEN THE PERSON ASKS TO SHARE OR PUBLISH DATA",
        "jc_endpoint_propose",
    ),
    (
        "## WHEN THE PERSON ASKS TO CHANGE AN ENDPOINT",
        "jc_endpoint_propose",
    ),
    ("## WHEN THE PERSON ASKS FOR AN INDICATOR", "jc_kpi_compute"),
    (
        "## WHEN THE PERSON ASKS TO KEEP AN INDICATOR UPDATED",
        "jc_pipeline_propose",
    ),
    (
        "## WHEN THE PERSON ASKS TO COMPLETE A CONTEXT SPACE",
        "jc_space_complete",
    ),
];

/// The text without the section that starts at `heading`, up to the next `## ` heading.
fn without_section(text: &str, heading: &str) -> String {
    let Some(start) = text.find(heading) else {
        return text.to_owned();
    };
    let body = start + heading.len();
    let end = text[body..]
        .find("\n## ")
        .map_or(text.len(), |offset| body + offset + 1);
    format!("{}{}", &text[..start], &text[end..])
}

pub(crate) fn space_complete_tool_call(answer: &str) -> Option<Value> {
    for fence in share::TOOL_FENCE.captures_iter(answer) {
        let Ok(value) = serde_json::from_str::<Value>(&fence[1]) else {
            continue;
        };
        if value.get("tool").and_then(Value::as_str) == Some("space_complete") {
            return Some(value);
        }
    }
    None
}

pub(crate) fn navigate_tool_call(answer: &str) -> Option<String> {
    for fence in share::TOOL_FENCE.captures_iter(answer) {
        let Ok(value) = serde_json::from_str::<Value>(&fence[1]) else {
            continue;
        };
        if value.get("tool").and_then(Value::as_str) == Some("navigate") {
            return value
                .get("route")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
    }
    None
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
    fn the_types_of_several_models_are_one_file_with_each_name_once() {
        let bikes = "// generated\nimport type { Row } from \"@joinedcontext/sdk\";\nexport interface Station extends Row {\n  type: \"Station\";\n}\nexport type Status = \"open\" | \"closed\";\n";
        let kpis = "// generated\nimport type { Row } from \"@joinedcontext/sdk\";\nexport interface KeyPerformanceIndicator extends Row {\n  type: \"KeyPerformanceIndicator\";\n}\nexport type Status = \"draft\";\n";
        let merged = merge_declarations(&[bikes.to_owned(), kpis.to_owned()]);
        assert_eq!(merged.matches("import type").count(), 1, "{merged}");
        assert_eq!(merged.matches("export type Status").count(), 1, "{merged}");
        assert!(merged.contains("\"open\" | \"closed\""), "{merged}");
        assert!(
            merged.contains("export interface KeyPerformanceIndicator"),
            "{merged}"
        );
        assert_eq!(merge_declarations(&[bikes.to_owned()]), bikes);
    }

    #[test]
    fn the_first_answer_names_the_application_in_bold() {
        let (title, prose) =
            title_of("**Helsinki Traffic Alerts Map** shows every alert on a map.");
        assert_eq!(title.as_deref(), Some("Helsinki Traffic Alerts Map"));
        assert_eq!(
            prose,
            "Helsinki Traffic Alerts Map shows every alert on a map."
        );
        assert_eq!(title_of("A map of alerts.").0, None);
        assert_eq!(title_of("**ab** too short").0, None);
    }

    #[test]
    fn a_commit_subject_is_the_first_sentence_and_short() {
        assert_eq!(
            commit_subject("The Overview has more KPIs. It also shows alerts.\n\nMore."),
            "The Overview has more KPIs."
        );
        assert_eq!(
            commit_subject("Version 2.5 of the map"),
            "Version 2.5 of the map"
        );
        let long = "This application provides an interactive map and dashboard for Helsinki city bike docking stations, showing real-time bike availability and free docking slots";
        let subject = commit_subject(long);
        assert!(subject.chars().count() <= 100, "{subject}");
        assert!(subject.ends_with('…'), "{subject}");
        assert_eq!(commit_message("Done."), "Done.");
        assert_eq!(commit_message(""), "Update the application");
        assert_eq!(commit_message("One. Two."), "One.\n\nOne. Two.");
    }

    #[test]
    fn a_credit_refusal_gives_its_figure_and_never_its_links() {
        let body = r#"{"error":{"message":"This request requires more credits, or fewer max_tokens. You requested up to 64000 tokens, but can only afford 58556. To increase, visit https://openrouter.ai/workspaces/default/keys/e0cd and adjust the key's total limit","code":402}}"#;
        assert_eq!(affordable_tokens(body), Some(58556));
        assert!(!provider_said(body).contains("openrouter.ai"));
        let said = CallError::Credit {
            affordable: affordable_tokens(body),
        }
        .said(64000);
        assert!(said.contains("58556") && said.contains("64000"), "{said}");
        assert!(!said.contains("http"), "{said}");
        assert_eq!(affordable_tokens("Payment Required"), None);
        assert_eq!(
            provider_said("plain <b>body</b> at http://x.test/a"),
            "plain <b>body</b> at (link removed)"
        );
    }

    #[test]
    fn a_type_the_endpoint_does_not_serve_and_the_abstract_base_class_are_left_out() {
        let index = json!({ "models": [
            { "name": "bikes", "version": 1, "types": ["BikeHireDockingStation"] },
            { "name": "weather", "version": 2, "types": ["WeatherObserved"] }
        ] });
        let named = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();

        assert_eq!(
            served_only(
                named(&[
                    "BikeHireDockingStation",
                    "Entity",
                    "Alert",
                    "WeatherObserved"
                ]),
                Some(&index)
            ),
            named(&["BikeHireDockingStation", "WeatherObserved"])
        );
        // Without an index, only the abstract base class goes.
        assert_eq!(
            served_only(named(&["BikeHireDockingStation", "Entity"]), None),
            named(&["BikeHireDockingStation"])
        );
        // An index that lists no type narrows nothing more.
        assert_eq!(
            served_only(named(&["Alert", "Entity"]), Some(&json!({ "models": [] }))),
            named(&["Alert"])
        );

        let needs = json!([
            { "types": ["BikeHireDockingStation", "Entity"], "attrs": ["name"] },
            { "types": ["Entity"], "attrs": ["id"] }
        ]);
        assert_eq!(
            needs_served(&needs, Some(&index)),
            json!([{ "types": ["BikeHireDockingStation"], "attrs": ["name"] }])
        );
    }

    #[test]
    fn without_section_drops_one_heading_up_to_the_next() {
        let text = "## A\none\n## B\ntwo\n```json\n{}\n```\n## C\nthree\n";
        assert_eq!(without_section(text, "## B"), "## A\none\n## C\nthree\n");
        assert_eq!(
            without_section(text, "## C"),
            "## A\none\n## B\ntwo\n```json\n{}\n```\n"
        );
        assert_eq!(without_section(text, "## D"), text);
    }

    #[test]
    fn navigate_tool_call_extracts_route() {
        let text = "I will open the Assistant.\n\n```json\n{\"tool\":\"navigate\",\"route\":\"/projects/helsinki/assistant\"}\n```\n";
        assert_eq!(
            navigate_tool_call(text),
            Some("/projects/helsinki/assistant".to_owned())
        );
        assert_eq!(navigate_tool_call("plain response"), None);
    }

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

    #[tokio::test]
    async fn pack_contains_every_view_kind_from_capabilities() {
        let caps: Value = serde_json::from_str(KIT_CAPABILITIES).expect("kit.json is valid JSON");
        let views = caps["capabilities"]["views"]
            .as_object()
            .expect("views is an object");
        assert!(!views.is_empty(), "views must not be empty");

        let state = AppState::new(crate::config::Config::for_tests(), None);
        let driver = Driver {
            state,
            http: reqwest::Client::new(),
            run_id: "test-run".into(),
            project: "test-proj".into(),
            prompt: "Test prompt".into(),
            data_needs: json!([]),
            endpoint_slug: "test-slug".into(),
            endpoints: Vec::new(),
            allows_write: false,
            bearer: "test-bearer".into(),
            proxy_base: "http://localhost:8080".into(),
            model: "test-model".into(),
            provider: "anthropic".into(),
            ttl: Duration::from_secs(60),
            passes: AtomicU32::new(0),
            schema_index: OnceLock::new(),
            branch: "agent/test".into(),
            path_prefix: "apps/test/".into(),
            created_by: "test-user".into(),
            identity: Identity {
                subject: "sub-test-user".into(),
                username: "test-user".into(),
                email: None,
                name: None,
                roles: vec![],
                groups: vec![],
            },
            access: Access::default(),
            kind: "application".into(),
            unattended: false,
            continues: None,
        };
        let pack = driver
            .pack(&json!({}), &BTreeMap::new(), &[], "instruction", None, None)
            .await;
        for kind in views.keys() {
            assert!(
                pack.contains(kind),
                "pack missing view kind '{kind}' from kit.json"
            );
        }
    }
}

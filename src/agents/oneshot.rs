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
use crate::agents::kit;
use crate::agents::patch;
use crate::agents::preview;
use crate::agents::profile::Profile;
use crate::agents::run::{AgentRun, AgentRunEvent, AgentRunStatus};
use crate::agents::store::now_rfc3339;
use crate::agents::{fields, kpi, share};
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

    /// A code run (Architecture/20 §4.1): the template on screen at once, one call writes the
    /// application over it, what does not build goes back once, and every message after the
    /// first run is one more pass over the same files.
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
        let template = files.clone();
        // What the run branch holds, so every commit carries what changed since the last one.
        let mut committed = BTreeMap::new();
        let mut versioned = false;
        self.publish_code(
            &files,
            "The template is on screen; writing the application for your request.",
            None,
            false,
        )
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
        // SDK-14: the first run sends one problem back, a runtime error of its preview included.
        // `first_run` holds while its preview may still report one; `may_repair` says whether
        // that report gets the repair or brings the template back.
        let (mut first_run, mut may_repair) = (false, false);
        match self
            .code_pass(&samples, &mut files, &conversation, &self.prompt)
            .await?
        {
            CodePass::Built { prose, repaired } => {
                self.publish_code(&files, &prose, Some(&mut committed), true)
                    .await?;
                versioned = true;
                conversation.push((self.prompt.clone(), prose));
                (first_run, may_repair) = (true, !repaired);
            }
            CodePass::Unchanged(prose) => {
                self.thought(&prose).await?;
                conversation.push((self.prompt.clone(), prose));
            }
            CodePass::Failed => {}
        }
        self.status(AgentRunStatus::Testing).await?;
        self.status(AgentRunStatus::Previewing).await?;
        if self.unattended {
            self.status(AgentRunStatus::AwaitingApproval).await?;
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
                // ponytail: a report names no preview version, so one posted by a frame the run
                // has replaced counts too; a version in the report is the upgrade.
                "preview_error" if first_run => {
                    let error = preview_error_line(&event.payload);
                    let mut failure = error.clone();
                    if std::mem::take(&mut may_repair) {
                        self.thought(&format!(
                            "The preview reported an error; asking for a repair:\n{error}"
                        ))
                        .await?;
                        let before = files.clone();
                        let (prose, errors) = self
                            .code_step(
                                &samples,
                                &mut files,
                                &[],
                                &self.prompt,
                                Some(std::slice::from_ref(&error)),
                            )
                            .await?;
                        if errors.is_empty() && files != before {
                            let prose = or_else(prose, "The error is repaired.");
                            self.publish_code(&files, &prose, Some(&mut committed), false)
                                .await?;
                            continue;
                        }
                        if !errors.is_empty() {
                            failure = errors.join("\n");
                        }
                    }
                    first_run = false;
                    files = template.clone();
                    self.publish_code(
                        &files,
                        &format!(
                            "The application did not recover, so the frame shows the template \
                             again:\n{failure}"
                        ),
                        Some(&mut committed),
                        false,
                    )
                    .await?;
                }
                "message" if sent_by_person(&event) => {
                    first_run = false;
                    let text = event
                        .payload
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    self.thought("Working on your message…").await?;
                    match self
                        .code_pass(&samples, &mut files, &conversation, &text)
                        .await
                    {
                        Ok(CodePass::Built { prose, .. }) => {
                            self.publish_code(&files, &prose, Some(&mut committed), !versioned)
                                .await?;
                            versioned = true;
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

    /// One pass of a code run: a call, its blocks applied, the project checked, and one repair
    /// call when it does not build (SDK-13, SDK-14). A pass that still does not build leaves the
    /// files as they were and says why.
    async fn code_pass(
        &self,
        samples: &Value,
        files: &mut BTreeMap<String, String>,
        conversation: &[(String, String)],
        instruction: &str,
    ) -> Result<CodePass, String> {
        let before = files.clone();
        let (mut prose, mut errors) = self
            .code_step(samples, files, conversation, instruction, None)
            .await?;
        let repaired = !errors.is_empty();
        if repaired {
            self.thought(&format!(
                "The application does not build; asking for a repair:\n{}",
                errors.join("\n")
            ))
            .await?;
            (prose, errors) = self
                .code_step(samples, files, conversation, instruction, Some(&errors))
                .await?;
        }
        if !errors.is_empty() {
            *files = before;
            self.thought(&format!(
                "The application still does not build, so the preview keeps the version before \
                 this request:\n{}",
                errors.join("\n")
            ))
            .await?;
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
            repaired,
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
        errors: Option<&[String]>,
    ) -> Result<(String, Vec<String>), String> {
        let user = self
            .code_pack(samples, files, conversation, instruction, errors)
            .await;
        let answer = self
            .complete_within(&code::SYSTEM, &user, CODE_OUTPUT_BUDGET)
            .await?;
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
        errors: Option<&[String]>,
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
        match errors {
            Some(errors) => {
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
            None if conversation.is_empty() && instruction == self.prompt => pack.push_str(
                "Write the whole application for the request above, with its tests, as \
                 SEARCH/REPLACE blocks over the files of the project.\n",
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
        // ponytail: the first model publishing a needed type; an endpoint over two models whose
        // types the application both needs gets the other model's types as the placeholder's.
        let model = models
            .iter()
            .find(|model| {
                model["types"].as_array().is_some_and(|types| {
                    types
                        .iter()
                        .filter_map(Value::as_str)
                        .any(|t| needed.iter().any(|n| n == t))
                })
            })
            .or(models.first())
            .ok_or("the endpoint publishes no model")?;
        let major = model["version"]
            .as_u64()
            .ok_or("the endpoint's model has no version")?;
        let source = self
            .read_text(&format!(
                "{}/v1/data/schema/v{major}/model.linkml.yaml",
                self.proxy_base
            ))
            .await?;
        crate::tools::model_tools::typescript(&self.state, &source).await
    }

    /// Stores the files, says `prose`, commits what changed since `committed` when given, and
    /// points the frame at the new version (SDK-15, SDK-17).
    async fn publish_code(
        &self,
        files: &BTreeMap<String, String>,
        prose: &str,
        committed: Option<&mut BTreeMap<String, String>>,
        first_version: bool,
    ) -> Result<(), String> {
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
        self.event("preview", json!({ "previewUrl": url })).await
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
        let user = self.conversation_pack(conversation, text, catalog.as_ref());
        let answer = self
            .complete_with_system(CONVERSATION_SYSTEM, &user)
            .await?;

        if let Some(call) = share::tool_call(&answer) {
            return self.share(call, &answer).await;
        }
        if let Some(call) = share::edit_call(&answer) {
            return self.edit_endpoint(call, &answer).await;
        }
        if let Some(call) = kpi::tool_call(&answer) {
            return self.kpi(call, &answer).await;
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
  "type": "<the entity type>",
  "attribute": "<the attribute folded; empty for count>",
  "agg": "avg | sum | count | min | max",
  "unit": "<a UN/CEFACT common code when the value has a unit, e.g. GQ for µg/m³, C62 for a count>",
  "q": "<an NGSI-LD filter narrowing the entities, or omit it>"
}}
```

The platform reads the entities through the endpoint, computes the value, renders the
`KeyPerformanceIndicator` entity with its formula and provenance, and shows it to the person.

## WHEN THE PERSON ASKS TO COMPLETE A CONTEXT SPACE

A request to complete, draft or fill out a context space's drafts. Answer with one or two plain sentences and then ONE fenced JSON block:

```json
{{
  "tool": "space_complete",
  "space": "<context space name>"
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
            return self.kpi(call, &answer).await.map(Some);
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

    /// The endpoint's schema index, read through the proxy with the run's ticket (EP-46).
    async fn schema_index(&self) -> Result<Value, String> {
        self.read_entities(&format!("{}/v1/data/schema/index.json", self.proxy_base))
            .await
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
    ) -> Result<String, String> {
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
        let params = match call {
            Ok(params) => params,
            Err(reason) => {
                self.event("tool", failed(reason.clone())).await?;
                let prose = format!("The indicator request could not be read: {reason}");
                self.thought(&prose).await?;
                return Ok(prose);
            }
        };
        let input = serde_json::to_value(&params).unwrap_or(Value::Null);
        if let Err(reason) = self.granted("jc_kpi_compute") {
            return self.refused("compute_kpi", started, input, reason).await;
        }
        let mut url = format!(
            "{}/v1/data/ngsi-ld/v1/entities?type={}&options=keyValues&limit=1000",
            self.proxy_base,
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
                self.thought(&prose).await?;
                return Ok(prose);
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
            self.thought(&prose).await?;
            return Ok(prose);
        };
        // The source endpoint, by its slug, for the provenance; the indicator space's
        // endpoint, when the project has one, for the card's write.
        let source = self
            .state
            .mirror
            .list(
                &self.project,
                "Endpoint",
                &crate::store::ListOptions::default(),
            )
            .items
            .into_iter()
            .find(|env| env.spec["slug"].as_str() == Some(self.endpoint_slug.as_str()));
        let (endpoint_name, endpoint_space) = match &source {
            Some(env) => (
                env.metadata.name.clone(),
                crate::api::assistant::ref_name(&env.spec["contextSpaceRef"])
                    .unwrap_or_else(|| self.project.clone()),
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
                return Ok(prose);
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
        Ok(prose)
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
enum CodePass {
    /// The files changed and build; `repaired` when a problem went back first.
    Built { prose: String, repaired: bool },
    /// The answer changed no file: a turn of the conversation, not a version.
    Unchanged(String),
    /// Still not building after the repair: the files are as they were and the chat says why.
    Failed,
}

/// `prose` trimmed, or `fallback` when the model wrote none.
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

fn or_else(prose: String, fallback: &str) -> String {
    match prose.trim() {
        "" => fallback.to_owned(),
        text => text.to_owned(),
    }
}

/// A `preview_error` event as the line the model gets back: `file:line: message`.
fn preview_error_line(payload: &Value) -> String {
    let message = payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("the preview reported an error");
    match (
        payload.get("file").and_then(Value::as_str),
        payload.get("line").and_then(Value::as_u64),
    ) {
        (Some(file), Some(line)) => format!("{file}:{line}: {message}"),
        (Some(file), None) => format!("{file}: {message}"),
        _ => message.to_owned(),
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
const TOOL_SECTIONS: [(&str, &str); 4] = [
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

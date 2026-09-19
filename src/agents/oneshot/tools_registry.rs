//! Every registered operation as a tool of the conversation (AG-64, ADR-N-021).
//!
//! The assistant calls the platform the way every other client does: one operation, one Verdict,
//! one Change. The list it sees is the registry filtered twice — by the profile's access block and
//! by the person's own grants — so a tool a viewer may not run is not offered and is refused if
//! asked for anyway. The hand-written tools beside these stay for the shapes the registry has no
//! operation for; the model reaches for a `jc_` tool first because it is the one that changes
//! anything.

use super::*;

use crate::ops::OperationSummary;

/// One `jc_` call of an answer.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RegistryCall {
    pub name: String,
    pub arguments: Value,
}

/// An operation that opens a Change by itself, and what the conversation does instead (AG-77: the
/// assistant opens the kind's form, or the deletion's confirmation, prefilled, and never proposes
/// on its own). The profile names these for the runs that do propose — a workspace run brings its
/// copy back — so they are taken away here, where the person is watching and one click away, and
/// refused if the model asks for one anyway.
pub(super) fn opens_a_change(name: &str) -> Option<&'static str> {
    match name {
        "jc_resource_delete" | "jc_project_delete" => Some(
            "open the removal with change_resource and `delete: true`; the person types the name \
             there and proposes it",
        ),
        _ if name.ends_with("_propose") => Some(
            "draft it with change_resource — `create: true` for a resource that does not exist \
             yet, a `patch` for one that does — and the kind's form opens filled for the person to \
             propose",
        ),
        _ => None,
    }
}

/// The operations this run may call: the profile's half and the person's half, both checked the
/// way [`Access::check`] checks them at call time, so the list never offers what the call refuses,
/// and without the ones that would propose in the person's place.
pub(super) fn offered(
    access: &Access,
    identity: &Identity,
    state: &AppState,
    project: &str,
) -> Vec<OperationSummary> {
    let caller = crate::ops::Caller {
        identity: identity.clone(),
        via: crate::ops::Via::Agent,
        access: None,
    };
    crate::ops::listing(&caller, state, project)
        .into_iter()
        .filter(|op| crate::ops::find(&op.name).is_some_and(|operation| access.names(operation)))
        .filter(|op| opens_a_change(&op.name).is_none())
        .collect()
}

/// The prompt section: what the platform can do, and how to ask for the detail of one.
pub(super) fn section(ops: &[OperationSummary]) -> String {
    if ops.is_empty() {
        return String::new();
    }
    let listed: String = ops
        .iter()
        .map(|op| {
            format!(
                "- `{}` — {}{}\n",
                op.name,
                op.description.trim(),
                if acts(&op.name) {
                    " (changes something: a draft, a Verdict or a Change)"
                } else {
                    ""
                }
            )
        })
        .collect();
    format!(
        r#"## THE PLATFORM'S OPERATIONS

Everything the platform does, it does through one of these operations. They are the same
operations the Portal's own pages and every MCP client call, so what you do here is what a
person sees there.

{listed}
Ask for one operation's input schema before you call it the first time:

```json
{{ "tool": "describe_tool", "name": "<operation name>" }}
```

Call it with its arguments:

```json
{{ "tool": "<operation name>", "arguments": {{ }} }}
```

Two more tools are the conversation itself. Open the page you are talking about, so the person
sees what you mean — the page is one of `spaces`, `space`, `models`, `model`, `endpoints`,
`endpoint`, `policies`, `shared`, `draft`, with the name of the one to open:

```json
{{ "tool": "jc_ui_navigate", "arguments": {{ "page": "space", "name": "helsinki" }} }}
```

To show the data itself, open `entities` with the endpoint and the entity type, and a `q` when the
person asked for a subset — the grid opens already narrowed, so nobody filters by hand what the
question already said:

```json
{{ "tool": "jc_ui_navigate", "arguments": {{ "page": "entities", "endpoint": "bikes-public", "type": "BikeHireDockingStation", "q": "availableBikeNumber==0" }} }}
```

And ask, rather than guess, whenever a choice is the person's — which space, which base model,
which unit. When more than one resource fits the ask, ask; never take the first. To choose among
the project's resources, name the kind in `pick` (`endpoints`, `spaces`, `datamodels`,
`pipelines`, `datasources`, `policies`, `projects`): the platform lists what the person may read,
and `options` then only narrows that list by name. `multiple` takes several answers, with an
optional `min` and `max`. Otherwise give the options you would take; the panel draws them as
buttons and one click answers. Ask one question at a time and wait for the answer:

```json
{{ "tool": "jc_ask", "arguments": {{ "question": "Which endpoints should the app read?", "pick": "endpoints", "multiple": true, "min": 1 }} }}
```

```json
{{ "tool": "jc_ask", "arguments": {{ "question": "Which unit?", "options": ["µg/m³", "ppm"], "default": "µg/m³" }} }}
```

A call that is refused answers with the reason; correct it and call again. Nothing here writes
context data or approves anything on the person's behalf: an operation that changes the
configuration leaves a draft or a Change for the person to approve.
"#
    )
}

/// Every `jc_` call of an answer, in order.
pub(super) fn calls(answer: &str) -> Vec<RegistryCall> {
    blocks(answer)
        .filter_map(|value| {
            let name = value.get("tool").and_then(Value::as_str)?;
            if !name.starts_with("jc_") {
                return None;
            }
            Some(RegistryCall {
                name: name.to_owned(),
                arguments: value.get("arguments").cloned().unwrap_or(json!({})),
            })
        })
        .collect()
}

/// The names of every `describe_tool` call of an answer.
pub(super) fn describes(answer: &str) -> Vec<String> {
    blocks(answer)
        .filter(|value| value.get("tool").and_then(Value::as_str) == Some("describe_tool"))
        .filter_map(|value| value.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

fn blocks(answer: &str) -> impl Iterator<Item = Value> + '_ {
    share::TOOL_FENCE
        .captures_iter(answer)
        .filter_map(|fence| serde_json::from_str::<Value>(&fence[1]).ok())
}

/// Whether a `jc_` tool changes something, which is what makes a call the data wrote refusable
/// (AG-20). The lane says it: green reads, yellow leaves a draft or a Change, red is the
/// dangerous one. An operation nobody registered is treated as acting: unknown is not safe.
pub(super) fn acts(name: &str) -> bool {
    crate::ops::find(name).is_none_or(|op| op.lane != crate::change::Lane::Green)
}

impl Driver {
    /// One registry call, as the person who started the run (AG-70). The answer is what goes back
    /// to the model; the `tool` event is what the person sees (AG-56).
    pub(super) async fn registry_call(&self, call: &RegistryCall) -> Result<String, String> {
        let started = std::time::Instant::now();
        // A proposal the person never read must not reach the approval queue (AG-77): the
        // conversation drafts, the form opens, the person proposes.
        if let Some(instead) = opens_a_change(&call.name) {
            let reason = format!(
                "'{}' opens a Change of its own, which the assistant never does: {instead}",
                call.name
            );
            self.event(
                "tool",
                failed_step(&call.name, started, &call.arguments, &reason),
            )
            .await?;
            return Ok(format!("error: {reason}"));
        }
        if let Err(reason) =
            self.access
                .check(&call.name, &self.identity, &self.state, &self.project)
        {
            self.event(
                "tool",
                failed_step(&call.name, started, &call.arguments, &reason),
            )
            .await?;
            return Ok(format!("error: {reason}"));
        }
        let op = match crate::ops::find(&call.name) {
            Some(op) => op,
            None => {
                let reason = format!("operation '{}' is not registered", call.name);
                self.event(
                    "tool",
                    failed_step(&call.name, started, &call.arguments, &reason),
                )
                .await?;
                return Ok(format!("error: {reason}"));
            }
        };
        let caller = crate::ops::Caller {
            identity: self.identity.clone(),
            via: crate::ops::Via::Agent,
            access: None,
        };
        match crate::ops::call(
            op,
            &caller,
            &self.state,
            &self.project,
            call.arguments.clone(),
        )
        .await
        {
            Ok(output) => {
                self.event(
                    "tool",
                    json!({
                        "tool": call.name,
                        "status": "ok",
                        "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                        "input": call.arguments,
                        "output": output,
                    }),
                )
                .await?;
                Ok(serde_json::to_string(&output).unwrap_or_else(|_| "{}".to_owned()))
            }
            Err(err) => {
                let reason = err.to_string();
                self.event(
                    "tool",
                    failed_step(&call.name, started, &call.arguments, &reason),
                )
                .await?;
                Ok(format!("error: {reason}"))
            }
        }
    }

    /// The input schema of one operation, or why there is none to show.
    pub(super) fn describe(&self, name: &str, offered: &[OperationSummary]) -> String {
        match offered.iter().find(|op| op.name == name) {
            Some(op) => serde_json::to_string_pretty(&json!({
                "name": op.name,
                "title": op.title,
                "description": op.description,
                "input": op.input_schema,
                "output": op.output_schema,
            }))
            .unwrap_or_else(|_| "{}".to_owned()),
            None => format!(
                "error: '{name}' is not an operation you may call; the ones you may call are \
                 listed above"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// The two tools of the conversation itself (UI-57, UI-59, AG-80)
// ---------------------------------------------------------------------------

/// The pages the assistant may open, and what each one needs (UI-59). The route is built here,
/// so a page the enum does not name cannot be reached however the model spells it.
const PAGES: [(&str, &str); 19] = [
    ("spaces", "/projects/{project}/spaces"),
    ("space", "/projects/{project}/spaces/{name}"),
    ("models", "/projects/{project}/models"),
    ("model", "/projects/{project}/models?name={name}"),
    ("endpoints", "/projects/{project}/endpoints"),
    ("endpoint", "/projects/{project}/endpoints?name={name}"),
    ("policies", "/projects/{project}/policies"),
    ("shared", "/projects/{project}/shared"),
    ("draft", "/projects/{project}/{plural}?draft={name}"),
    // The rest of what `ui/src/router.tsx` serves (T-1011, T-1012): the assistant opens the
    // page a person would, so every route a person reaches by clicking is one it can name.
    ("activity", "/projects/{project}/activity"),
    ("approvals", "/projects/{project}/approvals"),
    ("approval", "/projects/{project}/approvals/{name}"),
    ("explore", "/projects/{project}/explore"),
    // One entity of the explorer, opened on its detail (T-1017): "show me this one" opens the
    // row already selected instead of the list the person then searches by hand.
    ("entity", "/projects/{project}/explore?entityId={name}"),
    // One endpoint's grid, already narrowed (UI-64, UI-67): a person who asked a question wants
    // the rows that answer it, not the list to filter by hand. `q` is the only optional part —
    // without it the grid opens on the whole type.
    (
        "entities",
        "/projects/{project}/explore?endpoint={endpoint}&type={type}&q={q}",
    ),
    ("ckan", "/projects/{project}/ckan"),
    ("assistant", "/projects/{project}/assistant"),
    ("app", "/projects/{project}/apps/{name}"),
    // A resource of any kind, the way `model` and `endpoint` open one of theirs.
    ("resource", "/projects/{project}/{plural}?name={name}"),
];

/// One `jc_ui_navigate` call.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct NavigateCall {
    pub page: String,
    pub name: Option<String>,
    pub plural: Option<String>,
    /// The endpoint whose data the `entities` grid shows.
    pub endpoint: Option<String>,
    /// The entity type of that grid, and the NGSI-LD filter it opens narrowed by.
    pub entity_type: Option<String>,
    pub q: Option<String>,
}

/// One option of a question: the value the answer carries and what the person reads.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct AskOption {
    pub value: String,
    pub title: String,
    pub description: Option<String>,
}

/// One `jc_ask` call: the question, its options and the answer taken when nobody chooses.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct AskCall {
    pub question: String,
    pub options: Vec<AskOption>,
    /// A string, or for `multiple` an array of strings; settled against the options.
    pub default: Option<Value>,
    /// The kind whose resources the platform offers (AG-83), as `pick` names it.
    pub pick: Option<&'static str>,
    pub multiple: bool,
    pub min: Option<usize>,
    pub max: Option<usize>,
}

/// The kinds a question may offer by name (AG-83): `pick` as the model spells it, and the kind.
const PICKS: [(&str, &str); 7] = [
    ("endpoints", "Endpoint"),
    ("spaces", "ContextSpace"),
    ("datamodels", "DataModel"),
    ("pipelines", "Pipeline"),
    ("datasources", "DataSource"),
    ("policies", "Policy"),
    ("projects", "Project"),
];

pub(super) fn navigate_call(answer: &str) -> Option<Result<NavigateCall, String>> {
    let value = blocks(answer)
        .find(|value| value.get("tool").and_then(Value::as_str) == Some("jc_ui_navigate"))?;
    let arguments = value.get("arguments").cloned().unwrap_or(value.clone());
    let Some(page) = arguments.get("page").and_then(Value::as_str) else {
        return Some(Err(format!(
            "jc_ui_navigate names no page; the pages are {}",
            PAGES.map(|(name, _)| name).join(", ")
        )));
    };
    if !PAGES.iter().any(|(name, _)| *name == page) {
        return Some(Err(format!(
            "'{page}' is not a page of the Portal; the pages are {}",
            PAGES.map(|(name, _)| name).join(", ")
        )));
    }
    let text = |key: &str| {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    Some(Ok(NavigateCall {
        page: page.to_owned(),
        name: text("name"),
        plural: text("plural"),
        endpoint: text("endpoint"),
        entity_type: text("type"),
        q: text("q"),
    }))
}

/// The route a call opens, or the reason it opens none (UI-59). Every placeholder the template
/// names has to be filled, so a route cannot reach a page with a hole in it; `q` alone is
/// optional, because a question without a filter is the whole grid.
fn route_of(project: &str, call: &NavigateCall) -> Result<String, String> {
    let template = PAGES
        .iter()
        .find(|(name, _)| *name == call.page)
        .map(|(_, template)| *template)
        .ok_or_else(|| format!("'{}' is not a page of the Portal", call.page))?;
    let filled = [
        ("project", Some(project)),
        ("name", call.name.as_deref()),
        ("plural", call.plural.as_deref().or(Some("models"))),
        ("endpoint", call.endpoint.as_deref()),
        ("type", call.entity_type.as_deref()),
        ("q", call.q.as_deref()),
    ];
    let mut route = template.to_owned();
    for (placeholder, value) in filled {
        let hole = format!("{{{placeholder}}}");
        if !route.contains(&hole) {
            continue;
        }
        let value = value.unwrap_or_default().trim();
        if value.is_empty() && placeholder != "q" {
            let what = match placeholder {
                "name" => "the name of what to open",
                "endpoint" => "the endpoint whose data to open",
                "type" => "the entity type to show",
                other => other,
            };
            return Err(format!("the page '{}' needs {what}", call.page));
        }
        route = route.replace(&hole, &urlencoding(value));
    }
    Ok(without_empty_pairs(&route))
}

/// Drops the query pairs nothing filled: an optional placeholder left `q=` behind, and an empty
/// filter is not the request the person asking meant.
fn without_empty_pairs(route: &str) -> String {
    let Some((path, query)) = route.split_once('?') else {
        return route.to_owned();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| !pair.ends_with('='))
        .collect();
    if kept.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{}", kept.join("&"))
    }
}

pub(super) fn ask_call(answer: &str) -> Option<Result<AskCall, String>> {
    let value =
        blocks(answer).find(|value| value.get("tool").and_then(Value::as_str) == Some("jc_ask"))?;
    let arguments = value.get("arguments").cloned().unwrap_or(value.clone());
    Some(parse_ask(&arguments))
}

fn parse_ask(arguments: &Value) -> Result<AskCall, String> {
    let question = arguments
        .get("question")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    if question.is_empty() {
        return Err("jc_ask asks nothing: give it a question".to_owned());
    }
    let pick = match arguments.get("pick").and_then(Value::as_str) {
        None => None,
        Some(name) => Some(
            PICKS
                .iter()
                .find(|(pick, _)| *pick == name)
                .map(|(pick, _)| *pick)
                .ok_or_else(|| {
                    format!(
                        "'{name}' is not a kind to pick from; pick one of {}",
                        PICKS.map(|(pick, _)| pick).join(", ")
                    )
                })?,
        ),
    };
    let count = |key: &str| {
        arguments
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
    };
    let (min, max) = (count("min"), count("max"));
    if let (Some(min), Some(max)) = (min, max) {
        if max < min {
            return Err(format!("jc_ask asks for at least {min} and at most {max}"));
        }
    }
    let mut options: Vec<AskOption> = Vec::new();
    for option in arguments
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let text = |key: &str| {
            option
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
        };
        let value = option
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .or_else(|| text("value"))
            .or_else(|| text("const"))
            .or_else(|| text("name"))
            .or_else(|| text("title"));
        let Some(value) = value else { continue };
        if options.iter().any(|known| known.value == value) {
            continue;
        }
        options.push(AskOption {
            value: value.to_owned(),
            title: text("title").unwrap_or(value).to_owned(),
            description: text("description").map(str::to_owned),
        });
    }
    Ok(AskCall {
        question,
        options,
        default: arguments.get("default").cloned(),
        pick,
        multiple: arguments.get("multiple").and_then(Value::as_bool) == Some(true),
        min,
        max,
    })
}

/// The kind a `pick` names.
fn picked_kind(pick: &str) -> &'static str {
    PICKS
        .iter()
        .find(|(name, _)| *name == pick)
        .map_or("", |(_, kind)| *kind)
}

/// The options final, the default is settled against them: one of them for one answer, a set of
/// them (at least `min`) for several, the model's own text only for a question with no options.
fn settle(mut call: AskCall) -> Result<AskCall, String> {
    let values: Vec<&str> = call.options.iter().map(|o| o.value.as_str()).collect();
    if call.multiple {
        if values.is_empty() {
            return Err("a question with several answers needs options or a pick".to_owned());
        }
        let min = call.min.unwrap_or(0);
        if values.len() < min {
            return Err(format!(
                "the question asks for at least {min} answers and offers {}",
                values.len()
            ));
        }
        let given: Vec<String> = match &call.default {
            Some(Value::String(one)) => vec![one.clone()],
            Some(Value::Array(many)) => many
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        };
        let mut chosen: Vec<String> = Vec::new();
        for value in given.iter().map(String::as_str) {
            if values.contains(&value) && !chosen.iter().any(|c| c == value) {
                chosen.push(value.to_owned());
            }
        }
        for value in &values {
            if min <= chosen.len() {
                break;
            }
            if !chosen.iter().any(|c| c == value) {
                chosen.push((*value).to_owned());
            }
        }
        chosen.truncate(call.max.unwrap_or(usize::MAX).max(min));
        call.default = Some(json!(chosen));
    } else {
        let given = call
            .default
            .as_ref()
            .and_then(Value::as_str)
            .map(str::to_owned);
        call.default = match given {
            Some(one) if values.is_empty() || values.contains(&one.as_str()) => Some(json!(one)),
            _ => values.first().map(|first| json!(first)),
        };
    }
    Ok(call)
}

/// The schema the panel renders: options are a `oneOf` of `{const, title, description}`, several
/// answers an array of them, no options a text box (UI-57, UI-73).
fn question_schema(call: &AskCall) -> Value {
    let choices: Vec<Value> = call
        .options
        .iter()
        .map(|option| {
            let mut one = json!({ "const": option.value, "title": option.title });
            if let Some(description) = &option.description {
                one["description"] = json!(description);
            }
            one
        })
        .collect();
    let mut answer = if call.multiple {
        let mut many = json!({
            "type": "array",
            "title": call.question,
            "items": { "type": "string", "oneOf": choices },
            "uniqueItems": true,
        });
        if let Some(min) = call.min {
            many["minItems"] = json!(min);
        }
        if let Some(max) = call.max {
            many["maxItems"] = json!(max);
        }
        many
    } else {
        let mut one = json!({ "type": "string", "title": call.question });
        if !choices.is_empty() {
            one["oneOf"] = json!(choices);
        }
        one
    };
    if let Some(default) = call.default.as_ref() {
        answer["default"] = default.clone();
    }
    json!({
        "type": "object",
        "title": call.question,
        "properties": { "answer": answer },
        "required": ["answer"],
    })
}

/// One line about a resource a question offers: where it lives and, for an endpoint, what it
/// serves and to whom (AG-83).
fn describe_option(state: &AppState, project: &str, kind: &str, item: &Value) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(space) = item.get("space").and_then(Value::as_str) {
        parts.push(format!("space {space}"));
    }
    if kind == "Endpoint" {
        let name = item.get("name").and_then(Value::as_str)?;
        if let Some(endpoint) = state.mirror.get(project, kind, name) {
            let representations: Vec<&str> = endpoint.spec["enabledRepresentations"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if !representations.is_empty() {
                parts.push(representations.join(", "));
            }
            if let Some(audience) = endpoint.spec["audience"].as_str() {
                parts.push(audience.to_owned());
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// A title as a listing row carries it: a string, or a text per language.
fn title_of(item: &Value) -> Option<String> {
    match item.get("title")? {
        Value::String(title) if !title.trim().is_empty() => Some(title.clone()),
        Value::Object(texts) => texts
            .get("en")
            .or_else(|| texts.values().next())
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

impl Driver {
    /// Opens a page for the person (UI-59). The route is this table's, never the model's text.
    pub(super) async fn open_page(&self, call: &NavigateCall) -> Result<String, String> {
        let started = std::time::Instant::now();
        let route = match route_of(&self.project, call) {
            Ok(route) => route,
            Err(reason) => {
                self.event(
                    "tool",
                    failed_step(
                        "jc_ui_navigate",
                        started,
                        &json!({ "page": call.page }),
                        &reason,
                    ),
                )
                .await?;
                return Ok(format!("error: {reason}"));
            }
        };
        self.event(
            "tool",
            json!({
                "tool": "jc_ui_navigate",
                "status": "ok",
                "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "input": {
                    "page": call.page, "name": call.name, "plural": call.plural,
                    "endpoint": call.endpoint, "type": call.entity_type, "q": call.q,
                },
                "output": { "route": route },
            }),
        )
        .await?;
        self.event("navigate", json!({ "route": route })).await?;
        Ok(format!("opened {route}"))
    }

    /// The options of a `pick` question, from what the person may read (AG-83): the same
    /// listing the registry answers them with, narrowed by the names the model gave, never
    /// widened by them. `Err` is the refusal the model reads.
    pub(super) async fn fill_options(&self, mut call: AskCall) -> Result<AskCall, String> {
        if let Some(pick) = call.pick {
            let kind = picked_kind(pick);
            let op = crate::ops::find("jc_resource_list")
                .ok_or_else(|| "the resource listing is not registered".to_owned())?;
            let caller = crate::ops::Caller {
                identity: self.identity.clone(),
                via: crate::ops::Via::Agent,
                access: None,
            };
            let listed = crate::ops::call(
                op,
                &caller,
                &self.state,
                &self.project,
                json!({ "kind": kind }),
            )
            .await
            .map_err(|err| err.to_string())?;
            let named: Vec<String> = call.options.iter().map(|o| o.value.clone()).collect();
            call.options = listed["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| {
                    let name = item.get("name").and_then(Value::as_str)?;
                    if !named.is_empty() && !named.iter().any(|n| n == name) {
                        return None;
                    }
                    Some(AskOption {
                        value: name.to_owned(),
                        title: title_of(item).unwrap_or_else(|| name.to_owned()),
                        description: describe_option(&self.state, &self.project, kind, item),
                    })
                })
                .collect();
            if call.options.is_empty() {
                return Err(if named.is_empty() {
                    format!(
                        "there are no {pick} in project '{}' to pick from",
                        self.project
                    )
                } else {
                    format!("none of the {pick} named is one the person may pick")
                });
            }
        }
        settle(call)
    }

    /// Asks the person one question and leaves the turn (AG-80): the answer arrives as an
    /// `answer` event, which starts the next turn with what they chose.
    pub(super) async fn ask_person(&self, call: &AskCall) -> Result<String, String> {
        let id = format!("q-{}", crate::agents::store::now_rfc3339().replace(':', ""));
        let options: Vec<Value> = call
            .options
            .iter()
            .map(|o| json!({ "value": o.value, "title": o.title, "description": o.description }))
            .collect();
        self.event(
            "question",
            json!({
                "questionId": id,
                "schema": question_schema(call),
                "default": call.default,
                "options": options,
                "pick": call.pick,
                "multiple": call.multiple,
                "min": call.min,
                "max": call.max,
            }),
        )
        .await?;
        self.thought(&call.question).await?;
        Ok(call.question.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_outside_the_enum_is_refused_with_the_pages_there_are() {
        let answer = "```json\n{ \"tool\": \"jc_ui_navigate\", \"arguments\": { \"page\": \"/etc/passwd\" } }\n```";
        let err = navigate_call(answer)
            .expect("a call")
            .expect_err("a page nobody named");
        assert!(err.contains("spaces"), "{err}");
    }

    #[test]
    fn a_page_of_the_enum_carries_its_name() {
        let answer = "```json\n{ \"tool\": \"jc_ui_navigate\", \"arguments\": { \"page\": \"space\", \"name\": \"helsinki\" } }\n```";
        assert_eq!(
            navigate_call(answer).expect("a call").expect("a page"),
            NavigateCall {
                page: "space".to_owned(),
                name: Some("helsinki".to_owned()),
                plural: None,
                endpoint: None,
                entity_type: None,
                q: None,
            }
        );
    }

    #[test]
    fn seven_options_survive_and_the_default_is_the_first() {
        let answer = r#"```json
{ "tool": "jc_ask", "arguments": { "question": "Which space?", "options": ["a","b","c","d","e","f","g"] } }
```"#;
        let call = settle(ask_call(answer).expect("a call").expect("a question")).expect("settled");
        assert_eq!(call.options.len(), 7);
        assert_eq!(call.default, Some(json!("a")));
        let schema = question_schema(&call);
        assert_eq!(
            schema["properties"]["answer"]["oneOf"][6]["const"],
            json!("g")
        );
        assert_eq!(schema["required"], json!(["answer"]));
    }

    #[test]
    fn an_option_keeps_its_title_and_description_and_unicode() {
        let call = parse_ask(&json!({ "question": "Unit?", "options": [
            { "value": "ugm3", "title": "µg/m³", "description": "Mikrogramm je Kubikmeter" }] }))
        .expect("a question");
        let schema = question_schema(&settle(call).expect("settled"));
        let one = &schema["properties"]["answer"]["oneOf"][0];
        assert_eq!(one["const"], "ugm3");
        assert_eq!(one["title"], "µg/m³");
        assert_eq!(one["description"], "Mikrogramm je Kubikmeter");
    }

    #[test]
    fn several_answers_are_an_array_whose_default_is_a_subset() {
        let call = parse_ask(&json!({ "question": "Which?", "options": ["a", "b", "c"],
            "multiple": true, "min": 1, "max": 2, "default": ["c", "zzz"] }))
        .expect("a question");
        let call = settle(call).expect("settled");
        assert_eq!(call.default, Some(json!(["c"])));
        let answer = &question_schema(&call)["properties"]["answer"];
        assert_eq!(answer["type"], "array");
        assert_eq!(answer["minItems"], 1);
        assert_eq!(answer["maxItems"], 2);
        assert_eq!(answer["uniqueItems"], true);
        assert_eq!(answer["items"]["oneOf"][1]["const"], "b");
    }

    #[test]
    fn a_minimum_the_options_cannot_meet_and_an_unknown_pick_are_refused() {
        let call = parse_ask(
            &json!({ "question": "Which?", "options": ["a"], "multiple": true, "min": 2 }),
        )
        .expect("parsed");
        assert!(settle(call).is_err());
        let err =
            parse_ask(&json!({ "question": "Which?", "pick": "secrets" })).expect_err("refused");
        assert!(err.contains("endpoints"), "{err}");
        assert!(parse_ask(&json!({ "question": "Which?", "min": 3, "max": 1 })).is_err());
    }

    #[test]
    fn a_question_without_options_is_a_text_box() {
        let answer = "```json\n{ \"tool\": \"jc_ask\", \"arguments\": { \"question\": \"What should it be called?\" } }\n```";
        let call = ask_call(answer).expect("a call").expect("a question");
        assert!(call.options.is_empty());
        assert!(question_schema(&call)["properties"]["answer"]["oneOf"].is_null());
    }

    #[test]
    fn a_question_with_no_text_is_refused() {
        let answer =
            "```json\n{ \"tool\": \"jc_ask\", \"arguments\": { \"question\": \"  \" } }\n```";
        assert!(ask_call(answer).expect("a call").is_err());
    }

    #[test]
    fn a_jc_block_is_a_call_and_anything_else_is_not() {
        let answer = r#"Sure.

```json
{ "tool": "jc_catalog_search", "arguments": { "q": "bikes" } }
```

```json
{ "tool": "query_endpoint", "endpoint": "e", "name": "n", "arguments": {} }
```
"#;
        assert_eq!(
            calls(answer),
            vec![RegistryCall {
                name: "jc_catalog_search".to_owned(),
                arguments: json!({ "q": "bikes" }),
            }]
        );
    }

    #[test]
    fn a_call_without_arguments_is_an_empty_object() {
        let answer = "```json\n{ \"tool\": \"jc_draft_list\" }\n```";
        assert_eq!(calls(answer)[0].arguments, json!({}));
    }

    #[test]
    fn describe_names_the_operation_it_asks_about() {
        let answer = "```json\n{ \"tool\": \"describe_tool\", \"name\": \"jc_kpi_compute\" }\n```";
        assert_eq!(describes(answer), vec!["jc_kpi_compute".to_owned()]);
    }

    /// AG-77: the assistant opens the kind's form prefilled and never proposes on its own. The
    /// whole registry is walked, so an operation added or renamed later is classified here too
    /// rather than quietly reaching the approval queue from the dock.
    #[test]
    fn every_operation_that_opens_a_change_is_kept_from_the_conversation() {
        let opening: Vec<&str> = crate::ops::registry()
            .iter()
            .map(|op| op.name)
            .filter(|name| opens_a_change(name).is_some())
            .collect();
        assert_eq!(
            opening,
            vec![
                "jc_endpoint_propose",
                "jc_datasource_propose",
                "jc_pipeline_propose",
                "jc_space_propose",
                "jc_model_propose",
                "jc_resource_propose",
                "jc_resource_delete",
                "jc_project_delete",
                "jc_workspace_propose",
            ],
            "the operations the conversation may not call, in registry order"
        );
        // What the conversation does need: it drafts, checks and tests, and it reads.
        for kept in [
            "jc_draft_put",
            "jc_manifest_dry_run",
            "jc_datasource_check",
            "jc_pipeline_test",
            "jc_kpi_compute",
            "jc_space_complete",
            "jc_catalog_search",
            "jc_resource_get",
            "jc_change_list",
            "jc_workspace_open",
        ] {
            assert!(
                opens_a_change(kept).is_none(),
                "{kept} is how the conversation works and stays offered"
            );
        }
        // And the way out is named, so the refusal tells the model what to do instead.
        assert!(opens_a_change("jc_space_propose")
            .expect("named")
            .contains("change_resource"));
    }

    #[test]
    fn a_read_only_operation_does_not_act_and_an_unknown_one_does() {
        assert!(!acts("jc_catalog_search"));
        assert!(acts("jc_endpoint_propose"));
        assert!(acts("jc_not_an_operation"));
    }

    #[test]
    fn the_section_says_which_operations_change_something() {
        let ops = crate::ops::registry();
        let summaries: Vec<OperationSummary> = ops
            .iter()
            .filter(|op| op.name == "jc_catalog_search" || op.name == "jc_endpoint_propose")
            .map(|op| OperationSummary {
                name: op.name.to_string(),
                title: op.title.to_string(),
                description: op.description.to_string(),
                input_schema: (op.input)(),
                output_schema: (op.output)(),
                annotations: op.annotations,
                lane: op.lane,
            })
            .collect();
        let text = section(&summaries);
        assert!(text.contains("`jc_catalog_search`"), "{text}");
        assert!(
            text.contains("`jc_endpoint_propose` —")
                && text.contains("(changes something: a draft, a Verdict or a Change)"),
            "{text}"
        );
    }

    #[test]
    fn no_operation_is_no_section() {
        assert_eq!(section(&[]), "");
    }

    /// T-1011, T-1012: the assistant opens the page a person would, so every route
    /// `ui/src/router.tsx` serves is one it can name. A page the router has and the table does
    /// not is a page the person has to find by hand while the assistant says it cannot.
    #[test]
    fn every_page_a_person_reaches_is_one_the_assistant_can_name() {
        for page in [
            "activity",
            "approvals",
            "approval",
            "explore",
            "entity",
            "ckan",
            "assistant",
            "app",
            "resource",
            "entities",
        ] {
            assert!(
                PAGES.iter().any(|(name, _)| *name == page),
                "{page} is a route of the Portal and not a page of the table"
            );
        }
    }

    /// Every template names only the placeholders the call can fill, so a route cannot be built
    /// with a hole in it.
    #[test]
    fn a_page_template_names_only_the_placeholders_a_call_fills() {
        for (page, template) in PAGES {
            let mut rest = template;
            while let Some(start) = rest.find('{') {
                let end = rest[start..]
                    .find('}')
                    .map(|offset| start + offset)
                    .unwrap_or_else(|| panic!("{page}: unclosed placeholder in {template}"));
                let placeholder = &rest[start + 1..end];
                assert!(
                    matches!(
                        placeholder,
                        "project" | "name" | "plural" | "endpoint" | "type" | "q"
                    ),
                    "{page}: {template} names {placeholder}, which no call fills"
                );
                rest = &rest[end + 1..];
            }
        }
    }

    /// A page that takes a name is refused without one rather than opening the list instead.
    #[test]
    fn the_new_pages_carry_what_they_name() {
        let answer = "```json\n{ \"tool\": \"jc_ui_navigate\", \"arguments\": { \"page\": \"approval\", \"name\": \"chg-0000beef\" } }\n```";
        let call = navigate_call(answer).expect("a call").expect("a page");
        assert_eq!(call.page, "approval");
        assert_eq!(call.name.as_deref(), Some("chg-0000beef"));
    }

    /// The grid opens on the filter the question carried, and the filter reaches the route encoded
    /// (UI-64, UI-67): `==` and the space of a two-word value are not query syntax of their own.
    #[test]
    fn the_entities_page_opens_the_grid_narrowed_by_the_question() {
        let answer = r#"```json
{ "tool": "jc_ui_navigate", "arguments": { "page": "entities", "endpoint": "bikes-public",
  "type": "BikeHireDockingStation", "q": "availableBikeNumber==0;name~=\"Rautatientori\"" } }
```"#;
        let call = navigate_call(answer).expect("a call").expect("a page");
        assert_eq!(call.endpoint.as_deref(), Some("bikes-public"));
        assert_eq!(call.entity_type.as_deref(), Some("BikeHireDockingStation"));
        assert_eq!(
            route_of("helsinki", &call).expect("a route"),
            "/projects/helsinki/explore?endpoint=bikes-public&type=BikeHireDockingStation\
             &q=availableBikeNumber%3D%3D0%3Bname~%3D%22Rautatientori%22"
        );
    }

    /// Without a filter the grid opens on the whole type, and the route carries no empty pair:
    /// `q=` is a filter that matches nothing, not the absence of one.
    #[test]
    fn a_grid_without_a_filter_carries_no_empty_query_pair() {
        let call = NavigateCall {
            page: "entities".to_owned(),
            name: None,
            plural: None,
            endpoint: Some("air-public".to_owned()),
            entity_type: Some("AirQualityObserved".to_owned()),
            q: None,
        };
        assert_eq!(
            route_of("helsinki", &call).expect("a route"),
            "/projects/helsinki/explore?endpoint=air-public&type=AirQualityObserved"
        );
    }

    /// A grid without an endpoint or without a type is refused: the explorer would open on
    /// nothing and the person would read the assistant's sentence as if it had.
    #[test]
    fn a_grid_missing_its_endpoint_or_its_type_is_refused() {
        for (endpoint, entity_type, needed) in [
            (None, Some("AirQualityObserved"), "the endpoint"),
            (Some("air-public"), None, "the entity type"),
            (Some("air-public"), Some("   "), "the entity type"),
        ] {
            let call = NavigateCall {
                page: "entities".to_owned(),
                name: None,
                plural: None,
                endpoint: endpoint.map(str::to_owned),
                entity_type: entity_type.map(str::to_owned),
                q: Some("availableBikeNumber==0".to_owned()),
            };
            let err = route_of("helsinki", &call).expect_err("no route");
            assert!(err.contains(needed), "{err}");
        }
    }
}

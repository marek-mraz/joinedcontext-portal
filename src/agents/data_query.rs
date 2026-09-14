//! The assistant works the data itself (AG-75, AG-76): the data-plane MCP tools of the
//! endpoints a conversation carries, called through `jc-agent-proxy` so the gateway applies the
//! person's grants.
//!
//! The model sees each endpoint's own `tools/list`, write tools removed, and the other endpoints
//! of the project the person may open. It answers with one or several `query_endpoint` blocks;
//! the Portal opens a named endpoint the conversation does not read yet, runs the calls at once
//! and gives every result back, until the model answers in prose, drafts something, or the calls
//! of one message run out.

use serde_json::{json, Value};

use crate::agents::endpoints::RunEndpoint;
use crate::agents::share;

/// Calls of the façade one message may make before the model must answer (AG-76).
pub const MAX_CALLS: usize = 12;

/// Drafts of a KPI pipeline, or reads of an indicator, one message may try (AG-76).
pub const MAX_DRAFTS: usize = 3;

/// How much of one result the model reads back: a page of entities, not the whole space.
const MAX_RESULT_CHARS: usize = 12_000;

/// An endpoint of the project the conversation does not read yet and the person may open.
#[derive(Debug, Clone, PartialEq)]
pub struct Openable {
    pub name: String,
    pub title: Option<String>,
    pub space: String,
}

/// One call the model asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryCall {
    pub endpoint: String,
    pub name: String,
    pub arguments: Value,
}

/// Every `query_endpoint` block of an answer, in order; an `Err` names what one is missing.
pub fn tool_calls(answer: &str) -> Vec<Result<QueryCall, String>> {
    let mut calls = Vec::new();
    for fence in share::TOOL_FENCE.captures_iter(answer) {
        let Ok(value) = serde_json::from_str::<Value>(&fence[1]) else {
            continue;
        };
        if value.get("tool").and_then(Value::as_str) == Some("query_endpoint") {
            calls.push(call_of(&value));
        }
    }
    calls
}

/// The words of every `search_catalog` call of an answer: the model searches the catalog itself,
/// with its own words, as often as it needs (AG-58, AG-76).
pub fn search_calls(answer: &str) -> Vec<String> {
    share::TOOL_FENCE
        .captures_iter(answer)
        .filter_map(|fence| serde_json::from_str::<Value>(&fence[1]).ok())
        .filter(|value| value.get("tool").and_then(Value::as_str) == Some("search_catalog"))
        .filter_map(|value| {
            value
                .get("q")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|q| !q.is_empty())
                .map(str::to_owned)
        })
        .collect()
}

/// Why a tool the endpoint does not offer was refused, with what it does offer, so the model
/// calls one of those next rather than giving up (AG-75).
pub fn not_offered(tools: &[Value], name: &str, endpoint: &str) -> String {
    let offered: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect();
    if offered.is_empty() {
        format!("'{name}' is not a read tool endpoint '{endpoint}' offers: it offers you no read tool, so read another endpoint")
    } else {
        format!(
            "'{name}' is not a read tool endpoint '{endpoint}' offers; it offers: {}",
            offered.join(", ")
        )
    }
}

fn call_of(value: &Value) -> Result<QueryCall, String> {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let endpoint = text("endpoint").ok_or("a query_endpoint call names no endpoint")?;
    let name = text("name").ok_or("a query_endpoint call names no tool")?;
    let arguments = match value.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(object @ Value::Object(_)) => object.clone(),
        Some(_) => return Err("arguments must be an object".to_owned()),
    };
    Ok(QueryCall {
        endpoint,
        name,
        arguments,
    })
}

/// The JSON-RPC body of a `tools/call`.
pub fn rpc(call: &QueryCall) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": call.name, "arguments": call.arguments },
    })
}

/// The tools of one `tools/list` answer the assistant may call: those annotated read-only.
pub fn read_only_tools(list: &Value) -> Vec<Value> {
    list.pointer("/result/tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|tool| {
            tool.pointer("/annotations/readOnlyHint")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .map(|tool| {
            json!({
                "name": tool.get("name").cloned().unwrap_or(Value::Null),
                "description": tool.get("description").cloned().unwrap_or(Value::Null),
                "inputSchema": tool.get("inputSchema").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

/// Whether `name` is one of the tools offered for an endpoint.
pub fn offers(tools: &[Value], name: &str) -> bool {
    tools
        .iter()
        .any(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
}

/// What the model reads of a `tools/call` answer: the structured result or the text, capped.
pub fn result_text(answer: &Value) -> String {
    if let Some(error) = answer.get("error") {
        return format!("error: {error}");
    }
    let result = answer.get("result").cloned().unwrap_or(Value::Null);
    let text = if let Some(structured) = result.get("structuredContent") {
        structured.to_string()
    } else {
        result
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let flagged = if result.get("isError").and_then(Value::as_bool) == Some(true) {
        "error: "
    } else {
        ""
    };
    let mut capped: String = text.chars().take(MAX_RESULT_CHARS).collect();
    if text.chars().count() > MAX_RESULT_CHARS {
        capped.push_str("\n… (cut; ask for fewer attributes or a smaller limit)");
    }
    format!("{flagged}{capped}")
}

/// The read tools every endpoint's façade offers, for a conversation that has not opened one yet.
fn facade_tools() -> Value {
    json!([
        { "name": "list_types", "arguments": {} },
        { "name": "describe_schema", "arguments": { "type": "<entity type>" } },
        { "name": "list_attributes", "arguments": { "type": "<entity type>" } },
        { "name": "query_entities", "arguments": { "type": "<entity type>", "q": "<NGSI-LD filter>", "attrs": ["<attribute>"], "limit": 20 } },
        { "name": "get_entity", "arguments": { "id": "<entity id>" } }
    ])
}

/// The prompt section: the endpoints the conversation reads, each with the tools it offers, the
/// endpoints the person may open, and the call's shape.
pub fn section(endpoints: &[RunEndpoint], tools: &[Vec<Value>], openable: &[Openable]) -> String {
    let listed: Vec<Value> = endpoints
        .iter()
        .zip(tools)
        .map(|(endpoint, tools)| {
            json!({ "endpoint": endpoint.name, "contextSpace": endpoint.space, "tools": tools })
        })
        .collect();
    let others: Vec<Value> = openable
        .iter()
        .map(|o| json!({ "endpoint": o.name, "title": o.title, "contextSpace": o.space }))
        .collect();
    format!(
        r#"## WORKING WITH THE DATA

You work the data yourself, as an agent. A question about what the data says (how many, which,
where, the latest, one entity, the attributes of a type), and any indicator or pipeline you
draft, is grounded in the data, never in memory or an endpoint's title. Look before you draft:
list the types, describe the schema, read a page of entities.

Find the data yourself too. Search the project's catalog with your own words, as often as you
need:

```json
{{ "tool": "search_catalog", "q": "<words>" }}
```

The search matches words in names, titles and descriptions. When it finds nothing, search again
before you answer that nothing is there: synonyms, singular and plural, and the words of the
data's own language (a Slovak city names bikes cyklo or bicykel, a Finnish one pyörä); then
look into the endpoints themselves, their types and a page of entities. A failed call answers
with the reason: correct the call and make it again.

Call tools with fenced JSON blocks and nothing else in that answer. Several blocks in one answer
run at once, on one endpoint or several, so ask for everything you need together:

```json
{{
  "tool": "query_endpoint",
  "endpoint": "<an endpoint name from either list below>",
  "name": "<a read tool name>",
  "arguments": {{ "type": "<entity type>", "q": "<NGSI-LD filter>", "attrs": ["<attribute>"], "limit": 20 }}
}}
```

with arguments as the tool's inputSchema says. The platform calls them with the person's own
access and gives you every result; call again when you need more (at most {MAX_CALLS} calls for
one message), then answer in plain prose naming the entities you used, or draft what was asked.

The endpoints this conversation reads, with their tools:

```json
{}
```

Endpoints of the project the person may open: name one in a call and the platform adds it to
the conversation first. Their tools are the same read tools:

```json
{}
```

Read tools an endpoint may offer; each endpoint above lists the ones its policy grants the person,
and only those run: {}
"#,
        serde_json::to_string_pretty(&listed).unwrap_or_default(),
        serde_json::to_string_pretty(&others).unwrap_or_default(),
        facade_tools()
    )
}

/// The calls made for this message and what they answered, for the next model call.
pub fn results_section(results: &[(QueryCall, String)]) -> String {
    if results.is_empty() {
        return String::new();
    }
    let mut section = String::from("\n## WHAT YOUR CALLS ANSWERED\n\n");
    for (i, (call, text)) in results.iter().enumerate() {
        section.push_str(&format!(
            "### Call {} — {} on {} with {}\n```\n{}\n```\n\n",
            i + 1,
            call.name,
            call.endpoint,
            call.arguments,
            text
        ));
    }
    if results.len() >= MAX_CALLS {
        section.push_str("No calls are left for this message: answer now in plain prose.\n");
    } else {
        section.push_str("Call again if you need more, or answer in plain prose.\n");
    }
    section
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_searches_the_catalog_with_its_own_words_and_a_refusal_names_what_is_offered() {
        let answer = "Nothing under bikes; trying the city's words.\n```json\n{\"tool\":\"search_catalog\",\"q\":\"cyklo bicykel\"}\n```\n```json\n{\"tool\":\"search_catalog\",\"q\":\"  \"}\n```";
        assert_eq!(search_calls(answer), vec!["cyklo bicykel".to_owned()]);
        assert!(search_calls("```json\n{\"tool\":\"query_endpoint\"}\n```").is_empty());

        let tools = vec![
            json!({ "name": "query_entities" }),
            json!({ "name": "describe_schema" }),
        ];
        assert_eq!(
            not_offered(&tools, "list_types", "public-air"),
            "'list_types' is not a read tool endpoint 'public-air' offers; it offers: query_entities, describe_schema"
        );
        assert!(not_offered(&[], "list_types", "public-air").contains("offers you no read tool"));
    }

    #[test]
    fn every_call_of_an_answer_is_read_out_of_its_fences_with_its_arguments() {
        let answer = "Let me look.\n```json\n{\"tool\":\"query_endpoint\",\"endpoint\":\"helsinki-all\",\"name\":\"query_entities\",\"arguments\":{\"type\":\"BikeHireDockingStation\",\"q\":\"availableBikeNumber==0\"}}\n```\n```json\n{\"tool\":\"query_endpoint\",\"endpoint\":\"helsinki-kpi\",\"name\":\"list_types\"}\n```";
        let calls = tool_calls(answer);
        assert_eq!(calls.len(), 2);
        let first = calls[0].clone().expect("well formed");
        assert_eq!(first.endpoint, "helsinki-all");
        assert_eq!(first.name, "query_entities");
        assert_eq!(first.arguments["q"], "availableBikeNumber==0");
        assert_eq!(rpc(&first)["method"], "tools/call");
        assert_eq!(calls[1].clone().expect("well formed").arguments, json!({}));

        assert!(
            tool_calls("```json\n{\"tool\":\"query_endpoint\",\"name\":\"x\"}\n```")[0].is_err()
        );
        assert!(tool_calls("plain prose").is_empty());
    }

    #[test]
    fn the_section_lists_what_the_person_may_open_beside_what_is_read() {
        let read = vec![RunEndpoint {
            name: "helsinki-bikes".into(),
            slug: "s".into(),
            space: "helsinki".into(),
        }];
        let open = vec![Openable {
            name: "helsinki-kpi".into(),
            title: Some("Helsinki indicators".into()),
            space: "helsinki-kpi".into(),
        }];
        let text = section(&read, &[vec![json!({ "name": "query_entities" })]], &open);
        assert!(text.contains("\"endpoint\": \"helsinki-bikes\""));
        assert!(text.contains("Helsinki indicators"));
        assert!(text.contains("run at once"));
    }

    #[test]
    fn only_read_only_tools_are_offered_and_results_are_capped() {
        let list = json!({ "result": { "tools": [
            { "name": "query_entities", "annotations": { "readOnlyHint": true }, "inputSchema": {} },
            { "name": "upsert_entity", "annotations": { "readOnlyHint": false }, "inputSchema": {} },
            { "name": "unannotated", "inputSchema": {} }
        ] } });
        let tools = read_only_tools(&list);
        assert!(offers(&tools, "query_entities"));
        assert!(!offers(&tools, "upsert_entity"));
        assert!(!offers(&tools, "unannotated"));

        let big = "x".repeat(MAX_RESULT_CHARS + 10);
        let text =
            result_text(&json!({ "result": { "content": [{ "type": "text", "text": big }] } }));
        assert!(text.ends_with("smaller limit)"));
        assert!(result_text(&json!({ "result": { "isError": true, "content": [{ "type": "text", "text": "denied" }] } }))
            .starts_with("error: denied"));
    }
}

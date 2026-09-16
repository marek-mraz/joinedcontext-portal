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

/// The operations this run may call: the profile's half and the person's half, both checked the
/// way [`Access::check`] checks them at call time, so the list never offers what the call refuses.
pub(super) fn offered(
    access: &Access,
    identity: &Identity,
    state: &AppState,
    project: &str,
) -> Vec<OperationSummary> {
    let caller = crate::ops::Caller {
        identity: identity.clone(),
        via: crate::ops::Via::Agent,
    };
    crate::ops::listing(&caller, state, project)
        .into_iter()
        .filter(|op| crate::ops::find(&op.name).is_some_and(|operation| access.names(operation)))
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

#[cfg(test)]
mod tests {
    use super::*;

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
}

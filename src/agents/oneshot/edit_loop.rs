//! The editing agent: every instruction after the first run is a tool loop over the run's
//! files, the check, the functions runtime and the frame's errors (SDK-20, SDK-11, SDK-12,
//! SDK-17, AP-60, AG-25). The first run stays a whole-project pass; from then on the model
//! reads what it edits, checks what it changed and finishes with a sentence for the person.

use std::time::Instant;

use super::model::{assistant_turn_message, tool_result_message, ToolCall, ToolSpec};
use super::*;
use crate::agents::{code, patch};
use crate::api::agent_runs::{invoke_function, InvokeError};

/// Lines one `read_file` returns at most: a whole file of the application, as a rule.
const READ_WINDOW: usize = 2000;
/// Bytes of the application's own files the opening turn carries whole, smallest first: an edit
/// then starts from the code instead of a crawl of `read_file` windows (T-0896).
const OPENING_BYTES: usize = 200_000;
/// What a tool result carries back to the model at most.
const RESULT_CAP: usize = 8 * 1024;
/// What a tool event keeps of a result at most.
const EVENT_CAP: usize = 2 * 1024;
/// How many `preview_error` events one call returns at most.
const ERROR_WINDOW: usize = 20;
/// Output budget of one editing call: an edit is small, the answer is a tool call or a sentence.
const EDIT_OUTPUT_BUDGET: u32 = 8000;

/// The rules of the loop, as the model reads them (SDK-11, SDK-12, SDK-20).
const EDIT_SYSTEM: &str =
    "You edit a small web application that already runs. You work with tools only: \
list_files, read_file, edit_file, write_file, delete_file, check, call_function, preview_errors \
and finish.\n\
Read a file before you edit it, and edit with an exact search text. You may write only \
src/**/*.tsx, src/**/*.ts, functions/*.ts and tests/**/*.test.ts; imports come from \
@joinedcontext/sdk, react and the application's own files, nothing else. Run check after you \
change files; the preview reloads when a check passes. preview_errors tells you what the frame \
reported since the last reload; call_function runs one of the application's functions. When \
the request is done, call finish with one plain sentence for the person: what changed, in \
their words, no file names unless they asked.";

/// The tools of the loop, in the shape both providers read.
fn tools() -> Vec<ToolSpec> {
    let path = json!({ "type": "string", "description": "the file, relative to the application" });
    vec![
        ToolSpec {
            name: "list_files",
            description: "Every file of the application with its size.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "read_file",
            description: "A file whole, or the lines from..to of it (1-based); read a file whole unless it is very long.",
            input_schema: json!({ "type": "object", "properties": { "path": path, "from": { "type": "integer" }, "to": { "type": "integer" } }, "required": ["path"] }),
        },
        ToolSpec {
            name: "edit_file",
            description: "Replaces one exact occurrence of search with replace; an empty search creates or overwrites the file.",
            input_schema: json!({ "type": "object", "properties": { "path": path, "search": { "type": "string" }, "replace": { "type": "string" } }, "required": ["path", "search", "replace"] }),
        },
        ToolSpec {
            name: "write_file",
            description: "Writes a whole file.",
            input_schema: json!({ "type": "object", "properties": { "path": path, "content": { "type": "string" } }, "required": ["path", "content"] }),
        },
        ToolSpec {
            name: "delete_file",
            description: "Removes a file.",
            input_schema: json!({ "type": "object", "properties": { "path": path }, "required": ["path"] }),
        },
        ToolSpec {
            name: "check",
            description: "Builds the application: ok, or every problem, one per line.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "call_function",
            description: "Runs functions/{name}.ts of the application with a JSON body and returns its status and answer.",
            input_schema: json!({ "type": "object", "properties": { "name": { "type": "string" }, "body": {}, "query": { "type": "object" } }, "required": ["name"] }),
        },
        ToolSpec {
            name: "preview_errors",
            description: "The errors the preview frame reported since the last reload, oldest first.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "finish",
            description: "Ends the turn with one sentence for the person.",
            input_schema: json!({ "type": "object", "properties": { "message": { "type": "string" } }, "required": ["message"] }),
        },
    ]
}

/// A path the model named, made safe: relative, no `..`, no empty segment (SDK-11).
fn clean_path(raw: &str) -> Option<String> {
    let path = raw.trim();
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return None;
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return None;
    }
    Some(parts.join("/"))
}

fn cap(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… ({} more bytes)", &text[..end], text.len() - end)
}

fn arg<'a>(input: &'a Value, name: &str) -> &'a str {
    input.get(name).and_then(Value::as_str).unwrap_or_default()
}

fn list_files(files: &BTreeMap<String, String>) -> String {
    files
        .iter()
        .map(|(path, content)| format!("{path} ({} bytes)", content.len()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every file with its size, and the application's own files whole while they fit in
/// `OPENING_BYTES`, smallest first; a file left out says so, so the model reads it.
fn opening(files: &BTreeMap<String, String>) -> String {
    let mut out = String::from("Files:\n");
    out.push_str(&list_files(files));
    let mut own: Vec<(&String, &String)> = files
        .iter()
        .filter(|(path, _)| code::writable(path))
        .collect();
    own.sort_by_key(|(path, content)| (content.len(), (*path).clone()));
    let mut carried = 0usize;
    let mut left_out = Vec::new();
    for (path, content) in own {
        if carried + content.len() > OPENING_BYTES {
            left_out.push(path.as_str());
            continue;
        }
        carried += content.len();
        out.push_str(&format!("\n\n=== {path} ===\n{content}"));
    }
    if !left_out.is_empty() {
        out.push_str(&format!(
            "\n\nNot shown, read before editing: {}",
            left_out.join(", ")
        ));
    }
    out
}

fn read_file(files: &BTreeMap<String, String>, input: &Value) -> (String, bool) {
    let Some(path) = clean_path(arg(input, "path")) else {
        return ("not a path of the application".to_owned(), false);
    };
    let Some(content) = files.get(&path) else {
        return (format!("{path}: no such file"), false);
    };
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let from = input
        .get("from")
        .and_then(Value::as_u64)
        .map_or(1, |n| n.max(1) as usize)
        .min(total.max(1));
    let to = input
        .get("to")
        .and_then(Value::as_u64)
        .map_or(total, |n| n as usize)
        .clamp(from, total.max(from))
        .min(from + READ_WINDOW - 1);
    let mut out = format!("{path}: lines {from}-{to} of {total}\n");
    for (offset, line) in lines.iter().skip(from - 1).take(to + 1 - from).enumerate() {
        out.push_str(&format!("{:>4} | {line}\n", from + offset));
    }
    (out, true)
}

/// Whether one more file of `bytes` keeps the project inside SDK-11's limits.
fn fits(files: &BTreeMap<String, String>, path: &str, bytes: usize) -> Result<(), String> {
    let others: usize = files
        .iter()
        .filter(|(p, _)| p.as_str() != path)
        .map(|(_, c)| c.len())
        .sum();
    let count = files.len() + usize::from(!files.contains_key(path));
    if count > code::MAX_FILES || others + bytes > code::MAX_BYTES {
        return Err(format!(
            "the application may hold at most {} files and {} bytes together",
            code::MAX_FILES,
            code::MAX_BYTES
        ));
    }
    Ok(())
}

fn edit_file(files: &mut BTreeMap<String, String>, input: &Value) -> (String, bool) {
    let Some(path) = clean_path(arg(input, "path")) else {
        return (code::REFUSAL.to_owned(), false);
    };
    let replace = arg(input, "replace");
    if let Err(reason) = fits(files, &path, replace.len()) {
        return (reason, false);
    }
    let block = patch::Block {
        path,
        search: arg(input, "search").to_owned(),
        replace: replace.to_owned(),
    };
    let (applied, refused) = patch::apply_where(files, &[block], code::writable, code::REFUSAL);
    match (applied.first(), refused.first()) {
        (Some(done), _) => (format!("{}: {}", done.path, done.how), true),
        (None, Some(no)) => (format!("{}: {}", no.path, no.reason), false),
        (None, None) => ("nothing to apply".to_owned(), false),
    }
}

fn write_file(files: &mut BTreeMap<String, String>, input: &Value) -> (String, bool) {
    let Some(path) = clean_path(arg(input, "path")).filter(|path| code::writable(path)) else {
        return (code::REFUSAL.to_owned(), false);
    };
    let content = arg(input, "content");
    if let Err(reason) = fits(files, &path, content.len()) {
        return (reason, false);
    }
    let how = if files.contains_key(&path) {
        "replaced"
    } else {
        "created"
    };
    files.insert(path.clone(), content.to_owned());
    (format!("{path}: {how}, {} bytes", content.len()), true)
}

fn delete_file(files: &mut BTreeMap<String, String>, input: &Value) -> (String, bool) {
    let Some(path) = clean_path(arg(input, "path")).filter(|path| code::writable(path)) else {
        return (code::REFUSAL.to_owned(), false);
    };
    match files.remove(&path) {
        Some(_) => (format!("{path}: removed"), true),
        None => (format!("{path}: no such file"), false),
    }
}

impl Driver {
    /// One instruction after the first run: tool calls until `finish`, an answer without a
    /// call, the step limit or a failed model call (SDK-20). Returns the version it published,
    /// when a check passed after a change.
    pub(super) async fn edit_turn(
        &self,
        files: &mut BTreeMap<String, String>,
        committed: &mut BTreeMap<String, String>,
        conversation: &mut Vec<(String, String)>,
        instruction: &str,
        on_screen: Option<&Shown>,
    ) -> Result<Option<Shown>, String> {
        let tools = tools();
        let mut messages = vec![json!({
            "role": "user",
            "content": self.edit_user_turn(files, conversation, instruction),
        })];
        let mut shown: Option<Shown> = None;
        // Files changed since the last publish, and whether the current files passed a check.
        let mut dirty = false;
        let mut since_seq = on_screen.map_or(0, |shown| shown.seq);
        let limit = self.steps_per_run.max(1);
        let mut steps = 0u32;
        loop {
            // The profile's limit is checked before the model is asked, never after a call
            // it already made (AG-25).
            if steps >= limit {
                self.thought(&format!(
                    "The step limit of this run ({limit} tool calls a message) is reached; \
                     what passed the check is on screen."
                ))
                .await?;
                conversation.push((
                    instruction.to_owned(),
                    "The step limit was reached before the work was finished.".to_owned(),
                ));
                return Ok(shown);
            }
            let answer = self
                .complete_tools(EDIT_SYSTEM, &messages, &tools, EDIT_OUTPUT_BUDGET)
                .await
                .map_err(|err| err.said(EDIT_OUTPUT_BUDGET))?;
            messages.push(assistant_turn_message(&self.provider, &answer));
            // What the turn cost, on the run's counters (AG-44).
            if let Err(err) = self
                .state
                .agents
                .record_usage(
                    &self.run_id,
                    i64::try_from(answer.usage_tokens).unwrap_or(i64::MAX),
                    1,
                )
                .await
            {
                tracing::warn!(run = %self.run_id, error = %err, "usage not recorded");
            }
            if answer.calls.is_empty() {
                let message = answer.text.unwrap_or_default();
                return self
                    .finish_turn(
                        files,
                        committed,
                        conversation,
                        instruction,
                        &message,
                        dirty,
                        shown,
                    )
                    .await;
            }
            for call in &answer.calls {
                steps += 1;
                let started = Instant::now();
                let (result, ok) = match call.name.as_str() {
                    "list_files" => (list_files(files), true),
                    "read_file" => read_file(files, &call.input),
                    "edit_file" => {
                        let done = edit_file(files, &call.input);
                        dirty |= done.1;
                        done
                    }
                    "write_file" => {
                        let done = write_file(files, &call.input);
                        dirty |= done.1;
                        done
                    }
                    "delete_file" => {
                        let done = delete_file(files, &call.input);
                        dirty |= done.1;
                        done
                    }
                    "check" => {
                        let problems = code::problems(files);
                        if !problems.is_empty() {
                            (problems.join("\n"), false)
                        } else {
                            if dirty {
                                let next = self
                                    .publish_code(
                                        files,
                                        "The change passes the check; the preview shows it.",
                                        Some(committed),
                                        false,
                                    )
                                    .await?;
                                since_seq = next.seq;
                                shown = Some(next);
                                dirty = false;
                            }
                            ("ok".to_owned(), true)
                        }
                    }
                    "call_function" => self.call_function_tool(&call.input).await,
                    "preview_errors" => self.preview_errors_tool(since_seq).await,
                    "finish" => {
                        self.tool_event(call, "finished", true, started, steps)
                            .await?;
                        return self
                            .finish_turn(
                                files,
                                committed,
                                conversation,
                                instruction,
                                arg(&call.input, "message"),
                                dirty,
                                shown,
                            )
                            .await;
                    }
                    other => (format!("no tool named '{other}'"), false),
                };
                self.tool_event(call, &result, ok, started, steps).await?;
                messages.push(tool_result_message(
                    &self.provider,
                    &call.id,
                    &cap(&result, RESULT_CAP),
                ));
            }
        }
    }

    /// The turn's end: unpublished changes are checked and published with the message, or
    /// the message says why the preview keeps the last version.
    #[allow(clippy::too_many_arguments)]
    async fn finish_turn(
        &self,
        files: &mut BTreeMap<String, String>,
        committed: &mut BTreeMap<String, String>,
        conversation: &mut Vec<(String, String)>,
        instruction: &str,
        message: &str,
        dirty: bool,
        mut shown: Option<Shown>,
    ) -> Result<Option<Shown>, String> {
        let message = if message.trim().is_empty() {
            "Done."
        } else {
            message.trim()
        };
        if dirty {
            let problems = code::problems(files);
            if problems.is_empty() {
                shown = Some(
                    self.publish_code(files, message, Some(committed), false)
                        .await?,
                );
            } else {
                self.thought(&format!(
                    "{message}\n\nThe files do not build, so the preview keeps the last \
                     version:\n{}",
                    problems.join("\n")
                ))
                .await?;
            }
        } else {
            self.thought(message).await?;
        }
        conversation.push((instruction.to_owned(), message.to_owned()));
        Ok(shown)
    }

    /// One `tool` event per call (AG-25): the tool, its arguments, what it answered, the step.
    async fn tool_event(
        &self,
        call: &ToolCall,
        result: &str,
        ok: bool,
        started: Instant,
        step: u32,
    ) -> Result<(), String> {
        self.event(
            "tool",
            json!({
                "tool": call.name,
                "status": if ok { "ok" } else { "failed" },
                "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "input": call.input,
                "result": cap(result, EVENT_CAP),
                "step": step,
            }),
        )
        .await
    }

    /// The person's instruction with what the model needs to start: the files, the row types
    /// and the turns before (SDK-20).
    fn edit_user_turn(
        &self,
        files: &BTreeMap<String, String>,
        conversation: &[(String, String)],
        instruction: &str,
    ) -> String {
        let mut turn = String::new();
        if !conversation.is_empty() {
            turn.push_str("Earlier in this conversation:\n");
            for (asked, answered) in conversation.iter().rev().take(6).rev() {
                turn.push_str(&format!("- asked: {asked}\n  answered: {answered}\n"));
            }
            turn.push('\n');
        }
        turn.push_str(&opening(files));
        if let Some(types) = files.get(code::TYPES) {
            turn.push_str(&format!("\n\nRow types ({}):\n{types}", code::TYPES));
        }
        turn.push_str(&format!("\n\nThe request:\n{instruction}"));
        turn
    }

    /// `call_function`: one function of the run in jc-functions, as the person who started the
    /// run (AG-70, SDK-18).
    async fn call_function_tool(&self, input: &Value) -> (String, bool) {
        let name = arg(input, "name").to_owned();
        let body = input.get("body").cloned().unwrap_or(Value::Null);
        let query: BTreeMap<String, String> = input
            .get("query")
            .and_then(Value::as_object)
            .map(|object| {
                object
                    .iter()
                    .map(|(key, value)| {
                        let value = match value {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        };
                        (key.clone(), value)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let run = match self.state.agents.get_run(&self.run_id).await {
            Ok(Some(run)) => run,
            Ok(None) => return ("the run is gone".to_owned(), false),
            Err(err) => return (err.to_string(), false),
        };
        match invoke_function(&self.state, &run, &name, body, &query, &self.identity, None).await {
            Ok(invocation) => {
                let status = invocation.outcome["status"].as_u64().unwrap_or(500);
                let error = invocation
                    .outcome
                    .get("error")
                    .filter(|error| !error.is_null());
                let payload = match error {
                    Some(error) => format!("error: {error}"),
                    None => invocation
                        .outcome
                        .get("body")
                        .map(Value::to_string)
                        .unwrap_or_default(),
                };
                let logs = invocation
                    .outcome
                    .get("logs")
                    .and_then(Value::as_array)
                    .filter(|logs| !logs.is_empty())
                    .map(|logs| format!("\nlogs: {}", Value::Array(logs.clone())))
                    .unwrap_or_default();
                (
                    format!("status {status}\n{payload}{logs}"),
                    error.is_none() && status < 400,
                )
            }
            Err(InvokeError::NoFunction) => (format!("no function '{name}' in functions/"), false),
            Err(InvokeError::DoesNotBuild(problems)) => (
                format!("the functions do not build:\n{}", problems.join("\n")),
                false,
            ),
            Err(InvokeError::Unavailable(reason)) => (reason, false),
            Err(InvokeError::Refused { reason, .. }) => (reason, false),
        }
    }

    /// `preview_errors`: what the frame reported since the version on screen was published
    /// (SDK-14), oldest first.
    async fn preview_errors_tool(&self, since_seq: i64) -> (String, bool) {
        let events = match self
            .state
            .agents
            .events_since(&self.run_id, since_seq)
            .await
        {
            Ok(events) => events,
            Err(err) => return (err.to_string(), false),
        };
        let errors: Vec<String> = events
            .iter()
            .filter(|event| event.kind == "preview_error")
            .map(|event| event.payload.to_string())
            .collect();
        if errors.is_empty() {
            return ("no errors since the last reload".to_owned(), true);
        }
        let skipped = errors.len().saturating_sub(ERROR_WINDOW);
        let mut out = errors[skipped..].join("\n");
        if skipped > 0 {
            out = format!("({skipped} older errors not shown)\n{out}");
        }
        (out, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_outside_the_application_is_not_a_path() {
        assert_eq!(clean_path("src/App.tsx").as_deref(), Some("src/App.tsx"));
        assert!(clean_path("../../etc/passwd").is_none());
        assert!(clean_path("/etc/passwd").is_none());
        assert!(clean_path("src//App.tsx").is_none());
        assert!(clean_path("src/./App.tsx").is_none());
        assert!(clean_path("").is_none());
    }

    #[test]
    fn a_read_is_clamped_to_the_file_and_the_window() {
        let mut files = BTreeMap::new();
        files.insert("src/A.ts".to_owned(), "a\nb\nc\n".to_owned());
        let (text, ok) = read_file(&files, &json!({ "path": "src/A.ts", "from": 2, "to": 99 }));
        assert!(ok);
        assert!(text.starts_with("src/A.ts: lines 2-3 of 3\n"));
        assert!(text.contains("   2 | b\n   3 | c\n"));
        let (text, ok) = read_file(&files, &json!({ "path": "src/B.ts" }));
        assert!(!ok && text.ends_with("no such file"));
    }

    #[test]
    fn the_opening_turn_carries_the_small_files_whole_and_names_the_rest() {
        let mut files = BTreeMap::new();
        files.insert("package.json".to_owned(), "{}".to_owned());
        files.insert("src/App.tsx".to_owned(), "app".to_owned());
        files.insert("src/big.ts".to_owned(), "x".repeat(OPENING_BYTES));
        files.insert("functions/f.ts".to_owned(), "fn".to_owned());
        let text = opening(&files);
        assert!(text.contains("package.json (2 bytes)"));
        assert!(
            !text.contains("=== package.json ==="),
            "not the application's own"
        );
        assert!(text.contains("=== src/App.tsx ===\napp"));
        assert!(text.contains("=== functions/f.ts ===\nfn"));
        assert!(text.ends_with("Not shown, read before editing: src/big.ts"));
        assert!(opening(&BTreeMap::new()).starts_with("Files:\n"));
    }

    #[test]
    fn a_write_outside_the_sdk_paths_is_refused_and_writes_nothing() {
        let mut files = BTreeMap::new();
        for path in ["package.json", "../x.ts", "src/../../x.ts"] {
            let (text, ok) = write_file(&mut files, &json!({ "path": path, "content": "x" }));
            assert!(!ok, "{path}");
            assert_eq!(text, code::REFUSAL);
        }
        assert!(files.is_empty());
        let (_, ok) = write_file(
            &mut files,
            &json!({ "path": "src/pages/A.tsx", "content": "x" }),
        );
        assert!(ok);
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn an_edit_needs_an_exact_search() {
        let mut files = BTreeMap::new();
        // Four lines: a search that matches none is refused (a one-line file would be taken
        // as a whole-file rewrite by the patch rules).
        files.insert(
            "src/A.ts".to_owned(),
            "let a = 1;\nlet b = 2;\nlet c = 3;\nlet d = 4;\n".to_owned(),
        );
        let (text, ok) = edit_file(
            &mut files,
            &json!({ "path": "src/A.ts", "search": "let q = 7;", "replace": "let a = 3;" }),
        );
        assert!(!ok, "{text}");
        let (_, ok) = edit_file(
            &mut files,
            &json!({ "path": "src/A.ts", "search": "let a = 1;", "replace": "let a = 3;" }),
        );
        assert!(ok);
        assert!(files["src/A.ts"].starts_with("let a = 3;\nlet b = 2;"));
    }

    #[test]
    fn a_long_result_is_cut_at_the_cap() {
        let text = "é".repeat(5000);
        let cut = cap(&text, 100);
        assert!(cut.len() < 200);
        assert!(cut.contains("more bytes"));
    }
}

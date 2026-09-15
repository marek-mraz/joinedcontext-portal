//! Completions through the proxy and what the prompts carry: the schema, the samples, the rows the kit reads (AP-56, AP-57).

use super::*;

impl Driver {
    /// One call through the proxy, asked twice when the first answer carries no text: a
    /// provider answers empty now and then, and a second call is cheaper than a failed run.
    pub(super) async fn complete(&self, user: &str) -> Result<String, String> {
        self.complete_with_system(&self.narrowed(&SYSTEM), user)
            .await
    }

    pub(super) async fn complete_with_system(
        &self,
        system: &str,
        user: &str,
    ) -> Result<String, String> {
        self.complete_within(system, user, OUTPUT_BUDGET).await
    }

    /// A provider that says the key's credit covers fewer output tokens than asked is asked
    /// once more within what it covers: most answers are far shorter than the budget, and a
    /// run should not stop over a ceiling it would not have reached.
    pub(super) async fn complete_within(
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

    /// Common HTTP execution for model calls through the proxy, handling 402 credits and budget cuts.
    async fn post_llm(&self, path: &str, body: &Value, budget: u32) -> Result<Value, CallError> {
        let response = self
            .http
            .post(format!("{}{path}", self.proxy_base))
            .bearer_auth(&self.bearer)
            .json(body)
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
        Ok(answer)
    }

    /// One call through the proxy, in the body the profile's provider reads (AG-53).
    pub(super) async fn complete_once(
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
        let answer = self.post_llm(path, &body, budget).await?;
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

    /// One tool completion through the proxy with 402 retry and empty-answer retry (SDK-20).
    pub(super) async fn complete_tools(
        &self,
        system: &str,
        messages: &[Value],
        tools: &[ToolSpec],
        budget: u32,
    ) -> Result<ToolAnswer, CallError> {
        let first = match self
            .complete_tools_once(system, messages, tools, budget)
            .await
        {
            Err(CallError::Empty) => {
                self.complete_tools_once(system, messages, tools, budget)
                    .await
            }
            Err(CallError::Credit {
                affordable: Some(afford),
            }) if afford >= MIN_CREDIT_BUDGET && afford < budget => {
                self.complete_tools_once(system, messages, tools, afford - afford / 20)
                    .await
            }
            other => other,
        };
        first
    }

    async fn complete_tools_once(
        &self,
        system: &str,
        messages: &[Value],
        tools: &[ToolSpec],
        budget: u32,
    ) -> Result<ToolAnswer, CallError> {
        if self.provider == "anthropic" {
            let tools_json = tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.input_schema,
                    })
                })
                .collect::<Vec<_>>();
            let merged = merge_anthropic_messages(messages);
            let body = json!({
                "model": self.model,
                "max_tokens": budget,
                "system": system,
                "messages": merged,
                "tools": tools_json,
            });
            let answer = self.post_llm("/v1/llm/messages", &body, budget).await?;
            let mut text_parts = Vec::new();
            let mut calls = Vec::new();
            if let Some(content) = answer.get("content").and_then(Value::as_array) {
                for part in content {
                    let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
                    if part_type == "text" {
                        if let Some(t) = part.get("text").and_then(Value::as_str) {
                            text_parts.push(t);
                        }
                    } else if part_type == "tool_use" {
                        let id = part
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned();
                        let name = part
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned();
                        let input = part.get("input").cloned().unwrap_or(Value::Null);
                        calls.push(ToolCall { id, name, input });
                    }
                }
            }
            let text = if text_parts.is_empty() {
                None
            } else {
                Some(text_parts.join("\n"))
            };
            let usage_tokens = answer
                .pointer("/usage/input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                + answer
                    .pointer("/usage/output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
            if text.as_deref().unwrap_or("").trim().is_empty() && calls.is_empty() {
                return Err(CallError::Empty);
            }
            Ok(ToolAnswer {
                text,
                calls,
                usage_tokens,
            })
        } else {
            let tools_json = tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.input_schema,
                        }
                    })
                })
                .collect::<Vec<_>>();
            let mut full_messages = vec![json!({ "role": "system", "content": system })];
            full_messages.extend_from_slice(messages);
            let body = json!({
                "model": self.model,
                "max_tokens": budget,
                "messages": full_messages,
                "tools": tools_json,
            });
            let answer = self
                .post_llm("/v1/llm/chat/completions", &body, budget)
                .await?;
            let choice_msg = answer.pointer("/choices/0/message");
            let text = choice_msg
                .and_then(|m| m.get("content"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let mut calls = Vec::new();
            if let Some(tool_calls) = choice_msg
                .and_then(|m| m.get("tool_calls"))
                .and_then(Value::as_array)
            {
                for tc in tool_calls {
                    let id = tc
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let name = tc
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let input = if let Some(s) =
                        tc.pointer("/function/arguments").and_then(Value::as_str)
                    {
                        serde_json::from_str::<Value>(s).unwrap_or(Value::Null)
                    } else {
                        tc.pointer("/function/arguments")
                            .cloned()
                            .unwrap_or(Value::Null)
                    };
                    calls.push(ToolCall { id, name, input });
                }
            }
            let usage_tokens = answer
                .pointer("/usage/total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if text.as_deref().unwrap_or("").trim().is_empty() && calls.is_empty() {
                return Err(CallError::Empty);
            }
            Ok(ToolAnswer {
                text,
                calls,
                usage_tokens,
            })
        }
    }

    /// The entity types the data needs name, in order, once each.
    /// What the schema and the kit's own checks cannot say about a `form` (AP-61, AP-62): it
    /// needs an application that may write, and its fields are attributes the data needs
    /// declare for that type, so a form never edits what the endpoint never granted.
    pub(super) fn form_errors(&self, spec: &kit::Spec) -> Vec<String> {
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
    pub(super) fn need_attrs(&self, entity_type: &str) -> Vec<String> {
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
    pub(super) fn types(&self) -> Vec<String> {
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
    pub(super) fn served_needs(&self) -> Value {
        needs_served(&self.data_needs, self.schema_index.get())
    }

    /// The endpoint's schema index, read through the proxy with the run's ticket (EP-46). A run
    /// over several endpoints gets one index: every endpoint's models, each marked with the
    /// index of its endpoint; an endpoint whose index cannot be read keeps the types its needs
    /// name, so a proxy that does not serve it yet narrows nothing away.
    pub(super) async fn schema_index(&self) -> Result<Value, String> {
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
    pub(super) fn data_base(&self, at: usize) -> String {
        endpoints::data_base(&self.proxy_base, &self.endpoints, at)
    }

    /// Where the entities of one type are read: through the endpoint of the need naming it.
    pub(super) fn data_base_of(&self, entity_type: &str) -> String {
        self.data_base(endpoints::of_type(
            &self.endpoints,
            &self.data_needs,
            entity_type,
        ))
    }

    /// Every source's rows, read through the proxy in pages up to the source's limit. A source
    /// that cannot be read is an empty list and a line in the chat: the dashboard still shows.
    pub(super) async fn rows(&self, spec: &kit::Spec) -> Value {
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
    pub(super) async fn read_entities(&self, url: &str) -> Result<Value, String> {
        let body = self.read_text(url).await?;
        serde_json::from_str::<Value>(&body).map_err(|err| err.to_string())
    }

    /// One GET through the proxy with the run's ticket, as text.
    pub(super) async fn read_text(&self, url: &str) -> Result<String, String> {
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
    /// type several endpoints serve is read through each and joined by id, the first endpoint's
    /// entities leading. A type that cannot be read is an empty list with the reason in the chat:
    /// the model still gets the data needs, and the chat says what was missing.
    pub(super) async fn samples(&self, types: &[String]) -> Result<Value, String> {
        let mut samples = serde_json::Map::new();
        let mut joined = String::new();
        for entity_type in types {
            let serving = endpoints::serving(&self.endpoints, &self.data_needs, entity_type);
            let names: Vec<&str> = serving
                .iter()
                .filter_map(|&at| self.endpoints.get(at))
                .map(|endpoint| endpoint.name.as_str())
                .collect();
            self.thought(&format!(
                "Reading {SAMPLES_PER_TYPE} entities of {entity_type} through {}.",
                if self.endpoints.len() < 2 || names.is_empty() {
                    "the endpoint".to_owned()
                } else {
                    names.join(" and ")
                }
            ))
            .await?;
            let mut rows: Vec<Value> = Vec::new();
            let mut carried: Vec<String> = Vec::new();
            for &at in &serving {
                let base = format!(
                    "{}/ngsi-ld/v1/entities?type={}&limit={SAMPLES_PER_TYPE}&options=keyValues",
                    self.data_base(at),
                    urlencoding(entity_type)
                );
                let ids: Vec<&str> = rows.iter().filter_map(|row| row["id"].as_str()).collect();
                // A later endpoint is asked for the entities already read, so their parts meet.
                let mut read = if ids.is_empty() {
                    self.read_entities(&base).await
                } else {
                    self.read_entities(&format!("{base}&id={}", urlencoding(&ids.join(","))))
                        .await
                };
                if !ids.is_empty() && matches!(&read, Ok(Value::Array(found)) if found.is_empty()) {
                    read = self.read_entities(&base).await;
                }
                let name = self
                    .endpoints
                    .get(at)
                    .map_or("the endpoint", |endpoint| endpoint.name.as_str());
                match read {
                    Ok(Value::Array(entities)) => {
                        carried.push(format!(
                            "`{name}` carries {}",
                            endpoints::attributes_of(&entities).join(", ")
                        ));
                        endpoints::join_by_id(&mut rows, entities);
                    }
                    Ok(_) => {}
                    Err(reason) => {
                        self.thought(&format!(
                            "No sample of {entity_type} could be read through {name}: {reason}"
                        ))
                        .await?;
                    }
                }
            }
            if carried.len() > 1 {
                joined.push_str(&format!(
                    "- {entity_type}: {}. The samples are these rows joined by `id`; read the \
                     type from each endpoint with `{{ endpoint }}` and join the rows the same way.\n",
                    carried.join("; ")
                ));
            }
            samples.insert(entity_type.clone(), Value::Array(rows));
        }
        let _ = self.joined.set(joined);
        Ok(Value::Object(samples))
    }

    pub(super) async fn status(&self, status: AgentRunStatus) -> Result<(), String> {
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
    pub(super) async fn commit(&self, message: &str) -> Result<(), String> {
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
}

/// A tool the model may call in an editing turn (SDK-20), in the provider's own body.
pub(super) struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

/// One call the model asked for: the id the provider echoes, the tool and its arguments.
#[derive(Debug, Clone)]
pub(super) struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// What a tool completion answered: prose, calls, and what it cost.
#[derive(Debug, Clone, Default)]
pub(super) struct ToolAnswer {
    pub text: Option<String>,
    pub calls: Vec<ToolCall>,
    pub usage_tokens: u64,
}

/// Anthropic takes alternating roles: two turns of one role in a row are one turn with the
/// content blocks joined (a string is one text block).
pub(super) fn merge_anthropic_messages(messages: &[Value]) -> Vec<Value> {
    let blocks = |content: &Value| -> Vec<Value> {
        match content {
            Value::String(text) => vec![json!({ "type": "text", "text": text })],
            Value::Array(parts) => parts.clone(),
            other => vec![json!({ "type": "text", "text": other.to_string() })],
        }
    };
    let mut merged: Vec<Value> = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let content = blocks(message.get("content").unwrap_or(&Value::Null));
        match merged.last_mut() {
            Some(last) if last.get("role").and_then(Value::as_str) == Some(role) => {
                if let Some(parts) = last.get_mut("content").and_then(Value::as_array_mut) {
                    parts.extend(content);
                }
            }
            _ => merged.push(json!({ "role": role, "content": content })),
        }
    }
    merged
}

/// The model's own turn, appended so the next call sees what it asked for.
pub(super) fn assistant_turn_message(provider: &str, answer: &ToolAnswer) -> Value {
    if provider == "anthropic" {
        let mut content = Vec::new();
        if let Some(text) = answer
            .text
            .as_deref()
            .filter(|text| !text.trim().is_empty())
        {
            content.push(json!({ "type": "text", "text": text }));
        }
        for call in &answer.calls {
            content.push(json!({
                "type": "tool_use",
                "id": call.id,
                "name": call.name,
                "input": call.input,
            }));
        }
        json!({ "role": "assistant", "content": content })
    } else {
        let calls: Vec<Value> = answer
            .calls
            .iter()
            .map(|call| {
                json!({
                    "id": call.id,
                    "type": "function",
                    "function": { "name": call.name, "arguments": call.input.to_string() },
                })
            })
            .collect();
        let mut message = json!({ "role": "assistant", "content": answer.text });
        if !calls.is_empty() {
            message["tool_calls"] = Value::Array(calls);
        }
        message
    }
}

/// A tool's result, in the shape the provider reads it back.
pub(super) fn tool_result_message(provider: &str, call_id: &str, content: &str) -> Value {
    if provider == "anthropic" {
        json!({
            "role": "user",
            "content": [{ "type": "tool_result", "tool_use_id": call_id, "content": content }],
        })
    } else {
        json!({ "role": "tool", "tool_call_id": call_id, "content": content })
    }
}

//! The data tools the model calls on the endpoints a run opened: open, query, read, write (AG-70).

use super::*;

impl Driver {
    /// Each endpoint's read tools, from its own `tools/list` through the proxy, so the model
    /// sees exactly what the gateway offers this person there (AG-75). An endpoint that does
    /// not answer offers nothing.
    pub(super) async fn data_tools(&self, chosen: &[endpoints::RunEndpoint]) -> Vec<Vec<Value>> {
        let lists = (0..chosen.len()).map(|index| self.tools_of(chosen, index));
        futures_util::future::join_all(lists).await
    }

    /// The read tools one endpoint of the conversation offers, from its own `tools/list`.
    pub(super) async fn tools_of(
        &self,
        chosen: &[endpoints::RunEndpoint],
        index: usize,
    ) -> Vec<Value> {
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
    pub(super) fn openable_endpoints(
        &self,
        chosen: &[endpoints::RunEndpoint],
    ) -> Vec<data_query::Openable> {
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
    pub(super) async fn open_endpoint(
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
    pub(super) async fn query_endpoint(
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
                Some(data_query::not_offered(
                    tools.get(i).map_or(&[], Vec::as_slice),
                    &call.name,
                    &call.endpoint,
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
        let answer = self
            .call_endpoint(chosen, index.unwrap_or(0), &data_query::rpc(call))
            .await;
        let text = self.redacted(&data_query::result_text(&answer));
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

    /// One JSON-RPC request to an endpoint of the conversation through the proxy, with the
    /// person's grants: the endpoint's answer, or `{error}` saying why there is none.
    pub(super) async fn call_endpoint(
        &self,
        chosen: &[endpoints::RunEndpoint],
        index: usize,
        rpc: &Value,
    ) -> Value {
        let url = format!(
            "{}/mcp",
            endpoints::data_base(&self.proxy_base, chosen, index)
        );
        match self
            .http
            .post(&url)
            .bearer_auth(&self.bearer)
            .header("accept", "application/json")
            .json(rpc)
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
        }
    }

    /// What one read tool of an endpoint answered, structured; `Err` is the refusal in words.
    pub(super) async fn read_endpoint(
        &self,
        chosen: &[endpoints::RunEndpoint],
        index: usize,
        name: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        let call = data_query::QueryCall {
            endpoint: String::new(),
            name: name.to_owned(),
            arguments,
        };
        let answer = self
            .call_endpoint(chosen, index, &data_query::rpc(&call))
            .await;
        let refused = answer.get("error").is_some()
            || answer.pointer("/result/isError").and_then(Value::as_bool) == Some(true);
        if refused {
            return Err(self.redacted(&data_query::result_text(&answer)));
        }
        Ok(answer
            .pointer("/result/structuredContent")
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// A change to entities prepared for the person (AG-78): their grants on the endpoint and
    /// each entity are read through the proxy, and the preview is published for the card to
    /// apply. Nothing is written; what the grants refuse goes back to the model with the reason.
    pub(super) async fn write_entities(
        &self,
        call: Result<entity_write::WriteEntities, String>,
        answer: &str,
        chosen: &mut Vec<endpoints::RunEndpoint>,
        tools: &mut Vec<Vec<Value>>,
        last: bool,
    ) -> Result<Worked, String> {
        const TOOL: &str = "write_entities";
        let started = std::time::Instant::now();
        let input = call
            .as_ref()
            .ok()
            .and_then(|c| serde_json::to_value(c).ok())
            .unwrap_or(Value::Null);
        let prepared = match call {
            Ok(call) => self.prepared_write(&call, chosen, tools).await,
            Err(reason) => Err(reason),
        };
        let output = match prepared {
            Ok(output) => output,
            Err(reason) => {
                self.event("tool", failed_step(TOOL, started, &input, &reason))
                    .await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}"),
                        format!("The change could not be prepared: {reason}"),
                    )
                    .await;
            }
        };
        self.event(
            "tool",
            json!({
                "tool": TOOL,
                "status": "ok",
                "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "input": input,
                "output": output,
            }),
        )
        .await?;
        let mut prose = share::prose_of(answer);
        if prose.is_empty() {
            prose = "Review the values and apply the change.".to_owned();
        }
        self.thought(&prose).await?;
        Ok(Worked::Done(prose))
    }

    /// The preview of a `write_entities` call: every entity the person's grants let them update,
    /// as it is and as it would be.
    pub(super) async fn prepared_write(
        &self,
        call: &entity_write::WriteEntities,
        chosen: &mut Vec<endpoints::RunEndpoint>,
        tools: &mut Vec<Vec<Value>>,
    ) -> Result<Value, String> {
        entity_write::checked(call)?;
        let index = self.open_endpoint(chosen, tools, &call.endpoint).await?;
        let access = self
            .read_endpoint(chosen, index, "describe_access", json!({}))
            .await
            .map_err(|reason| {
                format!(
                    "the grants on '{}' could not be read: {reason}",
                    call.endpoint
                )
            })?;
        let mut entities = Vec::new();
        for change in &call.entities {
            let entity_type = entity_write::type_of(&change.id).unwrap_or_default();
            let attributes: Vec<&str> = change.attrs.keys().map(String::as_str).collect();
            if let Some(reason) = entity_write::refusal(&access, entity_type, &attributes) {
                return Err(format!("{}: {reason}", change.id));
            }
            let current = self
                .read_endpoint(chosen, index, "get_entity", json!({ "id": change.id }))
                .await
                .map_err(|reason| format!("{} could not be read: {reason}", change.id))?;
            entities.push(entity_write::previewed(change, entity_type, &current));
        }
        Ok(json!({
            "endpoint": call.endpoint,
            "slug": chosen[index].slug,
            "entities": entities,
        }))
    }
}

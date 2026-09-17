//! The change tools of the conversation: resources, dashboards, models, grants, the catalog search (AG-77, PF-52).

use super::*;

impl Driver {
    /// Whether this run may open a change of `kind` for the person (AG-70): the profile names
    /// `jc_resource_propose` with `propose` on the kind, or shares endpoints and the kind is
    /// Endpoint; and the person holds the verb, `delete` for a removal.
    pub(super) fn may_change(&self, kind: &str, verb: Verb) -> Result<(), String> {
        let shares = kind == "Endpoint" && self.granted("jc_endpoint_propose").is_ok();
        if !shares && !self.access.proposes("jc_resource_propose", kind) {
            return Err(format!(
                "the agent profile does not grant propose on {kind} through jc_resource_propose (AG-70)"
            ));
        }
        crate::permissions::for_request(&self.state, &self.identity, &self.home(kind))
            .check(kind, verb, None)
            .map_err(|err| err.to_string())
    }

    /// Where a kind's resources live: the organization's namespace for a RoleBinding, this
    /// project for everything else, and this project for a `Role` the run writes, which is a
    /// role of the project it works in (PF-68).
    pub(super) fn home(&self, kind: &str) -> String {
        match crate::resource::by_kind(kind) {
            Some(info) => crate::resource::home(info, &self.project),
            None => self.project.clone(),
        }
    }

    /// The project's resources this run may change, by kind, so the model names one that exists.
    pub(super) fn changeable(&self) -> BTreeMap<&'static str, Vec<String>> {
        crate::resource::kinds()
            .filter(|info| info.scope.allows_project() || info.scope.allows_organization())
            .filter(|info| self.may_change(info.kind, Verb::Propose).is_ok())
            .filter_map(|info| {
                let names = self.names_of(info.kind);
                (!names.is_empty()).then_some((info.kind, names))
            })
            .collect()
    }

    /// The names of this kind the run may name, from every place the kind lives: for a `Role`
    /// that is the project's own roles and the organization's, because both are in force here
    /// (PF-68, T-0872).
    pub(super) fn names_of(&self, kind: &str) -> Vec<String> {
        let homes = match crate::resource::by_kind(kind) {
            Some(info) => crate::resource::homes(info, &self.project),
            None => vec![self.project.clone()],
        };
        let mut names: Vec<String> = homes
            .iter()
            .flat_map(|home| {
                self.state
                    .mirror
                    .list(home, kind, &crate::store::ListOptions::default())
                    .items
            })
            .map(|envelope| envelope.metadata.name)
            .collect();
        names.sort();
        names
    }

    /// A change or a removal of a resource the conversation names (AG-77, AG-73). What the model
    /// got wrong goes back to it while drafts are left: an unknown kind or name with the real
    /// ones, a patch the check refuses with the reason and the manifest as it is. Nothing is
    /// proposed; the kind's page opens on the change or the removal for the person.
    pub(super) async fn change_resource(
        &self,
        call: Result<change::ChangeResource, String>,
        answer: &str,
        last: bool,
    ) -> Result<Worked, String> {
        const TOOL: &str = "change_resource";
        let started = std::time::Instant::now();
        let failed = |input: &Value, reason: &str| failed_step(TOOL, started, input, reason);
        let params = match call {
            Ok(params) => params,
            Err(reason) => {
                self.event("tool", failed(&Value::Null, &reason)).await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}"),
                        format!("The change could not be read: {reason}"),
                    )
                    .await;
            }
        };
        let input = serde_json::to_value(&params).unwrap_or(Value::Null);
        let changeable = self.changeable();
        let Some(info) = crate::resource::by_kind(params.kind.trim())
            .filter(|info| info.scope.allows_project() || info.scope.allows_organization())
        else {
            let kinds: Vec<&str> = changeable.keys().copied().collect();
            let reason = format!(
                "'{}' is not a kind of this project; the kinds you may change are {}",
                params.kind,
                kinds.join(", ")
            );
            self.event("tool", failed(&input, &reason)).await?;
            return self
                .again(
                    last,
                    format!("error: {reason}"),
                    format!("That could not be changed: {reason}"),
                )
                .await;
        };
        let verb = if params.delete {
            Verb::Delete
        } else {
            Verb::Propose
        };
        if let Err(reason) = self.may_change(info.kind, verb) {
            return self
                .refused(TOOL, started, input, reason)
                .await
                .map(Worked::Done);
        }
        if params.create {
            return self
                .create_dashboard(info.kind, &params, answer, input, started, last)
                .await;
        }
        let name = params.name.trim();
        let Some(current) = self
            .state
            .mirror
            .get(&self.home(info.kind), info.kind, name)
        else {
            let names = self.names_of(info.kind);
            let reason = if names.is_empty() {
                format!(
                    "{} '{name}' does not exist; the project has no {}",
                    info.kind, info.kind
                )
            } else {
                format!(
                    "{} '{name}' does not exist; the project's are {}",
                    info.kind,
                    names.join(", ")
                )
            };
            self.event("tool", failed(&input, &reason)).await?;
            return self
                .again(
                    last,
                    format!("error: {reason}"),
                    format!("That could not be changed: {reason}"),
                )
                .await;
        };
        let route = change::route(&self.project, info.kind, info.plural, name, params.delete);
        // A model's classes and attributes live in its LinkML source, not in the manifest.
        if info.kind == "DataModel" && !params.delete {
            let operations = params.operations.as_deref().unwrap_or_default();
            return self
                .change_model(name, operations, answer, input, started, last, route)
                .await;
        }
        let draft = json!({ "kind": info.kind, "name": name });

        if params.delete {
            self.event(
                "tool",
                json!({
                    "tool": TOOL,
                    "status": "ok",
                    "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    "input": input,
                    "output": { "kind": info.kind, "name": name, "delete": true },
                }),
            )
            .await?;
            let mut prose = share::prose_of(answer);
            if prose.is_empty() {
                prose =
                    format!("Opened the removal of '{name}'; type its name there to propose it.");
            }
            self.thought(&prose).await?;
            self.event("navigate", json!({ "route": route })).await?;
            return Ok(Worked::Done(prose));
        }

        let mut current = serde_json::to_value(&current).map_err(|err| err.to_string())?;
        if let Value::Object(fields) = &mut current {
            fields.remove("status");
        }
        let Some(patch) = params.patch.as_ref() else {
            // A call without a patch reads the manifest first, so the next one names real fields.
            return self
                .again(
                    last,
                    format!("the manifest of {} '{name}' as it is: {current}; answer with change_resource and a patch of the fields that change", info.kind),
                    format!("Which field of '{name}' should change, and to what value?"),
                )
                .await;
        };
        let manifest = match change::patched(&current, patch) {
            Ok(manifest) => manifest,
            Err(reason) => {
                self.event("tool", failed(&input, &reason)).await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}; the manifest as it is: {current}"),
                        format!("The change to '{name}' could not be made: {reason}"),
                    )
                    .await;
            }
        };
        // A pipeline change the test does not exercise opens untested: a pause is not a mapping.
        let untested = info.kind == "Pipeline" && !change::reaches_the_test(&current, &manifest);
        self.open_change(
            info, name, manifest, answer, TOOL, input, started, last, route, draft, untested,
        )
        .await
    }

    /// A new dashboard with the new layers its pages draw (AG-77, UI-17, UI-18). Every manifest
    /// passes the dry run and is kept as the person's draft, and the dashboard editor opens on
    /// the dashboard; its one proposal carries the layers with it. What the model got wrong goes
    /// back to it with the project's real names and the endpoint's attributes.
    pub(super) async fn create_dashboard(
        &self,
        kind: &str,
        params: &change::ChangeResource,
        answer: &str,
        input: Value,
        started: std::time::Instant,
        last: bool,
    ) -> Result<Worked, String> {
        const TOOL: &str = "change_resource";
        let millis = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let name = params.name.trim();
        if kind == "Dashboard" {
            if let Err(reason) = self.may_change("Layer", Verb::Propose) {
                return self
                    .refused(TOOL, started, input, reason)
                    .await
                    .map(Worked::Done);
            }
        }
        let mut drafted = match self.new_dashboard(kind, name, params).await {
            Ok(drafted) => drafted,
            Err(reason) => {
                self.event("tool", failed_step(TOOL, started, &input, &reason))
                    .await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}"),
                        format!("The dashboard could not be drafted: {reason}"),
                    )
                    .await;
            }
        };
        for manifest in &drafted {
            let refused = match crate::api::dry_run::execute_dry_run(
                &self.identity,
                &self.state,
                &self.project,
                manifest.clone(),
            )
            .await
            {
                Ok(result) if result.valid => None,
                Ok(result) => Some(serde_json::to_string(&result.plan).unwrap_or_default()),
                Err(err) => Some(err.to_string()),
            };
            if let Some(findings) = refused {
                let reason = format!(
                    "the platform's check refuses {} '{}': {findings}",
                    manifest["kind"].as_str().unwrap_or_default(),
                    manifest["metadata"]["name"].as_str().unwrap_or_default()
                );
                self.event("tool", failed_step(TOOL, started, &input, &reason))
                    .await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}; the manifest was {manifest}"),
                        format!("The dashboard does not pass the platform's check: {reason}"),
                    )
                    .await;
            }
        }
        for manifest in &drafted {
            let kind = manifest["kind"].as_str().unwrap_or_default();
            let named = manifest["metadata"]["name"].as_str().unwrap_or_default();
            let kept = self
                .state
                .drafts
                .put(
                    &self.project,
                    kind,
                    named,
                    manifest.clone(),
                    None,
                    &self.created_by,
                    "assistant",
                )
                .await;
            let verdict = crate::ops::verdict::Verdict::green(manifest, None);
            let checked = match kept {
                Ok(_) => self
                    .state
                    .drafts
                    .set_verdict(&self.project, kind, named, verdict)
                    .await
                    .map(|_| ()),
                Err(err) => Err(err),
            };
            if let Err(err) = checked {
                tracing::warn!(run = %self.run_id, kind, name = named, error = %err, "dashboard draft not kept");
            }
        }
        // The dashboard is drafted last, so it is the one the editor opens on.
        let dashboard = drafted.pop().unwrap_or(Value::Null);
        let layers: Vec<&str> = drafted
            .iter()
            .filter_map(|layer| layer["metadata"]["name"].as_str())
            .collect();
        self.event(
            "tool",
            json!({
                "tool": TOOL,
                "status": "ok",
                "durationMs": millis(),
                "input": input,
                "output": { "kind": "Dashboard", "name": name, "checked": true, "create": true, "layers": layers },
            }),
        )
        .await?;
        let mut prose = share::prose_of(answer);
        if prose.is_empty() {
            prose = format!("Drafted the dashboard '{name}'; review it and propose it.");
        }
        self.thought(&prose).await?;
        self.event(
            "navigate",
            json!({
                "route": format!("/projects/{}/dashboards?edit={name}", self.project),
                "prefill": dashboard,
                "draft": { "kind": "Dashboard", "name": name },
            }),
        )
        .await?;
        Ok(Worked::Done(prose))
    }

    /// The new layers and then the dashboard, as manifests, or what is wrong with the call in
    /// words the model can act on.
    pub(super) async fn new_dashboard(
        &self,
        kind: &str,
        name: &str,
        params: &change::ChangeResource,
    ) -> Result<Vec<Value>, String> {
        if kind != "Dashboard" {
            return Err(format!(
                "only a Dashboard is created here; a new {kind} is drafted on its own page"
            ));
        }
        if self
            .state
            .mirror
            .get(&self.project, "Dashboard", name)
            .is_some()
        {
            return Err(format!(
                "Dashboard '{name}' already exists; change it with a patch instead"
            ));
        }
        let spec = params.patch.as_ref().and_then(change::spec_of).ok_or(
            "a new dashboard carries its spec as the patch: {\"spec\": {\"title\", \"visibility\", \"pages\"}}",
        )?;
        let public = spec["visibility"].as_str() == Some("public");
        let endpoints: BTreeMap<String, Value> = self
            .state
            .mirror
            .list(
                &self.project,
                "Endpoint",
                &crate::store::ListOptions::default(),
            )
            .items
            .into_iter()
            .map(|envelope| (envelope.metadata.name, envelope.spec))
            .collect();
        let existing = self.names_of("Layer");
        let mut manifests = Vec::new();
        for layer in &params.layers {
            let layer_name = layer.name.trim();
            if existing.iter().any(|known| known == layer_name) {
                return Err(format!(
                    "Layer '{layer_name}' already exists; draw it by its name and leave it out of layers"
                ));
            }
            let source = layer.spec["sourceEndpointRef"].as_str().unwrap_or_default();
            let Some(endpoint) = endpoints.get(source) else {
                return Err(format!(
                    "layer '{layer_name}' reads '{source}', which is not an endpoint of the project; its endpoints are {}",
                    endpoints.keys().cloned().collect::<Vec<_>>().join(", ")
                ));
            };
            if public && endpoint["audience"].as_str() != Some("public") {
                return Err(format!(
                    "a public dashboard reads only public endpoints, and '{source}' is not public (UI-19)"
                ));
            }
            let entity_type = layer.spec["entityType"].as_str().unwrap_or_default();
            let named = change::named_attributes(&layer.spec);
            let schema = if named.is_empty() {
                None
            } else {
                fields::of_endpoint(
                    &self.state,
                    &self.project,
                    endpoint,
                    &[entity_type.to_owned()],
                )
                .await
            };
            if let Some(schema) = schema {
                let Some(properties) = schema[entity_type]["properties"].as_object() else {
                    return Err(format!(
                        "the data model behind '{source}' has no type '{entity_type}'"
                    ));
                };
                let unknown: Vec<&str> = named
                    .iter()
                    .map(String::as_str)
                    .filter(|attribute| !properties.contains_key(*attribute))
                    .collect();
                if !unknown.is_empty() {
                    return Err(format!(
                        "layer '{layer_name}' names {}, which {entity_type} on '{source}' does not have; its attributes are {}",
                        unknown.join(", "),
                        properties.keys().cloned().collect::<Vec<_>>().join(", ")
                    ));
                }
            }
            manifests.push(change::manifest(
                "Layer",
                &self.project,
                layer_name,
                layer.spec.clone(),
            ));
        }
        for drawn in crate::dashboards::layer_names(&spec) {
            let new = params.layers.iter().any(|layer| layer.name.trim() == drawn);
            if !new && !existing.contains(&drawn) {
                return Err(format!(
                    "a page draws '{drawn}', which is neither a layer of the project nor one of the new layers; the project's layers are {}",
                    if existing.is_empty() { "none".to_owned() } else { existing.join(", ") }
                ));
            }
        }
        manifests.push(change::manifest("Dashboard", &self.project, name, spec));
        Ok(manifests)
    }

    /// A data model changed by the editor's operations (AG-77, DM-13): applied to the LinkML
    /// source the repository holds and put through the source's dry run, which compiles it and
    /// classifies the change, a breaking one included; the Models page opens on the same
    /// operations and applies them to the text. What the model got wrong goes back to it with the
    /// model's classes and their slots.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn change_model(
        &self,
        name: &str,
        operations: &[model_change::Operation],
        answer: &str,
        input: Value,
        started: std::time::Instant,
        last: bool,
        route: String,
    ) -> Result<Worked, String> {
        const TOOL: &str = "change_resource";
        let home = &self.home("DataModel");
        let spec = self
            .state
            .mirror
            .get(home, "DataModel", name)
            .map(|envelope| envelope.spec)
            .unwrap_or(Value::Null);
        let model = match crate::api::datamodels::read_source(&self.state, home, name)
            .await
            .map_err(|err| err.to_string())
            .and_then(|source| {
                serde_yaml_ng::from_str::<Value>(&source).map_err(|err| err.to_string())
            }) {
            Ok(model) => model,
            Err(reason) => {
                let reason =
                    format!("the source of data model '{name}' could not be read: {reason}");
                self.event("tool", failed_step(TOOL, started, &input, &reason))
                    .await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}"),
                        format!("The data model '{name}' could not be changed: {reason}"),
                    )
                    .await;
            }
        };
        let outline = model_change::outline(&model);
        if operations.is_empty() {
            return self
                .again(
                    last,
                    format!("the classes of data model '{name}' and their slots: {outline}; answer with change_resource and the operations that change it"),
                    format!("What should change in the data model '{name}'?"),
                )
                .await;
        }
        let checked = match model_change::apply(&model, operations) {
            Ok(changed) => match serde_yaml_ng::to_string(&changed) {
                Ok(source) => crate::api::datamodels::check_source(
                    &self.state,
                    home,
                    name,
                    &spec,
                    &source,
                    None,
                )
                .await
                .map(|(checked, _)| checked)
                .map_err(|err| format!("the platform's check refuses the change: {err}")),
                Err(err) => Err(err.to_string()),
            },
            Err(reason) => Err(reason),
        };
        let checked = match checked {
            Ok(checked) => checked,
            Err(reason) => {
                self.event("tool", failed_step(TOOL, started, &input, &reason))
                    .await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}; the classes of data model '{name}' and their slots: {outline}"),
                        format!("The data model '{name}' could not be changed: {reason}"),
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
                "output": {
                    "kind": "DataModel",
                    "name": name,
                    "checked": true,
                    "severity": checked.severity,
                    "changes": checked.changes,
                    "version": checked.version,
                },
            }),
        )
        .await?;
        let mut prose = share::prose_of(answer);
        if prose.is_empty() {
            prose =
                format!("Opened the data model '{name}' with the change; review it and save it.");
        }
        self.thought(&prose).await?;
        self.event(
            "navigate",
            json!({ "route": route, "prefill": { "operations": operations } }),
        )
        .await?;
        Ok(Worked::Done(prose))
    }

    /// A role for people or a group (AG-77, PF-52): the binding named and checked like any
    /// proposal, which holds it to what the person holds on its scope, then the Access page's
    /// grant form opens on it. An unknown role goes back with the organization's roles.
    pub(super) async fn grant_role(
        &self,
        call: Result<grant::GrantRole, String>,
        answer: &str,
        last: bool,
    ) -> Result<Worked, String> {
        const TOOL: &str = "grant_role";
        let started = std::time::Instant::now();
        let params = match call {
            Ok(params) => params,
            Err(reason) => {
                self.event("tool", failed_step(TOOL, started, &Value::Null, &reason))
                    .await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}"),
                        format!("The grant could not be read: {reason}"),
                    )
                    .await;
            }
        };
        let input = serde_json::to_value(&params).unwrap_or(Value::Null);
        if let Err(reason) = self.may_change("RoleBinding", Verb::Propose) {
            return self
                .refused(TOOL, started, input, reason)
                .await
                .map(Worked::Done);
        }
        let roles = self.names_of("Role");
        let checked = if roles.iter().any(|role| role == params.role.trim()) {
            grant::manifest(
                &params,
                &crate::api::assistant::org_domain(&self.state, &self.project),
            )
        } else {
            Err(format!(
                "the organization has no role '{}'; its roles are {}",
                params.role,
                roles.join(", ")
            ))
        };
        let manifest = match checked {
            Ok(manifest) => manifest,
            Err(reason) => {
                self.event("tool", failed_step(TOOL, started, &input, &reason))
                    .await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}"),
                        format!("The role could not be granted: {reason}"),
                    )
                    .await;
            }
        };
        let name = manifest["metadata"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let info = crate::resource::by_kind("RoleBinding")
            .ok_or_else(|| "RoleBinding is not a kind of this Portal".to_owned())?;
        let route = format!("/projects/{}/access?grant={name}", self.project);
        let draft = json!({ "kind": "RoleBinding", "name": name });
        self.open_change(
            info, &name, manifest, answer, TOOL, input, started, last, route, draft, false,
        )
        .await
    }

    /// The patched manifest checked by the dry run every channel uses (AG-77), kept as the
    /// person's draft and opened on the kind's page. A refused check goes back to the model.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn open_change(
        &self,
        info: &crate::resource::KindInfo,
        name: &str,
        manifest: Value,
        answer: &str,
        tool: &str,
        input: Value,
        started: std::time::Instant,
        last: bool,
        route: String,
        draft: Value,
        untested: bool,
    ) -> Result<Worked, String> {
        let millis = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let home = &self.home(info.kind);
        let (refused, probe) = match crate::api::dry_run::execute_dry_run(
            &self.identity,
            &self.state,
            home,
            manifest.clone(),
        )
        .await
        {
            Ok(result) if result.valid => (None, result.probe),
            Ok(result) => (
                Some(format!(
                    "the plan does not validate: {}",
                    serde_json::to_string(&result.plan).unwrap_or_default()
                )),
                None,
            ),
            Err(err) => (Some(err.to_string()), None),
        };
        if let Some(findings) = refused {
            self.event(
                "tool",
                json!({
                    "tool": tool,
                    "status": "failed",
                    "durationMs": millis(),
                    "input": input,
                    "error": findings,
                }),
            )
            .await?;
            return self
                .again(
                    last,
                    format!("error: the platform's check refuses the change: {findings}; the changed manifest was {manifest}"),
                    format!("The change to '{name}' does not pass the platform's check: {findings}"),
                )
                .await;
        }
        // A changed pipeline runs on a page of its source and a changed data source fetches its
        // URL before the form opens (AG-77, PL-45, MF-39); a red test goes back to the model.
        let tested = if untested {
            Ok(Tested {
                card: Some(
                    json!({ "untested": "the change does not touch what the pipeline reads, maps or writes" }),
                ),
                verdict: None,
            })
        } else {
            self.tested(info.kind, &manifest, probe).await
        };
        let tested = match tested {
            Ok(tested) => tested,
            Err((findings, card)) => {
                self.event(
                    "tool",
                    json!({
                        "tool": tool,
                        "status": "failed",
                        "durationMs": millis(),
                        "input": input,
                        "error": format!("the test is not green: {findings}"),
                        "output": { "kind": info.kind, "name": name, "test": card },
                    }),
                )
                .await?;
                return self
                    .again(
                        last,
                        format!("error: the change's test is not green: {findings}; what it ran on: {card}; the changed manifest was {manifest}"),
                        format!("The change to '{name}' does not pass its test: {findings}"),
                    )
                    .await;
            }
        };
        if let Err(err) = self
            .state
            .drafts
            .put(
                home,
                info.kind,
                name,
                manifest.clone(),
                None,
                &self.created_by,
                "assistant",
            )
            .await
        {
            tracing::warn!(run = %self.run_id, kind = %info.kind, error = %err, "change draft not kept");
        }
        // The draft carries the check it passed, so the form proposes it without a second one
        // while the person leaves it as it is (AG-77).
        let verdict = tested
            .verdict
            .unwrap_or_else(|| crate::ops::verdict::Verdict::green(&manifest, None));
        if let Err(err) = self
            .state
            .drafts
            .set_verdict(home, info.kind, name, verdict)
            .await
        {
            tracing::warn!(run = %self.run_id, kind = %info.kind, error = %err, "change verdict not kept");
        }
        let mut output = json!({ "kind": info.kind, "name": name, "checked": true });
        if let Some(card) = tested.card {
            output["test"] = card;
        }
        self.event(
            "tool",
            json!({
                "tool": tool,
                "status": "ok",
                "durationMs": millis(),
                "input": input,
                "output": output,
            }),
        )
        .await?;
        let mut prose = share::prose_of(answer);
        if prose.is_empty() {
            prose = format!("Opened '{name}' with the change; review it and propose it.");
        }
        self.thought(&prose).await?;
        // The endpoint form reads its own values; every other page opens on the manifest.
        let prefill = if info.kind == "Endpoint" {
            share::form_values(&manifest)
        } else {
            manifest
        };
        self.event(
            "navigate",
            json!({ "route": route, "prefill": prefill, "draft": draft }),
        )
        .await?;
        Ok(Worked::Done(prose))
    }

    /// The test of a changed Pipeline or DataSource; every other kind has none. `Err` carries the
    /// findings and the card of what the test ran on; a test that could not run (no runner, a
    /// credential, a source without a page) is a card that says why, never a refusal.
    pub(super) async fn tested(
        &self,
        kind: &str,
        manifest: &Value,
        probe: Option<crate::api::dry_run::Probe>,
    ) -> Result<Tested, (String, Value)> {
        match kind {
            "DataSource" => {
                let Some(probe) = probe else {
                    return Ok(Tested::default());
                };
                let failed = crate::api::pipeline_test::feed_failed(&probe).map(str::to_owned);
                let card =
                    json!({ "source": { "url": manifest["spec"]["http"]["url"] }, "probe": probe });
                match failed {
                    Some(reason) => Err((reason, card)),
                    None => Ok(Tested {
                        card: Some(card),
                        verdict: None,
                    }),
                }
            }
            "Pipeline" => {
                let (sample, source) = match self.page_of(manifest).await {
                    Ok(page) => page,
                    Err(untested) => {
                        return Ok(Tested {
                            card: Some(json!({ "untested": untested })),
                            verdict: None,
                        })
                    }
                };
                let Some(op) = crate::ops::find("jc_pipeline_test") else {
                    return Ok(Tested::default());
                };
                let caller = crate::ops::Caller {
                    identity: self.identity.clone(),
                    via: crate::ops::Via::Agent,
                    access: None,
                };
                let test = json!({ "pipeline": manifest, "sample": sample });
                let out = match crate::ops::call(op, &caller, &self.state, &self.project, test)
                    .await
                {
                    Ok(out) => out,
                    Err(err) => {
                        return Ok(Tested {
                            card: Some(json!({ "source": source, "untested": err.to_string() })),
                            verdict: None,
                        })
                    }
                };
                let verdict = out.get("verdict").cloned().unwrap_or(Value::Null);
                let card = json!({
                    "source": source,
                    "verdict": { "ok": verdict["ok"], "findings": verdict["findings"] },
                    "records": out.pointer("/input/events"),
                    "sample": out.get("mapping").and_then(Value::as_array).map(|m| m.iter().take(3).cloned().collect::<Vec<_>>()),
                });
                if verdict["ok"].as_bool() == Some(true) {
                    Ok(Tested {
                        card: Some(card),
                        verdict: serde_json::from_value(verdict).ok(),
                    })
                } else {
                    Err((findings_of(&verdict), card))
                }
            }
            _ => Ok(Tested::default()),
        }
    }

    /// What a pipeline is tested on (PL-43): one fetch of its `http` data source, or a page of the
    /// endpoint it reads when this conversation reads that endpoint, with the source it came from.
    /// `Err` says why there is none.
    pub(super) async fn page_of(&self, manifest: &Value) -> Result<(Value, Value), String> {
        let source = &manifest["spec"]["source"];
        if let Some(name) = source["dataSourceRef"]["name"].as_str() {
            let Some(data_source) = self.state.mirror.get(&self.project, "DataSource", name) else {
                return Err(format!("its data source '{name}' does not exist"));
            };
            return match crate::api::pipeline_test::probe_plan(&data_source.spec) {
                Some(Ok(url)) => Ok((
                    json!({ "url": url, "format": "json" }),
                    json!({ "dataSource": name }),
                )),
                Some(Err(skipped)) => Err(skipped.skipped.unwrap_or_default()),
                None => Err(format!(
                    "its data source '{name}' is not fetched over http, so it has no page to test on"
                )),
            };
        }
        let Some(endpoint) = source["endpointRef"]["name"].as_str() else {
            return Err("it names no source to test on".to_owned());
        };
        let chosen = match self.state.agents.get_run(&self.run_id).await {
            Ok(Some(run)) => endpoints::of_run(&run),
            _ => self.endpoints.clone(),
        };
        let Some(index) = chosen.iter().position(|e| e.name == endpoint) else {
            return Err(format!(
                "this conversation does not read its endpoint '{endpoint}'"
            ));
        };
        let query = &source["query"];
        let mut url = format!(
            "{}/ngsi-ld/v1/entities?limit=100",
            endpoints::data_base(&self.proxy_base, &chosen, index)
        );
        for (parameter, value) in [
            ("type", query["type"].as_str().map(str::to_owned)),
            ("q", query["q"].as_str().map(str::to_owned)),
            (
                "attrs",
                query["attrs"].as_array().map(|attrs| {
                    attrs
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                }),
            ),
        ] {
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                url.push_str(&format!("&{parameter}={}", urlencoding(&value)));
            }
        }
        let page = self.read_text(&url).await?;
        Ok((
            // The runner reads an endpoint's page as one message, as the KPI pipeline test does.
            json!({ "text": page, "format": "text" }),
            json!({ "endpoint": endpoint }),
        ))
    }

    /// What goes back to the model while drafts are left, or the person's answer on the last one.
    pub(super) async fn again(
        &self,
        last: bool,
        reason: String,
        prose: String,
    ) -> Result<Worked, String> {
        if last {
            self.thought(&prose).await?;
            return Ok(Worked::Done(prose));
        }
        Ok(Worked::Again(reason))
    }

    /// The project's endpoints as manifests, from the Portal's mirror of the repository.
    pub(super) fn project_endpoints(&self) -> Vec<Value> {
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

    /// The `edit_endpoint` call of the earlier prompt, still read for one release (EP-72, AG-77):
    /// the edit becomes the endpoint's changed manifest and opens like a `change_resource` change.
    pub(super) async fn edit_endpoint(
        &self,
        call: Result<share::EditEndpoint, String>,
        answer: &str,
    ) -> Result<String, String> {
        const TOOL: &str = "edit_endpoint";
        let started = std::time::Instant::now();
        let failed = |input: &Value, reason: &str| {
            json!({
                "tool": TOOL,
                "status": "failed",
                "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "input": input,
                "error": reason,
            })
        };
        let params = match call {
            Ok(params) => params,
            Err(reason) => {
                self.event("tool", failed(&Value::Null, &reason)).await?;
                let prose = format!("The change could not be read: {reason}");
                self.thought(&prose).await?;
                return Ok(prose);
            }
        };
        let input = serde_json::to_value(&params).unwrap_or(Value::Null);
        if let Err(reason) = self.may_change("Endpoint", Verb::Propose) {
            return self.refused(TOOL, started, input, reason).await;
        }
        let edit = match share::edit(&self.project_endpoints(), &params) {
            Ok(edit) => edit,
            Err(reason) => {
                self.event("tool", failed(&input, &reason)).await?;
                let prose = format!("The endpoint could not be changed: {reason}");
                self.thought(&prose).await?;
                return Ok(prose);
            }
        };
        let name = params.name.trim();
        let info = crate::resource::by_kind("Endpoint").ok_or("Endpoint is not a kind")?;
        let route = change::route(&self.project, info.kind, info.plural, name, false);
        let draft = json!({ "kind": info.kind, "name": name });
        match self
            .open_change(
                info,
                name,
                edit.endpoint,
                answer,
                TOOL,
                input,
                started,
                true,
                route,
                draft,
                false,
            )
            .await?
        {
            Worked::Done(prose) | Worked::Again(prose) => Ok(prose),
        }
    }

    /// One catalog search for both doors (AG-70): the platform runs it and records the tool
    /// step; `None` when nothing in the project matches.
    async fn catalog(&self, q: &str) -> Result<Option<Value>, String> {
        let started = std::time::Instant::now();
        let catalog = crate::api::assistant::search(&self.state, &self.project, q, None).await;
        let output = serde_json::to_value(&catalog).unwrap_or(Value::Null);
        self.event(
            "tool",
            json!({
                "tool": "search_catalog",
                "status": "ok",
                "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "input": { "q": q },
                "output": output.clone(),
            }),
        )
        .await?;
        Ok((!catalog.items.is_empty()).then_some(output))
    }

    /// The search the platform runs before the first pass: outside the run's access it is
    /// skipped, and the prompt goes without it (AG-70).
    pub(super) async fn find(&self, question: &str) -> Result<Option<Value>, String> {
        if self.granted("jc_catalog_search").is_err() {
            return Ok(None);
        }
        self.catalog(question).await
    }

    /// A `search_catalog` call of the model, with its own words: the same search and the same
    /// tool step as [`Self::find`], answered as text the next model call reads.
    pub(super) async fn search(&self, q: &str) -> Result<String, String> {
        if let Err(reason) = self.granted("jc_catalog_search") {
            return Ok(format!("error: {reason}"));
        }
        Ok(match self.catalog(q).await? {
            Some(output) => output.to_string(),
            None => format!("nothing in the project matches the words '{q}'"),
        })
    }

    /// The profile and the person who started the run both allow the operation behind a tool
    /// (AG-70).
    pub(super) fn granted(&self, operation: &str) -> Result<(), String> {
        self.access
            .check(operation, &self.identity, &self.state, &self.project)
    }

    /// A tool call outside the run's access: a failed `tool` event before anything runs, and the
    /// reason in the chat (AG-56, AG-70).
    pub(super) async fn refused(
        &self,
        tool: &str,
        started: std::time::Instant,
        input: Value,
        reason: String,
    ) -> Result<String, String> {
        self.event("tool", failed_step(tool, started, &input, &reason))
            .await?;
        let prose = format!("That is outside what this assistant may do: {reason}");
        self.thought(&prose).await?;
        Ok(prose)
    }

    /// A prompt without the sections of the tools this run may not call, so the model is shown
    /// only the effective tool set (AG-70).
    pub(super) fn narrowed(&self, prompt: &str) -> String {
        TOOL_SECTIONS
            .iter()
            .filter(|(_, operation)| self.granted(operation).is_err())
            .fold(prompt.to_owned(), |text, (heading, _)| {
                without_section(&text, heading)
            })
    }
}

//! The code pass of a workspace run: the model writes files, the workspace builds and tests them, the first version is published (AG-53, AG-54).

use super::*;

impl Driver {
    /// A code run (Architecture/20 §4.1): one call writes the application over the template,
    /// what does not build goes back once, every generated version is checked against what the
    /// frame observed (SDK-28), and every message after the first run is one more pass over the
    /// same files. The template is the model's context, never the preview (SDK-14).
    pub(super) async fn drive_code(
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

        let samples = self.samples(&self.types()).await?;
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
                    // A version on screen is edited in place, tool by tool (SDK-20); before one
                    // exists the message is one more whole pass.
                    if shown.is_some() {
                        match self
                            .edit_turn(
                                &mut files,
                                &mut committed,
                                &mut conversation,
                                &text,
                                shown.as_ref(),
                            )
                            .await
                        {
                            Ok(Some(next)) => shown = Some(next),
                            Ok(None) => {}
                            Err(message) => {
                                self.thought(&format!("The turn failed: {message}")).await?
                            }
                        }
                        continue;
                    }
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
    pub(super) async fn verify(
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
    pub(super) async fn code_pass(
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
            // The errors go to the model (`Fix::Build`); the person reads one plain sentence,
            // never the patch protocol (T-0785).
            self.thought("The application does not build; fixing it.")
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
            let errors: Vec<String> = errors.into_iter().filter(|e| !is_protocol(e)).collect();
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
    pub(super) async fn code_step(
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
                // A block the model must send again is a repair, not a refusal: the step
                // stays a plain step (T-0785).
                "exitCode": if refused.is_empty() { 0 } else { 1 },
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
    pub(super) async fn code_pack(
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
        if let Some(joined) = self.joined.get().filter(|joined| !joined.is_empty()) {
            pack.push_str("\nTypes read from several endpoints:\n");
            pack.push_str(joined);
        }
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
    pub(super) async fn jc_types(&self) -> Result<String, String> {
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
    pub(super) async fn publish_code(
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
    pub(super) async fn commit_files(
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
    pub(super) async fn first_version(&self) {
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
}

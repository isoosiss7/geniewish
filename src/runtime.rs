use crate::{
    app_server::AppServer,
    state::{State, Update},
    storage::Store,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

#[derive(Clone)]
struct Approval {
    id: Value,
    owner: u64,
    turn: String,
}

pub use crate::state::Delivery;

#[derive(Default)]
pub struct Settings {
    pub router_model: Option<String>,
    pub hr_model: Option<String>,
    pub worker_model: Option<String>,
    pub output_dir: PathBuf,
}

pub struct Runtime {
    pub server: AppServer,
    workspace: PathBuf,
    loaded: HashSet<u64>,
    turns: HashMap<u64, String>,
    approvals: Vec<Approval>,
    final_messages: HashMap<u64, Value>,
    pub wait_for_playback: bool,
    deliveries: HashMap<Delivery, Vec<Delivery>>,
    pub(crate) settings: Settings,
    bridge: crate::tools::Bridge,
    tool_requests: HashMap<String, crate::tools::Request>,
    tool_results: HashMap<String, Value>,
    native_tools: HashMap<String, Value>,
    pub(crate) hr_confirmation: Option<String>,
    pub(crate) hr_reads: Vec<Delivery>,
    pub(crate) last_reply: Option<Value>,
    pub(crate) routing_config: Option<Value>,
    pub(crate) routing_clarification: Option<String>,
}

/// Commit local metadata before allowing the in-memory state to advance.
pub fn change<T>(
    state: &mut State,
    store: &Store,
    operation: impl FnOnce(&mut State) -> Result<T>,
) -> Result<T> {
    let mut next = state.clone();
    let result = operation(&mut next)?;
    store.save(&next)?;
    *state = next;
    Ok(result)
}

pub fn emit(value: Value) -> Result<()> {
    crate::diagnostics::record(
        "output",
        json!({"type":value["type"],"genie":value["genie"],"method":value["method"],"status":value["status"]}),
    );
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &value)?;
    writeln!(stdout)?;
    stdout.flush()?;
    Ok(())
}

impl Runtime {
    pub async fn connect(
        executable: &str,
        workspace: PathBuf,
        mut settings: Settings,
    ) -> Result<Self> {
        std::fs::create_dir_all(&settings.output_dir)?;
        settings.output_dir = settings.output_dir.canonicalize()?;
        Ok(Self {
            server: AppServer::connect(executable).await?,
            workspace: workspace.canonicalize()?,
            loaded: HashSet::new(),
            turns: HashMap::new(),
            approvals: vec![],
            final_messages: HashMap::new(),
            wait_for_playback: false,
            deliveries: HashMap::new(),
            settings,
            bridge: crate::tools::Bridge::open()?,
            tool_requests: HashMap::new(),
            tool_results: HashMap::new(),
            native_tools: HashMap::new(),
            hr_confirmation: None,
            hr_reads: vec![],
            last_reply: None,
            routing_config: None,
            routing_clarification: None,
        })
    }

    pub async fn restricted_config(&mut self) -> Result<Value> {
        if let Some(config) = &self.routing_config {
            return Ok(config.clone());
        }
        let loaded = self
            .server
            .call("config/read", json!({"includeLayers":false}))
            .await?;
        let mut config = json!({"features.memories":false,"memories.generate_memories":false,"memories.use_memories":false,
            "features.shell_tool":false,"features.apps":false,"features.code_mode":false,"features.code_mode_only":false,"features.multi_agent":false,
            "features.skip_host_skill_discovery":true,"features.skill_search":false,"include_apps_instructions":false,"project_doc_max_bytes":0,
            "cloud.mcp.enabled":false,"cloud.skills.enabled":false,"web_search":"disabled","tools.update_plan.enabled":false,"tools.experimental_request_user_input.enabled":false});
        for table in ["mcp_servers", "plugins"] {
            if let Some(entries) = loaded["config"][table].as_object() {
                for key in entries.keys() {
                    config[format!("{table}.{key}.enabled")] = false.into();
                }
            }
        }
        self.routing_config = Some(config.clone());
        Ok(config)
    }

    pub async fn next_event(&mut self) -> Result<Value> {
        tokio::select! {
            biased;
            event = self.server.next_event() => event,
            request = self.bridge.requests.recv() => {
                let request = request.context("HR tool bridge disconnected.")?;
                let id = request.call.id.clone();
                let message = json!({"method":"geniewish/tool/call","params":{"callId":id}});
                if let Some(previous) = self.tool_results.get(&id) {
                    let _ = request.reply.send(previous.clone());
                    return Ok(json!({"method":"geniewish/tool/replayed"}));
                }
                self.tool_requests.insert(id, request);
                Ok(message)
            }
        }
    }

    pub async fn ensure_thread(
        &mut self,
        state: &mut State,
        store: &Store,
        owner: u64,
    ) -> Result<String> {
        let genie = state.get(owner)?.clone();
        if self.loaded.contains(&owner) {
            return genie
                .thread_id
                .context("Loaded genie has no lifetime thread.");
        }
        let manager = state.manager == Some(owner);
        let mut config = if manager {
            self.restricted_config().await?
        } else {
            json!({"memories.use_memories":false,"memories.generate_memories":false})
        };
        if manager {
            config["mcp_servers.geniewish_hr"] = json!({"command":std::env::current_exe()?,"args":["--hr-tools",self.bridge.path.to_str().context("Invalid local tool path")?,&owner.to_string()],"enabled":true,"required":true,"startup_timeout_sec":10,"tool_timeout_sec":120,"default_tools_approval_mode":"approve"});
        } else {
            config["sandbox_workspace_write.writable_roots"] = json!([self.settings.output_dir]);
        }
        let voice_policy = format!(
            "Your name is {}. Refer to the user as Human. This is your one lifetime conversation. Human's role assignment accompanies messages and can be blank or change. Keep final replies plain prose, usually two to four short sentences; no spoken Markdown, tables, code blocks or full paths. For detailed reports save the full result in {} with a unique filename, verify successful writing, and tell Human the actual filename and a brief summary. Project source stays in its workspace. Human can request a longer readout. Do not give intermediate progress; only the final result. Ask actionable questions and seek required approvals. ",
            genie.name,
            self.settings.output_dir.display()
        );
        let instructions = if manager {
            format!(
                "{voice_policy}You are the HR manager. Handle all team dialogue yourself using manage_genies and save_report. Do not use shell, files, apps, or other work tools. Use tool results as authoritative team state; never claim an action succeeded before the tool succeeds. Choose an available catalog name if omitted, and hire immediately with a blank role unless Human provided one; do not ask for an optional role. For compound requests, make ordered tool calls, using the actual hired name for delegation. Forward every requirement to the worker. Confirm retirement only after an explicit named confirmation from Human. Pending questions belong to this conversation. Do not use tools for negated or hypothetical actions. Work requests can be delegated to a work genie."
            )
        } else {
            format!(
                "{voice_policy}Use your native work tools. Team management belongs to the HR manager; do not claim to hire or retire genies."
            )
        };
        let mut params = json!({"cwd":self.workspace,"approvalPolicy":"on-request","sandbox":if manager {"read-only"} else {"workspace-write"},"config":config,"developerInstructions":instructions});
        if manager {
            params["baseInstructions"] = json!(
                "You are geniewish's conversational HR manager. Use only the provided geniewish management tools for actions. Keep your own lifetime conversation; do not perform project work yourself."
            );
        }
        if let Some(model) = if manager {
            &self.settings.hr_model
        } else {
            &self.settings.worker_model
        } {
            params["model"] = json!(model);
        }
        let method = if let Some(thread) = &genie.thread_id {
            params["threadId"] = json!(thread);
            "thread/resume"
        } else {
            params["ephemeral"] = json!(false);
            "thread/start"
        };
        let response = self.server.call(method, params).await?;
        let thread = response["thread"]["id"]
            .as_str()
            .context("Runtime did not return a thread identity.")?
            .to_owned();
        if let Some(saved) = &genie.thread_id {
            ensure!(
                saved == &thread,
                "Resume returned a different conversation; refusing to replace it."
            );
        } else if let Err(error) = change(state, store, |s| s.bind_thread(owner, &thread)) {
            let cleanup = self
                .server
                .call("thread/delete", json!({"threadId":thread}))
                .await;
            return Err(error.context(format!(
                "The request was not sent. Unbound thread: {thread}; cleanup succeeded: {}",
                cleanup.is_ok()
            )));
        }
        if let Some(turns) = response["thread"]["turns"].as_array() {
            for turn in turns.iter().filter(|t| t["status"] == "inProgress") {
                if let Some(id) = turn["id"].as_str() {
                    self.turns.insert(owner, id.into());
                }
            }
        }
        self.loaded.insert(owner);
        Ok(thread)
    }

    pub fn busy(&self, owner: u64) -> bool {
        self.turns.contains_key(&owner)
    }
    pub(crate) fn approval_count(&self, owner: u64) -> usize {
        self.approvals.iter().filter(|a| a.owner == owner).count()
    }

    /// Returns true when the message steered the existing turn.
    pub async fn submit(&mut self, state: &mut State, store: &Store, text: &str) -> Result<bool> {
        self.submit_for(state, store, state.current()?.id, text)
            .await
    }

    pub async fn submit_for(
        &mut self,
        state: &mut State,
        store: &Store,
        owner: u64,
        text: &str,
    ) -> Result<bool> {
        crate::diagnostics::record(
            "work.destination",
            json!({"genie":state.get(owner)?.name,"busy":self.busy(owner)}),
        );
        crate::diagnostics::text("work.input", text);
        let thread = self.ensure_thread(state, store, owner).await?;
        let role = &state.get(owner)?.role;
        let context = if Some(owner) == state.manager {
            self.hr_confirmation = state
                .retiring
                .and_then(|id| state.get(id).ok())
                .filter(|g| crate::management::named_confirmation(text, &g.name))
                .map(|g| g.name.clone());
            format!(
                "Current authoritative team snapshot: {}",
                self.team_snapshot(state)
            )
        } else {
            format!(
                "Human's current role assignment for you: {}.",
                if role.is_empty() { "none" } else { role }
            )
        };
        let input = json!([{ "type":"text", "text":context }, {"type":"text","text":text}]);
        if let Some(turn) = self.turns.get(&owner).cloned() {
            let response = self
                .server
                .call(
                    "turn/steer",
                    json!({"threadId":thread,"expectedTurnId":turn,"input":input}),
                )
                .await?;
            ensure!(
                response["turnId"].as_str() == Some(&turn),
                "The steering acknowledgement did not match the active turn."
            );
            return Ok(true);
        }
        let response = self
            .server
            .call("turn/start", json!({"threadId":thread,"input":input}))
            .await?;
        let turn = response["turn"]["id"]
            .as_str()
            .context("Runtime did not return a turn identity.")?
            .to_owned();
        self.turns.insert(owner, turn);
        emit(json!({"type":"working","genie":state.get(owner)?.name}))?;
        Ok(false)
    }

    pub async fn retire(&mut self, state: &mut State, store: &Store, chosen: &str) -> Result<()> {
        let owner = state.named(chosen)?.id;
        ensure!(
            state.retiring == Some(owner) && Some(owner) != state.manager,
            "There is no matching retirement waiting for confirmation."
        );
        if let Some(thread) = &state.get(owner)?.thread_id {
            self.server
                .call("thread/delete", json!({"threadId":thread}))
                .await?;
        }
        change(state, store, |s| s.finish_retirement(owner))?;
        self.loaded.remove(&owner);
        self.turns.remove(&owner);
        self.approvals.retain(|a| a.owner != owner);
        self.final_messages.remove(&owner);
        Ok(())
    }

    pub async fn approve(&mut self, state: &State, accept: bool) -> Result<()> {
        let owner = state.current()?.id;
        let pending: Vec<_> = self.approvals.iter().filter(|a| a.owner == owner).collect();
        ensure!(
            pending.len() == 1,
            "There must be exactly one approval waiting for the current genie."
        );
        let approval = (*pending[0]).clone();
        ensure!(
            self.turns.get(&owner) == Some(&approval.turn),
            "This approval is stale."
        );
        self.server
            .respond(
                approval.id.clone(),
                json!({"decision":if accept {"accept"} else {"decline"}}),
            )
            .await?;
        self.approvals.retain(|a| a.id != approval.id);
        Ok(())
    }

    pub async fn cancel(&mut self, state: &State) -> Result<()> {
        let owner = state.current()?.id;
        let turn = self
            .turns
            .get(&owner)
            .context("This genie has no active work.")?;
        self.server
            .call(
                "turn/interrupt",
                json!({"threadId":state.get(owner)?.thread_id,"turnId":turn}),
            )
            .await?;
        self.turns.remove(&owner);
        if Some(owner) == state.manager {
            self.hr_confirmation = None;
            self.native_tools.clear();
        }
        Ok(())
    }

    pub async fn restore_reply(&mut self, state: &State) -> Result<()> {
        let Ok(genie) = state.current() else {
            return Ok(());
        };
        if self
            .last_reply
            .as_ref()
            .is_some_and(|r| r["genie"] == genie.name)
        {
            return Ok(());
        }
        self.last_reply = None;
        if let Some(update) = state.inbox.iter().rev().find(|m| m.owner == genie.id) {
            let text = self.history_text(update).await?;
            self.last_reply =
                Some(json!({"genie":genie.name,"text":text.chars().take(800).collect::<String>()}));
        }
        Ok(())
    }

    async fn history_text(&mut self, update: &Update) -> Result<String> {
        let response = self
            .server
            .call(
                "thread/read",
                json!({"threadId":update.thread_id,"includeTurns":true}),
            )
            .await?;
        response["thread"]["turns"]
            .as_array()
            .context("History is unavailable. The message remains unread.")?
            .iter()
            .filter(|t| t["id"] == update.turn_id)
            .filter_map(|t| t["items"].as_array())
            .flatten()
            .find(|item| item["id"] == update.item_id)
            .and_then(|item| item["text"].as_str())
            .map(str::to_owned)
            .context("The original message could not be read. It remains unread.")
    }

    pub async fn read_update(&mut self, state: &State, chosen: &str) -> Result<(Update, String)> {
        let owner = state.named(chosen)?.id;
        let update = state
            .unread()
            .find(|m| m.owner == owner && !self.hr_reads.contains(&Delivery::from(*m)))
            .context("There is no unread message from that genie.")?
            .clone();
        let text = self.history_text(&update).await?;
        Ok((update, text))
    }

    pub fn reply_update(
        &mut self,
        state: &mut State,
        store: &Store,
        update: &Update,
        speaker: u64,
        text: &str,
    ) -> Result<()> {
        let delivery = Delivery::from(update);
        let genie = state.get(speaker)?;
        emit(
            json!({"type":"reply","genie":genie.name,"voice":genie.voice,"text":text,"delivery":if self.wait_for_playback {Some(&delivery)} else {None}}),
        )?;
        if self.wait_for_playback {
            self.deliveries.insert(delivery, update.relayed.clone());
        } else {
            change(state, store, |s| {
                mark_delivery(s, &delivery, &update.relayed)
            })?;
        }
        Ok(())
    }

    pub fn acknowledge_playback(
        &mut self,
        state: &mut State,
        store: &Store,
        delivery: Delivery,
    ) -> Result<()> {
        ensure!(
            self.wait_for_playback && self.deliveries.contains_key(&delivery),
            "That playback acknowledgement was not issued by this client."
        );
        let related = &self.deliveries[&delivery];
        change(state, store, |s| mark_delivery(s, &delivery, related))?;
        self.deliveries.remove(&delivery);
        Ok(())
    }

    pub async fn event(&mut self, state: &mut State, store: &Store, message: Value) -> Result<()> {
        // A late RPC response is not a server request, even when its id is no longer pending.
        if message.get("method").is_none() {
            return Ok(());
        }
        let method = message["method"].as_str().unwrap_or("");
        let params = &message["params"];
        if method == "geniewish/tool/call" {
            let id = params["callId"]
                .as_str()
                .context("Missing HR call identity")?;
            let request = self.tool_requests.remove(id).context("Unknown HR call")?;
            let native = self
                .native_tools
                .iter()
                .find(|(_, p)| {
                    p["turnId"].as_str() == self.turns.get(&request.call.owner).map(String::as_str)
                        && p["item"]["tool"] == request.call.tool
                        && p["item"]["arguments"] == request.call.arguments
                })
                .map(|(id, _)| id.clone());
            let result = if let Some(native) = native {
                self.native_tools.remove(&native);
                self.management_call(
                    state,
                    store,
                    request.call.owner,
                    &request.call.tool,
                    request.call.arguments,
                )
                .await
            } else {
                Err(anyhow::anyhow!(
                    "This tool call is not associated with the active HR turn."
                ))
            };
            crate::diagnostics::record(
                "hr.tool_result",
                json!({"tool":request.call.tool,"success":result.is_ok()}),
            );
            let response = match result {
                Ok(value) => {
                    json!({"content":[{"type":"text","text":value.to_string()}],"isError":false})
                }
                Err(error) => {
                    crate::diagnostics::text("hr.tool_error", &format!("{error:#}"));
                    json!({"content":[{"type":"text","text":error.to_string()}],"isError":true})
                }
            };
            self.tool_results.insert(id.to_owned(), response.clone());
            let _ = request.reply.send(response);
            return Ok(());
        }
        if method == "serverRequest/resolved" {
            self.approvals.retain(|a| a.id != params["requestId"]);
            return Ok(());
        }
        let owner = params["threadId"].as_str().and_then(|thread| {
            state
                .genies
                .iter()
                .find(|g| g.thread_id.as_deref() == Some(thread))
                .map(|g| g.id)
        });
        if let Some(request) = message.get("id") {
            if let Some(owner) = owner
                && [
                    "item/commandExecution/requestApproval",
                    "item/fileChange/requestApproval",
                ]
                .contains(&method)
            {
                let turn = params["turnId"]
                    .as_str()
                    .context("Approval has no turn identity.")?
                    .to_owned();
                self.turns.insert(owner, turn.clone());
                self.approvals.push(Approval {
                    id: request.clone(),
                    owner,
                    turn,
                });
                emit(
                    json!({"type":"approval","genie":state.get(owner)?.name,"background":state.active != Some(owner),"command":params["command"],"reason":params["reason"]}),
                )?;
                return Ok(());
            }
            self.server.unsupported(request.clone()).await?;
            return emit(json!({"type":"unsupported_request","method":method}));
        }
        let Some(owner) = owner else {
            return Ok(());
        };
        let name = state.get(owner)?.name.clone();
        if state.manager == Some(owner)
            && method == "item/started"
            && params["item"]["type"] == "mcpToolCall"
            && params["item"]["server"] == "geniewish_hr"
            && let Some(id) = params["item"]["id"].as_str()
        {
            self.native_tools.insert(id.into(), params.clone());
        }
        if method == "item/completed"
            && let Some(id) = params["item"]["id"].as_str()
        {
            self.native_tools.remove(id);
        }
        match method {
            "turn/started" => {
                if let Some(turn) = params["turn"]["id"].as_str() {
                    self.turns.insert(owner, turn.into());
                }
            }
            "item/completed"
                if params["item"]["type"] == "agentMessage"
                    && params["item"]["phase"] != "commentary" =>
            {
                // Keep only the latest non-commentary candidate until the turn returns.
                self.final_messages.insert(owner, params.clone());
            }
            "turn/completed" => {
                self.turns.remove(&owner);
                self.approvals.retain(|a| a.owner != owner);
                if let Some(result) = self.final_messages.remove(&owner)
                    && params["turn"]["status"] == "completed"
                {
                    self.deliver(state, store, owner, &result)?;
                }
                if Some(owner) == state.manager {
                    self.hr_confirmation = None;
                    self.hr_reads.clear();
                    self.native_tools.clear();
                }
                emit(
                    json!({"type":"completed","genie":name,"status":params["turn"]["status"],"error":params["turn"]["error"]}),
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    fn deliver(
        &mut self,
        state: &mut State,
        store: &Store,
        owner: u64,
        params: &Value,
    ) -> Result<()> {
        let item = &params["item"];
        let text = item["text"].as_str().context("Message has no text.")?;
        let update = Update {
            owner,
            thread_id: params["threadId"]
                .as_str()
                .context("Missing thread identity.")?
                .into(),
            turn_id: params["turnId"]
                .as_str()
                .context("Missing turn identity.")?
                .into(),
            item_id: item["id"]
                .as_str()
                .context("Missing message identity.")?
                .into(),
            read: false,
            relayed: if Some(owner) == state.manager {
                self.hr_reads.clone()
            } else {
                vec![]
            },
        };
        if change(state, store, |s| s.record_update(update.clone()))? {
            let genie = state.get(owner)?;
            if state.active == Some(owner) {
                self.last_reply = Some(
                    json!({"genie":genie.name,"text":text.chars().take(800).collect::<String>()}),
                );
                self.reply_update(state, store, &update, owner, text)?;
            } else {
                emit(json!({"type":"update","genie":genie.name,"unread":state.unread().count()}))?;
            }
        }
        Ok(())
    }
}

pub fn emit_team(state: &State) -> Result<()> {
    emit(
        json!({"type":"team","active":state.current().ok().map(|g| &g.name),"genies":state.genies.iter().map(|g| json!({"name":g.name,"role":g.role,"voice":g.voice})).collect::<Vec<_>>()}),
    )
}

fn mark_delivery(state: &mut State, delivery: &Delivery, related: &[Delivery]) -> Result<()> {
    state.mark_read(&delivery.thread_id, &delivery.turn_id, &delivery.item_id)?;
    for item in related {
        // A subsequently retired genie no longer has an inbox entry.
        if state.inbox.iter().any(|m| Delivery::from(m) == *item) {
            state.mark_read(&item.thread_id, &item.turn_id, &item.item_id)?;
        }
    }
    Ok(())
}

use crate::{
    app_server::AppServer,
    state::{State, Update},
    storage::Store,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct Delivery {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
}

impl From<&Update> for Delivery {
    fn from(update: &Update) -> Self {
        Self {
            thread_id: update.thread_id.clone(),
            turn_id: update.turn_id.clone(),
            item_id: update.item_id.clone(),
        }
    }
}

pub struct Runtime {
    pub server: AppServer,
    workspace: PathBuf,
    loaded: HashSet<u64>,
    turns: HashMap<u64, String>,
    approvals: Vec<Approval>,
    final_messages: HashMap<u64, Value>,
    pub wait_for_playback: bool,
    deliveries: HashSet<Delivery>,
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
    pub async fn connect(executable: &str, workspace: PathBuf) -> Result<Self> {
        Ok(Self {
            server: AppServer::connect(executable).await?,
            workspace: workspace.canonicalize()?,
            loaded: HashSet::new(),
            turns: HashMap::new(),
            approvals: vec![],
            final_messages: HashMap::new(),
            wait_for_playback: false,
            deliveries: HashSet::new(),
            routing_config: None,
            routing_clarification: None,
        })
    }

    pub async fn ensure_thread(
        &mut self,
        state: &mut State,
        store: &Store,
        owner: u64,
    ) -> Result<String> {
        let genie = state.get(owner)?.clone();
        if let Some(thread) = &genie.thread_id {
            if !self.loaded.contains(&owner) {
                let response = self.server.call("thread/resume", json!({"threadId":thread,"config":{"memories.use_memories":false,"memories.generate_memories":false}})).await?;
                ensure!(
                    response["thread"]["id"].as_str() == Some(thread),
                    "Resume returned a different conversation; refusing to replace it."
                );
                if let Some(turns) = response["thread"]["turns"].as_array() {
                    for turn in turns.iter().filter(|t| t["status"] == "inProgress") {
                        if let Some(id) = turn["id"].as_str() {
                            self.turns.insert(owner, id.into());
                        }
                    }
                }
                self.loaded.insert(owner);
            }
            return Ok(thread.clone());
        }
        let response = self.server.call("thread/start", json!({
            "cwd":self.workspace,"ephemeral":false,"approvalPolicy":"on-request","sandbox":"workspace-write",
            "config":{"memories.use_memories":false,"memories.generate_memories":false},
            "developerInstructions":format!("Your name is {}. Refer to the user as {}. Human's current role assignment accompanies each message; it can change or be blank. A blank role means no specialized role was assigned. Keep spoken replies concise. This is your ongoing conversation. Use your available tools for work. Roster and speaker changes are handled by the client. Do not claim to hire or retire genies: those operations belong to the client. Do not provide intermediate progress messages; give your result when the work finishes.", genie.name, crate::HUMAN_NAME)
        })).await?;
        let thread = response["thread"]["id"]
            .as_str()
            .context("Runtime did not return a thread identity.")?
            .to_owned();
        if let Err(error) = change(state, store, |s| s.bind_thread(owner, &thread)) {
            let cleanup = self
                .server
                .call("thread/delete", json!({"threadId":thread}))
                .await;
            return Err(error.context(format!(
                "The request was not sent. Unbound thread: {thread}; cleanup succeeded: {}",
                cleanup.is_ok()
            )));
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
        let owner = state.current()?.id;
        crate::diagnostics::record(
            "work.destination",
            json!({"genie":state.get(owner)?.name,"busy":self.busy(owner)}),
        );
        crate::diagnostics::text("work.input", text);
        let thread = self.ensure_thread(state, store, owner).await?;
        let role = &state.get(owner)?.role;
        let input = json!([{ "type":"text", "text":format!("Human's current role assignment for you: {}.", if role.is_empty() {"none"} else {role}) }, {"type":"text","text":text}]);
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
        let owner = state.confirmed_retirement(chosen)?;
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
        Ok(())
    }

    pub async fn read_update(&mut self, state: &State, chosen: &str) -> Result<(Update, String)> {
        let owner = state.named(chosen)?.id;
        let update = state
            .unread()
            .find(|m| m.owner == owner)
            .context("There is no unread message from that genie.")?
            .clone();
        let response = self
            .server
            .call(
                "thread/read",
                json!({"threadId":update.thread_id,"includeTurns":true}),
            )
            .await?;
        let turns = response["thread"]["turns"]
            .as_array()
            .context("History is unavailable. The message remains unread.")?;
        let text = turns
            .iter()
            .filter(|t| t["id"] == update.turn_id)
            .filter_map(|t| t["items"].as_array())
            .flatten()
            .find(|item| item["id"] == update.item_id)
            .and_then(|item| item["text"].as_str())
            .context("The original message could not be read. It remains unread.")?
            .to_owned();
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
            self.deliveries.insert(delivery);
        } else {
            change(state, store, |s| {
                s.mark_read(&delivery.thread_id, &delivery.turn_id, &delivery.item_id)
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
            self.wait_for_playback && self.deliveries.contains(&delivery),
            "That playback acknowledgement was not issued by this client."
        );
        change(state, store, |s| {
            s.mark_read(&delivery.thread_id, &delivery.turn_id, &delivery.item_id)
        })?;
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
        };
        if change(state, store, |s| s.record_update(update.clone()))? {
            let genie = state.get(owner)?;
            if state.active == Some(owner) {
                self.reply_update(state, store, &update, owner, text)?;
            } else {
                emit(json!({"type":"update","genie":genie.name,"unread":state.unread().count()}))?;
            }
        }
        Ok(())
    }
}

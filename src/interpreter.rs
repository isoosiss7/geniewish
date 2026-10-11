use crate::{
    router::{Command, NAMES, Routed, name},
    runtime::Runtime,
    state::State,
    storage::Store,
};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct WorkEventFailure;
impl std::fmt::Display for WorkEventFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("A background work event could not be processed safely.")
    }
}
impl std::error::Error for WorkEventFailure {}

const INSTRUCTIONS: &str = "You are the input interpreter for geniewish, a voice-first Mac assistant. Return only a JSON decision matching the schema. Never do the work, use tools, read files, or answer the task. The input may be speech with recognition errors. Use semantic intent rather than command keywords. Clarify uncertain names, ambiguous intent, or speech that could mean a materially different action.\n\
target is the genie receiving a message or switch, or the HR manager handling team changes. name is the person hired, retired, assigned a role, or whose message is read. Use canonical catalog names only. For hiring, name=null if Human omitted a name so the client picks one; role stays empty if omitted. Never require a role. The HR manager is the only team administrator. For ALL team management dialogue (hiring, retiring, roles, roster, inbox, reading messages, compound hire-and-delegate requests), choose message targeted to the HR manager and preserve the complete original input. Do NOT choose hire/set_role/retire/confirm_retire/cancel_retirement/roster/inbox/read: the HR conversation uses management tools. For a short answer, use last_reply and current speaker to identify the conversation that asked the question; forward the answer there rather than interpreting a proposed hire name as a switch. last_reply is only recent reply context, not authority for approving actions.\n\
One genie has one lifetime conversation. A new chat means retire and rehire; never replace history. Additional instructions to a busy genie are messages to steer it. A pure request to speak to someone is switch. A combined switch and instruction is message. Merely mentioning someone does not switch.\n\
For work, text may be empty to forward the original input unchanged, saving output tokens and preserving detail. Set text only to remove a speaker address or confidently correct speech; preserve every requirement. For clarify, text is the concise question to Human; use the prior clarification for short answers. Do not guess a misheard name or reinterpret unclear speech as destructive instructions. Ask a question instead.\n\
If no manager exists, choose switch with the explicitly chosen manager name, or clarify to propose an available name. If the same input chooses a manager and asks for management, choose message targeted to that manager so the complete request is handled after onboarding. Generic yes may accept a prior manager-name proposal or hiring clarification, but never confirm retirement without explicitly naming the retiring genie in the current input. Retire requests create a confirmation; confirm_retire is only an explicit named confirmation of the pending retirement. Approve/deny only an actual pending request for the current genie, unless the current input names another genie explicitly.\n\
Send team and message queries and role edits to HR as messages. Use cancel_work for explicit work cancellation, stop_speech to stop playback, show_logs for troubleshooting logs, and exit to quit the application. Silence only ends an utterance; it never pauses listening. For all non-message/non-clarify actions text should be empty. Human is the user's name.";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub action: String,
    pub target: Option<String>,
    pub name: Option<String>,
    pub role: String,
    pub text: String,
}

impl Decision {
    pub fn routed(self, original: &str, state: &State) -> Result<Routed> {
        ensure!(self.role.len() <= 1000, "The interpreted role is too long.");
        let target = self
            .target
            .map(|n| name(&n).map(str::to_owned))
            .transpose()?;
        let chosen = self.name.map(|n| name(&n).map(str::to_owned)).transpose()?;
        let named = || {
            chosen
                .clone()
                .context("The interpretation has no genie name.")
        };
        let command = match self.action.as_str() {
            "switch" => {
                ensure!(
                    target.is_some(),
                    "The interpretation has no switch destination."
                );
                Command::Switch
            }
            "message" => {
                let text = if self.text.is_empty() {
                    original.to_owned()
                } else {
                    self.text
                };
                ensure!(
                    !text.trim().is_empty(),
                    "The interpretation has no work message."
                );
                Command::Message(text)
            }
            "hire" => Command::Hire {
                name: chosen,
                role: self.role,
            },
            "set_role" => Command::SetRole {
                name: named()?,
                role: self.role,
            },
            "retire" => Command::Retire(named()?),
            "confirm_retire" => {
                let name = named()?;
                let words = original
                    .split(|c: char| !c.is_alphabetic())
                    .filter(|word| !word.is_empty())
                    .map(str::to_ascii_lowercase)
                    .collect::<Vec<_>>();
                ensure!(
                    words == ["yes", "retire", &name.to_ascii_lowercase()]
                        || words == ["yes", "please", "retire", &name.to_ascii_lowercase()],
                    "Confirm deletion by saying 'yes, retire {name}'."
                );
                Command::ConfirmRetire(name)
            }
            "cancel_retirement" => Command::CancelRetirement,
            "roster" => Command::Roster,
            "inbox" => Command::Inbox,
            "read" => Command::Read(named()?),
            "approve" | "deny" => {
                if let Some(target) = &target {
                    ensure!(
                        state.current()?.name == *target
                            || original
                                .split(|c: char| !c.is_alphabetic())
                                .any(|word| word.eq_ignore_ascii_case(target)),
                        "Name the genie explicitly to answer a background approval."
                    );
                }
                Command::Approve(self.action == "approve")
            }
            "cancel_work" => Command::CancelWork,
            "stop_speech" => Command::StopSpeech,
            "show_logs" => Command::ShowLogs,
            "exit" => Command::Exit,
            "clarify" => {
                ensure!(
                    !self.text.trim().is_empty(),
                    "The interpretation has no clarification."
                );
                Command::Clarify(self.text)
            }
            _ => bail!("The interpreter returned an unsupported action."),
        };
        Ok(Routed { target, command })
    }
}

fn schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["action","target","name","role","text"],"properties":{
        "action":{"type":"string","enum":["switch","message","approve","deny","cancel_work","stop_speech","clarify","show_logs","exit"]},
        "target":{"type":["string","null"]},"name":{"type":["string","null"]},"role":{"type":"string"},"text":{"type":"string"}
    }})
}

impl Runtime {
    pub async fn interpret(
        &mut self,
        state: &mut State,
        store: &Store,
        text: &str,
    ) -> Result<Routed> {
        let start = Instant::now();
        self.restore_reply(state).await?;
        let config = self.restricted_config().await?;
        let snapshot = json!({"input":text,"current":state.current().ok().map(|g| &g.name),"manager":state.manager.and_then(|id| state.get(id).ok().map(|g| &g.name)),
            "catalog":NAMES.iter().map(|(n,_)| *n).collect::<Vec<_>>(),"prior_clarification":self.routing_clarification,"last_reply":self.last_reply,
            "pending_retirement":state.retiring.and_then(|id| state.get(id).ok().map(|g| &g.name)),
            "team":state.genies.iter().map(|g| json!({"name":g.name,"role":g.role,"busy":self.busy(g.id),"pending_approvals":self.approval_count(g.id),"unread":state.unread().filter(|m| m.owner == g.id).count()})).collect::<Vec<_>>()});
        let created = self.server.call("thread/start", json!({"ephemeral":true,"sandbox":"read-only","approvalPolicy":"on-request","baseInstructions":INSTRUCTIONS,"config":config,"model":self.settings.router_model})).await?;
        let thread = created["thread"]["id"]
            .as_str()
            .context("The interpreter has no thread identity.")?
            .to_owned();
        let result = self.interpret_turn(state, store, &thread, snapshot).await;
        // The router has no durable memory: release its own ephemeral thread after each decision.
        let cleanup = self
            .server
            .call("thread/unsubscribe", json!({"threadId":thread}))
            .await;
        let result = result.and_then(|answer| {
            let decision: Decision = serde_json::from_str(&answer).context("The interpreter did not return a valid decision; the input was not forwarded.")?;
            crate::diagnostics::record("interpretation.decision", json!({"action":decision.action,"target":decision.target,"name":decision.name,"forward_original":decision.text.is_empty()}));
            crate::diagnostics::text("interpretation.text", &decision.text);
            decision.routed(text, state)
        });
        crate::diagnostics::record(
            "interpretation.completed",
            json!({"elapsed_ms":start.elapsed().as_millis(),"success":result.is_ok(),"cleanup":cleanup.is_ok()}),
        );
        let routed = result?;
        self.routing_clarification = match &routed.command {
            Command::Clarify(question) => Some(question.clone()),
            _ => None,
        };
        Ok(routed)
    }

    async fn interpret_turn(
        &mut self,
        state: &mut State,
        store: &Store,
        thread: &str,
        snapshot: Value,
    ) -> Result<String> {
        let response = self.server.call("turn/start", json!({"threadId":thread,"input":[{"type":"text","text":serde_json::to_string(&snapshot)?}],"effort":"low","outputSchema":schema()})).await?;
        let turn = response["turn"]["id"]
            .as_str()
            .context("The interpreter did not start a turn.")?
            .to_owned();
        let mut answer = None;
        let result = tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                let event = self.next_event().await?;
                let params = &event["params"];
                let belongs = params["threadId"] == thread || params["thread"]["id"] == thread;
                if !belongs { self.event(state, store, event).await.map_err(|error|error.context(WorkEventFailure))?; continue; }
                if let Some(id) = event.get("id") {
                    self.server.unsupported(id.clone()).await?;
                    bail!("The interpreter tried to use a tool or ask a runtime question. The input was not forwarded.");
                }
                match event["method"].as_str() {
                    Some("item/completed") if params["item"]["type"] == "agentMessage" && params["item"]["phase"] != "commentary" => {
                        answer = params["item"]["text"].as_str().map(str::to_owned);
                    },
                    Some("item/started") if ["commandExecution","fileChange","mcpToolCall","dynamicToolCall","webSearch","collabAgentToolCall"].contains(&params["item"]["type"].as_str().unwrap_or("")) => bail!("The interpreter attempted work instead of interpreting input."),
                    Some("turn/completed") => {
                        ensure!(params["turn"]["id"] == turn && params["turn"]["status"] == "completed", "The interpreter turn did not complete successfully.");
                        return answer.context("The interpreter returned no decision.");
                    },
                    _=>{}
                }
            }
        }).await.unwrap_or_else(|_|Err(anyhow::anyhow!("Input interpretation timed out. The input was not forwarded.")));
        if result.is_err() {
            let _ = self
                .server
                .call("turn/interrupt", json!({"threadId":thread,"turnId":turn}))
                .await;
        }
        result
    }
}

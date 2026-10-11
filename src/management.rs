use crate::{
    router::{NAMES, name},
    runtime::{Runtime, change},
    state::State,
    storage::Store,
};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Action {
    action: String,
    name: Option<String>,
    role: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    filename: String,
    content: String,
}

// Tool ownership is HR's identity, independent of the current foreground speaker.
fn manager_change<T>(
    state: &mut State,
    store: &Store,
    operation: impl FnOnce(&mut State) -> Result<T>,
) -> Result<T> {
    change(state, store, |s| {
        let foreground = s.active;
        s.active = s.manager;
        let result = operation(s);
        s.active = foreground;
        result
    })
}

pub fn named_confirmation(input: &str, chosen: &str) -> bool {
    let words = input
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    words == ["yes", "retire", &chosen.to_ascii_lowercase()]
        || words == ["yes", "please", "retire", &chosen.to_ascii_lowercase()]
}

impl Runtime {
    pub fn team_snapshot(&self, state: &State) -> Value {
        json!({"manager":state.manager.and_then(|id| state.get(id).ok().map(|g| &g.name)),"active":state.current().ok().map(|g| &g.name),
            "available_names":NAMES.iter().filter(|(n,_)| state.named(n).is_err()).map(|(n,_)| *n).collect::<Vec<_>>(),
            "pending_retirement":state.retiring.and_then(|id| state.get(id).ok().map(|g| &g.name)),
            "unread":state.unread().count(),"genies":state.genies.iter().map(|g| json!({"name":g.name,"role":g.role,"busy":self.busy(g.id),"pending_approvals":self.approval_count(g.id),"unread":state.unread().filter(|m| m.owner==g.id).count()})).collect::<Vec<_>>()})
    }

    pub async fn management_call(
        &mut self,
        state: &mut State,
        store: &Store,
        owner: u64,
        tool: &str,
        arguments: Value,
    ) -> Result<Value> {
        ensure!(
            Some(owner) == state.manager && self.busy(owner),
            "Management tools belong to the active HR turn."
        );
        if tool == "save_report" {
            let report: Report = serde_json::from_value(arguments)?;
            let path = save_report(&self.settings.output_dir, &report.filename, &report.content)?;
            return Ok(json!({"filename":path.file_name().and_then(|n| n.to_str()),"path":path}));
        }
        ensure!(tool == "manage_genies", "Unknown geniewish tool.");
        let action: Action = serde_json::from_value(arguments)?;
        ensure!(
            action.role.len() <= 1000 && action.text.len() <= 128 * 1024,
            "The management request is too large."
        );
        let chosen = action.name.as_deref().map(name).transpose()?;
        let required = || chosen.context("This action requires a genie name.");
        let mut result = json!({});
        match action.action.as_str() {
            "hire" => {
                let chosen = chosen
                    .or_else(|| {
                        NAMES
                            .iter()
                            .map(|(n, _)| *n)
                            .find(|n| state.named(n).is_err())
                    })
                    .context("All names are in use. Retire a genie to free a name.")?;
                manager_change(state, store, |s| s.hire(chosen, &action.role))?;
                result = json!({"hired":chosen,"role":state.named(chosen)?.role});
            }
            "set_role" => {
                manager_change(state, store, |s| s.set_role(required()?, &action.role))?;
            }
            "retire" => {
                let chosen = required()?;
                manager_change(state, store, |s| s.request_retirement(chosen))?;
                result = json!({"confirmation_required":format!("Say 'yes, retire {chosen}' to delete their conversation. Project files remain.")});
            }
            "confirm_retire" => {
                let chosen = required()?;
                ensure!(
                    self.hr_confirmation.as_deref() == Some(chosen),
                    "Human must explicitly say 'yes, retire {chosen}' for the pending retirement."
                );
                // Retire uses the shared lifetime deletion path, with manager authorization.
                self.retire(state, store, chosen).await?;
                self.hr_confirmation = None;
                result = json!({"retired":chosen});
            }
            "cancel_retirement" => {
                manager_change(state, store, |s| {
                    s.retiring = None;
                    Ok(())
                })?;
                self.hr_confirmation = None;
            }
            "roster" | "inbox" => {}
            "switch" => {
                ensure!(
                    state.active == state.manager,
                    "Human has already switched away from HR; do not change the foreground from a background HR turn."
                );
                let genie = state.named(required()?)?.clone();
                if genie.thread_id.is_some() {
                    self.ensure_thread(state, store, genie.id).await?;
                }
                change(state, store, |s| s.switch(&genie.name))?;
                crate::runtime::emit(
                    json!({"type":"reply","genie":genie.name,"voice":genie.voice,"text":if self.busy(genie.id) {format!("{} is busy but listening.",genie.name)} else {format!("{} here.",genie.name)}}),
                )?;
                result = json!({"active":state.current()?.name});
            }
            "message" => {
                let target = state.named(required()?)?.id;
                ensure!(
                    Some(target) != state.manager,
                    "Delegate work to a work genie, not HR itself."
                );
                ensure!(
                    !action.text.trim().is_empty(),
                    "The delegated message is empty."
                );
                let steered = self.submit_for(state, store, target, &action.text).await?;
                result = json!({"sent_to":state.get(target)?.name,"steered":steered});
            }
            "read" => {
                let (update, text) = self.read_update(state, required()?).await?;
                self.hr_reads.push(crate::runtime::Delivery::from(&update));
                self.hr_reads.extend(update.relayed.clone());
                let mut unique = std::collections::HashSet::new();
                self.hr_reads.retain(|d| unique.insert(d.clone()));
                result = json!({"from":required()?,"message":text});
            }
            _ => bail!("Unsupported management action."),
        }
        crate::runtime::emit_team(state)?;
        Ok(json!({"result":result,"team":self.team_snapshot(state)}))
    }
}

pub fn save_report(directory: &Path, filename: &str, content: &str) -> Result<std::path::PathBuf> {
    ensure!(
        !filename.is_empty()
            && filename.len() <= 120
            && filename
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            && !filename.starts_with('.'),
        "Choose a simple report filename, without directories."
    );
    ensure!(
        !content.is_empty() && content.len() <= 128 * 1024,
        "Report content is empty or too large."
    );
    let file = Path::new(filename);
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .context("Invalid report filename.")?;
    let extension = file.extension().and_then(|s| s.to_str()).unwrap_or("txt");
    for counter in 0..1000 {
        let path = directory.join(if counter == 0 {
            filename.to_owned()
        } else {
            format!("{stem}-{counter}.{extension}")
        });
        let mut output = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        output.write_all(content.as_bytes())?;
        output.sync_all()?;
        return Ok(path);
    }
    bail!("No unique report filename is available.")
}

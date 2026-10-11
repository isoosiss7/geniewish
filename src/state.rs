use crate::router::{NAMES, name};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Genie {
    pub id: u64,
    pub name: String,
    pub role: String,
    pub voice: String,
    pub thread_id: Option<String>,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Update {
    pub owner: u64,
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub read: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relayed: Vec<Delivery>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    pub next_id: u64,
    pub manager: Option<u64>,
    pub active: Option<u64>,
    pub retiring: Option<u64>,
    pub genies: Vec<Genie>,
    pub inbox: Vec<Update>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            next_id: 1,
            manager: None,
            active: None,
            retiring: None,
            genies: vec![],
            inbox: vec![],
        }
    }
}

impl State {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.next_id > 0, "Invalid identity counter.");
        ensure!(
            self.version == 1,
            "Unsupported state version; do not reset your saved identities."
        );
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        let mut threads = HashSet::new();
        for g in &self.genies {
            ensure!(
                g.id > 0 && g.id < self.next_id && ids.insert(g.id),
                "Invalid genie identity."
            );
            ensure!(
                name(&g.name)? == g.name && names.insert(&g.name),
                "Invalid or duplicate name."
            );
            ensure!(g.role.len() <= 1000, "Genie role is too long.");
            if let Some(thread) = &g.thread_id {
                ensure!(
                    !thread.is_empty() && threads.insert(thread),
                    "Invalid or shared thread."
                );
            }
        }
        ensure!(
            self.genies.is_empty() == self.manager.is_none(),
            "Invalid manager mapping."
        );
        ensure!(
            self.genies.is_empty() == self.active.is_none(),
            "Invalid foreground mapping."
        );
        for id in [self.manager, self.active, self.retiring]
            .into_iter()
            .flatten()
        {
            ensure!(ids.contains(&id), "Saved identity no longer exists.");
        }
        ensure!(
            self.retiring.is_none() || self.retiring != self.manager,
            "The manager cannot retire itself."
        );
        for m in &self.inbox {
            ensure!(
                self.get(m.owner)?.thread_id.as_deref() == Some(&m.thread_id),
                "Update belongs to a different conversation."
            );
        }
        Ok(())
    }

    pub fn get(&self, id: u64) -> Result<&Genie> {
        self.genies
            .iter()
            .find(|g| g.id == id)
            .context("Genie no longer exists.")
    }
    pub fn named(&self, input: &str) -> Result<&Genie> {
        self.genies
            .iter()
            .find(|g| g.name.eq_ignore_ascii_case(input))
            .context("That genie is not on your team.")
    }
    pub fn current(&self) -> Result<&Genie> {
        self.get(
            self.active
                .context("First choose your HR manager's name.")?,
        )
    }

    pub fn hire(&mut self, chosen: &str, role: &str) -> Result<u64> {
        let chosen = name(chosen)?;
        ensure!(role.len() <= 1000, "Please give the genie a short role.");
        ensure!(
            !self.genies.iter().any(|g| g.name == chosen),
            "{chosen} is already hired."
        );
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .context("Identity counter exhausted.")?;
        let voice = NAMES
            .iter()
            .find(|(n, _)| *n == chosen)
            .context("Unknown name.")?
            .1;
        self.genies.push(Genie {
            id,
            name: chosen.into(),
            role: role.trim().into(),
            voice: voice.into(),
            thread_id: None,
        });
        if self.manager.is_none() {
            self.manager = Some(id);
            self.active = Some(id);
        }
        Ok(id)
    }

    pub fn set_role(&mut self, chosen: &str, role: &str) -> Result<()> {
        self.require_manager()?;
        ensure!(role.len() <= 1000, "Please give the genie a short role.");
        let id = self.named(chosen)?.id;
        ensure!(Some(id) != self.manager, "The manager keeps its HR role.");
        self.genies
            .iter_mut()
            .find(|g| g.id == id)
            .context("Genie no longer exists.")?
            .role = role.trim().into();
        Ok(())
    }

    pub fn bind_thread(&mut self, id: u64, thread: &str) -> Result<()> {
        ensure!(!thread.is_empty(), "Missing runtime thread identity.");
        ensure!(
            !self
                .genies
                .iter()
                .any(|g| g.id != id && g.thread_id.as_deref() == Some(thread)),
            "Thread already belongs to another genie."
        );
        let g = self
            .genies
            .iter_mut()
            .find(|g| g.id == id)
            .context("Genie no longer exists.")?;
        if let Some(existing) = &g.thread_id {
            ensure!(
                existing == thread,
                "A lifetime conversation cannot be replaced."
            );
        }
        g.thread_id = Some(thread.into());
        Ok(())
    }

    pub fn switch(&mut self, chosen: &str) -> Result<u64> {
        let id = self.named(chosen)?.id;
        self.active = Some(id);
        Ok(id)
    }
    pub fn require_manager(&self) -> Result<()> {
        ensure!(
            self.active.is_some() && self.active == self.manager,
            "Ask your HR manager to manage the team."
        );
        Ok(())
    }
    pub fn request_retirement(&mut self, chosen: &str) -> Result<u64> {
        self.require_manager()?;
        let id = self.named(chosen)?.id;
        ensure!(
            Some(id) != self.manager,
            "A replacement HR manager is required first."
        );
        self.retiring = Some(id);
        Ok(id)
    }
    pub fn confirmed_retirement(&self, chosen: &str) -> Result<u64> {
        self.require_manager()?;
        let id = self.named(chosen)?.id;
        ensure!(
            self.retiring == Some(id),
            "There is no matching retirement waiting for confirmation."
        );
        Ok(id)
    }
    /// Call only after the runtime has successfully deleted any bound thread.
    pub fn finish_retirement(&mut self, id: u64) -> Result<()> {
        ensure!(
            self.retiring == Some(id) && Some(id) != self.manager,
            "Retirement identity changed."
        );
        if self.active == Some(id) {
            self.active = self.manager;
        }
        self.genies.retain(|g| g.id != id);
        self.inbox.retain(|m| m.owner != id);
        self.retiring = None;
        Ok(())
    }
    pub fn record_update(&mut self, update: Update) -> Result<bool> {
        ensure!(
            self.get(update.owner)?.thread_id.as_deref() == Some(&update.thread_id),
            "Update owner does not match its thread."
        );
        if self.inbox.iter().any(|m| {
            m.thread_id == update.thread_id
                && m.turn_id == update.turn_id
                && m.item_id == update.item_id
        }) {
            return Ok(false);
        }
        self.inbox.push(update);
        Ok(true)
    }
    pub fn unread(&self) -> impl Iterator<Item = &Update> {
        self.inbox.iter().filter(|m| !m.read)
    }
    pub fn mark_read(&mut self, thread: &str, turn: &str, item: &str) -> Result<()> {
        let update = self
            .inbox
            .iter_mut()
            .find(|m| m.thread_id == thread && m.turn_id == turn && m.item_id == item);
        match update {
            Some(update) => {
                update.read = true;
                Ok(())
            }
            None => bail!("That update no longer exists."),
        }
    }
}

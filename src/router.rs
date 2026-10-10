use anyhow::{Result, bail};

pub const NAMES: &[(&str, &str)] = &[
    ("Mike", "Daniel"),
    ("Lucas", "Eddy (English (UK))"),
    ("Annie", "Samantha"),
    ("Ben", "Fred"),
    ("Clara", "Karen"),
    ("Allan", "Reed (English (US))"),
    ("Oscar", "Ralph"),
    ("Ruby", "Tessa"),
    ("Zoe", "Moira"),
    ("Hugo", "Eddy (English (US))"),
];

pub fn name(input: &str) -> Result<&'static str> {
    NAMES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(input.trim()))
        .map(|(n, _)| *n)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Choose a name from: {}",
                NAMES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
            )
        })
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Switch,
    Hire { name: Option<String>, role: String },
    SetRole { name: String, role: String },
    Retire(String),
    ConfirmRetire(String),
    CancelRetirement,
    Roster,
    Inbox,
    Read(String),
    Approve(bool),
    CancelWork,
    StopSpeech,
    Message(String),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Routed {
    pub target: Option<String>,
    pub command: Command,
}

pub fn route(input: &str) -> Result<Routed> {
    let text = input.trim().trim_end_matches(['.', '?', '!']).trim();
    let lower = text.to_ascii_lowercase();
    let text = ["hi ", "hello ", "hey "]
        .iter()
        .find(|prefix| lower.starts_with(**prefix))
        .map_or(text, |prefix| &text[prefix.len()..]);
    if text.is_empty() {
        bail!("Please say a command.");
    }
    let lower = text.to_ascii_lowercase();
    let mut addressed = text;
    let mut explicit = false;
    for prefix in [
        "i want to speak to ",
        "i want to talk to ",
        "speak to ",
        "talk to ",
        "switch to ",
        "back to ",
    ] {
        if lower.starts_with(prefix) {
            addressed = &text[prefix.len()..];
            explicit = true;
            break;
        }
    }
    let lower = addressed.to_ascii_lowercase();
    let mut target = None;
    let mut body = addressed;
    for (n, _) in NAMES {
        if addressed.eq_ignore_ascii_case(n) {
            return Ok(Routed {
                target: Some((*n).into()),
                command: Command::Switch,
            });
        }
        if lower.starts_with(&n.to_ascii_lowercase()) {
            let rest = &addressed[n.len()..];
            let next = rest.trim_start_matches([',', ' ']);
            let verb = next
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            let direct = [
                "please", "can", "could", "would", "build", "help", "explain", "approve", "deny",
                "hire", "retire", "read", "what", "who", "tell", "stop", "cancel", "remember",
                "any", "yes", "pls", "set", "change", "clear",
            ]
            .contains(&verb.as_str());
            if rest.starts_with(',') || (rest.starts_with(' ') && (direct || explicit)) {
                target = Some((*n).into());
                body = next;
                break;
            }
        }
    }
    if explicit && target.is_none() {
        bail!("Choose a genie name from your team; that address was not recognized.");
    }
    if body.is_empty() && target.is_some() {
        return Ok(Routed {
            target,
            command: Command::Switch,
        });
    }
    let lower = body.to_ascii_lowercase();
    let body = ["please ", "pls "]
        .iter()
        .find(|prefix| lower.starts_with(**prefix))
        .map_or(body, |prefix| &body[prefix.len()..]);
    let lower = body.to_ascii_lowercase();
    let command = if let Some(rest) = lower.strip_prefix("hire ") {
        let (chosen, role) = match rest.find(" as ") {
            Some(split) => (
                &rest[..split],
                body[5 + split + 4..]
                    .trim()
                    .trim_start_matches("my ")
                    .trim(),
            ),
            None => (rest, ""),
        };
        if role.len() > 1000 {
            bail!("Please give the genie a short role.");
        }
        let chosen = chosen.trim().trim_end_matches(" for me").trim();
        let unnamed = [
            "a genie",
            "a new genie",
            "new genie",
            "a genie for me",
            "a new genie for me",
        ]
        .contains(&chosen);
        let chosen = ["call her ", "call him ", "call them ", "call it ", "named "]
            .iter()
            .find_map(|prefix| chosen.split_once(prefix).map(|(_, name)| name))
            .unwrap_or(chosen);
        Command::Hire {
            name: if unnamed {
                None
            } else {
                Some(name(chosen)?.into())
            },
            role: role.into(),
        }
    } else if lower.starts_with("set ") || lower.starts_with("change ") {
        let offset = if lower.starts_with("set ") { 4 } else { 7 };
        if let Some(split) = lower[offset..].find("'s role to ") {
            let role = body[offset + split + 11..]
                .trim()
                .trim_start_matches("my ")
                .trim();
            Command::SetRole {
                name: name(&body[offset..offset + split])?.into(),
                role: role.into(),
            }
        } else {
            Command::Message(body.into())
        }
    } else if lower.starts_with("clear ") && lower.ends_with("'s role") {
        Command::SetRole {
            name: name(&body[6..body.len() - 7])?.into(),
            role: String::new(),
        }
    } else if let Some(rest) = lower
        .strip_prefix("yes, retire ")
        .or_else(|| lower.strip_prefix("yes retire "))
    {
        Command::ConfirmRetire(name(rest)?.into())
    } else if let Some(rest) = lower
        .strip_prefix("retire ")
        .or_else(|| lower.strip_prefix("fire "))
    {
        Command::Retire(name(rest)?.into())
    } else if ["cancel retirement", "never mind"].contains(&lower.as_str()) {
        Command::CancelRetirement
    } else if ["who is on my team", "list genies", "my team"].contains(&lower.as_str()) {
        Command::Roster
    } else if ["any messages", "do i have any messages", "check messages"].contains(&lower.as_str())
    {
        Command::Inbox
    } else if lower.starts_with("read ") && lower.ends_with("'s message") {
        Command::Read(name(&body[5..body.len() - 10])?.into())
    } else if lower == "approve once" {
        Command::Approve(true)
    } else if lower == "deny" {
        Command::Approve(false)
    } else if lower == "cancel your work" {
        Command::CancelWork
    } else if lower == "stop talking" {
        Command::StopSpeech
    } else {
        Command::Message(body.into())
    };
    Ok(Routed { target, command })
}

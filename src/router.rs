use anyhow::{Result, bail};

pub const NAMES: &[(&str, &str)] = &[
    ("Mike", "Daniel"),
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
    Hire { name: String, role: String },
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
                "any", "yes",
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
    let body = body
        .strip_prefix("Please ")
        .or_else(|| body.strip_prefix("please "))
        .unwrap_or(body);
    let lower = body.to_ascii_lowercase();
    let command = if let Some(rest) = lower.strip_prefix("hire ") {
        let Some((chosen, _)) = rest.split_once(" as ") else {
            bail!("Say 'hire Annie as my product manager'.");
        };
        let split = rest.find(" as ").expect("split was checked");
        let role = body[5 + split + 4..]
            .trim()
            .trim_start_matches("my ")
            .trim();
        if role.is_empty() || role.len() > 1000 {
            bail!("Please give the genie a short role.");
        }
        Command::Hire {
            name: name(chosen)?.into(),
            role: role.into(),
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

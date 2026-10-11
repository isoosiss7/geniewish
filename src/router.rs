use anyhow::Result;

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
    Approve(bool),
    CancelWork,
    StopSpeech,
    Clarify(String),
    ShowLogs,
    Exit,
    Message(String),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Routed {
    pub target: Option<String>,
    pub command: Command,
}

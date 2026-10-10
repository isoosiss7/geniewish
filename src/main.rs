use anyhow::Result;
use geniewish::{
    router::{Command, route},
    state::State,
    storage::Store,
};
use std::{
    io::{self, BufRead},
    path::PathBuf,
};

fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--help") {
        println!(
            "geniewish\n\nRun from your workspace. Local state is saved in .geniewish/.\nFirst say a manager name, for example Mike.\nCommands: Mike, hire Annie as my product manager; talk to Annie;\nMike, retire Annie; yes, retire Annie.\nThis checkpoint handles local commands; work integration follows."
        );
        return Ok(());
    }
    let directory = PathBuf::from(".geniewish");
    let store = Store::open(&directory)?;
    let mut state = store.load()?;
    println!("Choose your HR manager's name, or continue with your saved team.");
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line == "quit" {
            break;
        }
        let before = state.clone();
        match handle(&mut state, &line).and_then(|reply| {
            store.save(&state)?;
            Ok(reply)
        }) {
            Ok(reply) => println!("{reply}"),
            Err(error) => {
                state = before;
                eprintln!("{error:#}");
            }
        }
    }
    Ok(())
}

fn handle(state: &mut State, input: &str) -> Result<String> {
    let routed = route(input)?;
    if state.manager.is_none() {
        anyhow::ensure!(
            routed.command == Command::Switch,
            "Choose a manager name first, for example Mike."
        );
        let chosen = routed.target.as_deref().expect("A switch has a target");
        state.hire(chosen, "HR manager")?;
        return Ok(format!("{chosen} here. I am your HR manager."));
    }
    if let Some(name) = routed.target {
        state.switch(&name)?;
    }
    match routed.command {
        Command::Switch => Ok(format!("{} here.", state.current()?.name)),
        Command::Hire { name, role } => {
            state.require_manager()?;
            state.hire(&name, &role)?;
            Ok(format!("{name} is ready as your {role}."))
        }
        Command::Retire(name) => {
            state.request_retirement(&name)?;
            Ok(format!(
                "Say 'yes, retire {name}' to delete their conversation. Project files remain."
            ))
        }
        Command::ConfirmRetire(name) => {
            let id = state.confirmed_retirement(&name)?;
            anyhow::ensure!(
                state.get(id)?.thread_id.is_none(),
                "Runtime deletion is required first."
            );
            state.finish_retirement(id)?;
            Ok(format!("{name} is retired."))
        }
        Command::CancelRetirement => {
            state.retiring = None;
            Ok("Retirement cancelled.".into())
        }
        Command::Roster => Ok(state
            .genies
            .iter()
            .map(|g| format!("{}: {}", g.name, g.role))
            .collect::<Vec<_>>()
            .join("; ")),
        Command::Inbox => Ok(format!(
            "You have {} unread updates.",
            state.unread().count()
        )),
        Command::Message(_) => Ok(format!(
            "{} is selected. Work integration is not connected in this checkpoint.",
            state.current()?.name
        )),
        _ => anyhow::bail!("This command needs the work integration."),
    }
}

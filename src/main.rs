use anyhow::{Context, Result, bail, ensure};
use geniewish::{
    app_server::AppServer,
    router::{Command, route},
    runtime::{Runtime, change, emit},
    state::State,
    storage::Store,
};
use serde_json::json;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, BufReader};

struct Options {
    live: bool,
    check: bool,
    playback: bool,
    executable: String,
    workspace: PathBuf,
    state: PathBuf,
}

fn options() -> Result<Option<Options>> {
    let mut options = Options {
        live: false,
        check: false,
        playback: false,
        executable: "codex".into(),
        workspace: std::env::current_dir()?,
        state: ".geniewish".into(),
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                println!(
                    "geniewish\n\nOptions:\n  --live                 Connect to the installed Codex runtime\n  --check-runtime        Check the runtime protocol without creating a conversation\n  --codex PATH           Runtime executable (default: codex)\n  --workspace PATH       Workspace for new conversations\n  --state-dir PATH       Local metadata directory (default: .geniewish)\n  --ack-delivery         Wait for speech playback acknowledgements (native client)\n\nInput: one command per line; output: JSON events. Type 'quit' to exit.\nFirst choose a manager name, e.g. Mike. Then:\n  Mike, hire Annie as my product manager\n  Annie, help me plan the landing page\n  Talk to Mike\n  Mike, any messages?\n  Read Annie's message\n  Mike, retire Annie\n  Yes, retire Annie\n\nThis is the text fallback. For the Mac voice window, run ./scripts/run-mac.sh."
                );
                return Ok(None);
            }
            "--live" => options.live = true,
            "--check-runtime" => options.check = true,
            "--ack-delivery" => options.playback = true,
            "--codex" => options.executable = arguments.next().context("--codex needs a path.")?,
            "--workspace" => {
                options.workspace = arguments
                    .next()
                    .context("--workspace needs a path.")?
                    .into()
            }
            "--state-dir" => {
                options.state = arguments
                    .next()
                    .context("--state-dir needs a path.")?
                    .into()
            }
            _ => bail!("Unknown option: {argument}"),
        }
    }
    Ok(Some(options))
}

#[tokio::main]
async fn main() -> Result<()> {
    let Some(options) = options()? else {
        return Ok(());
    };
    if options.check {
        let _server = AppServer::connect(&options.executable).await?;
        emit(json!({"type":"connected","client":"geniewish"}))?;
        return Ok(());
    }
    let store = Store::open(&options.state)?;
    let mut state = store.load()?;
    let mut runtime = if options.live {
        let mut runtime = Runtime::connect(&options.executable, options.workspace).await?;
        runtime.wait_for_playback = options.playback;
        Some(runtime)
    } else {
        None
    };
    emit(
        json!({"type":"ready","live":options.live,"active":state.active.and_then(|id| state.get(id).ok().map(|g| &g.name)),"names":geniewish::router::NAMES.iter().map(|(name,_)| *name).collect::<Vec<_>>(),"text":if state.manager.is_none() {"Choose your HR manager's name."} else {"Your saved team is ready."}}),
    )?;
    emit_team(&state)?;
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            line = input.next_line() => {
                let Some(mut line) = line? else {break;};
                if line == "quit" {break;}
                if options.playback && line.starts_with('{') {
                    let result = (|| -> Result<bool> {
                        let message: serde_json::Value = serde_json::from_str(&line)?;
                        if message["type"] == "input" {
                            line = message["text"].as_str().context("Client input has no text.")?.to_owned();
                            return Ok(false);
                        }
                        ensure!(message["type"] == "delivered", "Unknown client acknowledgement.");
                        let delivery = serde_json::from_value(message["delivery"].clone())?;
                        runtime.as_mut().context("No runtime is connected.")?.acknowledge_playback(&mut state, &store, delivery)?;
                        Ok(true)
                    })();
                    match result {
                        Ok(true) => continue,
                        Ok(false) => {},
                        Err(error) => {emit(json!({"type":"error","text":format!("{error:#}")}))?; continue;}
                    }
                }
                if let Err(error) = handle(&mut state, &store, &mut runtime, &line).await {
                    emit(json!({"type":"error","text":format!("{error:#}")}))?;
                }
                emit_team(&state)?;
            }
            event = async {
                match runtime.as_mut() {Some(runtime) => runtime.server.next_event().await, None => std::future::pending().await}
            } => {
                // Transport/storage failures stop the client; continuing could lose unread events or create replacement contexts.
                let message = event?;
                runtime.as_mut().context("Runtime disappeared.")?.event(&mut state, &store, message).await?;
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    Ok(())
}

fn reply(state: &State, text: String) -> Result<()> {
    let genie = state.current()?;
    emit(json!({"type":"reply","genie":genie.name,"voice":genie.voice,"text":text}))
}

fn emit_team(state: &State) -> Result<()> {
    emit(
        json!({"type":"team","active":state.active.and_then(|id| state.get(id).ok().map(|g| &g.name)),"genies":state.genies.iter().map(|g| json!({"name":g.name,"role":g.role,"voice":g.voice})).collect::<Vec<_>>()}),
    )
}

async fn handle(
    state: &mut State,
    store: &Store,
    runtime: &mut Option<Runtime>,
    input: &str,
) -> Result<()> {
    let routed = route(input)?;
    if state.manager.is_none() {
        ensure!(
            routed.target.is_some(),
            "Choose a manager name first, for example Mike."
        );
        let chosen = routed
            .target
            .as_deref()
            .context("A manager name is required.")?;
        change(state, store, |s| s.hire(chosen, "HR manager"))?;
        if routed.command == Command::Switch {
            return reply(
                state,
                format!("{chosen} here. I am your HR manager, Human."),
            );
        }
    }
    if let Some(name) = &routed.target {
        change(state, store, |s| s.switch(name))?;
    }
    match routed.command {
        Command::Switch => {
            let current = state.current()?.clone();
            if current.thread_id.is_some()
                && let Some(runtime) = runtime
            {
                runtime.ensure_thread(state, store, current.id).await?;
            }
            let busy = runtime.as_ref().is_some_and(|r| r.busy(current.id));
            reply(
                state,
                if busy {
                    format!("{} is busy but listening.", current.name)
                } else {
                    format!("{} here.", current.name)
                },
            )
        }
        Command::Hire { name, role } => {
            state.require_manager()?;
            let name = name
                .or_else(|| {
                    geniewish::router::NAMES
                        .iter()
                        .find(|(name, _)| state.named(name).is_err())
                        .map(|(name, _)| (*name).to_owned())
                })
                .context("All names are in use. Retire a genie to free a name.")?;
            change(state, store, |s| s.hire(&name, &role))?;
            reply(
                state,
                if role.is_empty() {
                    format!("We hired {name}, Human.")
                } else {
                    format!("We hired {name} as your {role}, Human.")
                },
            )
        }
        Command::SetRole { name, role } => {
            change(state, store, |s| s.set_role(&name, &role))?;
            reply(
                state,
                if role.is_empty() {
                    format!("{name}'s role is now blank, Human.")
                } else {
                    format!("{name}'s role is now {role}, Human.")
                },
            )
        }
        Command::Retire(name) => {
            change(state, store, |s| s.request_retirement(&name))?;
            reply(
                state,
                format!(
                    "Say 'yes, retire {name}' to delete their conversation. Project files remain."
                ),
            )
        }
        Command::ConfirmRetire(name) => {
            let id = state.confirmed_retirement(&name)?;
            if let Some(runtime) = runtime {
                runtime.retire(state, store, &name).await?;
            } else {
                ensure!(
                    state.get(id)?.thread_id.is_none(),
                    "Reconnect with --live to delete this genie's saved conversation."
                );
                change(state, store, |s| s.finish_retirement(id))?;
            }
            reply(
                state,
                format!("{name} is retired. Their name is available."),
            )
        }
        Command::CancelRetirement => {
            change(state, store, |s| {
                s.retiring = None;
                Ok(())
            })?;
            reply(state, "Retirement cancelled.".into())
        }
        Command::Roster => reply(
            state,
            state
                .genies
                .iter()
                .map(|g| {
                    if g.role.is_empty() {
                        g.name.clone()
                    } else {
                        format!("{}: {}", g.name, g.role)
                    }
                })
                .collect::<Vec<_>>()
                .join("; "),
        ),
        Command::Inbox => {
            let mut counts = vec![];
            for genie in &state.genies {
                let count = state.unread().filter(|m| m.owner == genie.id).count();
                if count > 0 {
                    counts.push(format!("{count} from {}", genie.name));
                }
            }
            reply(
                state,
                format!(
                    "You have {} unread updates{}.",
                    state.unread().count(),
                    if counts.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", counts.join(", "))
                    }
                ),
            )
        }
        Command::Read(name) => {
            let runtime = runtime
                .as_mut()
                .context("Use --live to read the original saved message.")?;
            let (update, text) = runtime.read_update(state, &name).await?;
            let speaker = state.current()?.id;
            runtime.reply_update(
                state,
                store,
                &update,
                speaker,
                &format!("{name} says: {text}"),
            )
        }
        Command::Approve(accept) => {
            runtime
                .as_mut()
                .context("No runtime is connected.")?
                .approve(state, accept)
                .await?;
            reply(
                state,
                if accept { "Approved once." } else { "Denied." }.into(),
            )
        }
        Command::CancelWork => {
            runtime
                .as_mut()
                .context("No runtime is connected.")?
                .cancel(state)
                .await?;
            reply(state, "Cancellation requested.".into())
        }
        Command::StopSpeech => emit(json!({"type":"stop_speech"})),
        Command::Message(text) => {
            let runtime = runtime
                .as_mut()
                .context("Work is not connected. Restart with --live.")?;
            emit(
                json!({"type":"user","speaker":geniewish::HUMAN_NAME,"genie":state.current()?.name,"text":text}),
            )?;
            if runtime.submit(state, store, &text).await? {
                reply(state, "I got it.".into())
            } else {
                Ok(())
            }
        }
    }
}

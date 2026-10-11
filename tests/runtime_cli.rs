use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

struct Client {
    child: Child,
    input: ChildStdin,
    events: Receiver<Value>,
}

impl Client {
    fn open(directory: &Path, mode: &str) -> Self {
        Self::open_playback(directory, mode, false)
    }
    fn open_playback(directory: &Path, mode: &str, playback: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_geniewish"));
        if playback {
            command.arg("--ack-delivery");
        }
        let mut child = command
            .args(["--live", "--codex"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app_server.py"))
            .arg("--state-dir")
            .arg(directory.join("local"))
            .arg("--output-dir")
            .arg(directory.join("reports"))
            .env("GENIEWISH_TEST_HISTORY", directory.join("history.json"))
            .env("GENIEWISH_TEST_MODE", mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (sender, events) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else {
                    break;
                };
                let Ok(value) = serde_json::from_str(&line) else {
                    break;
                };
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        let client = Self {
            child,
            input,
            events,
        };
        client.until(|e| e["type"] == "ready");
        client
    }
    fn send(&mut self, text: &str) {
        writeln!(self.input, "{text}").unwrap();
        self.input.flush().unwrap();
    }
    fn until(&self, predicate: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let event = self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("Timed out waiting for a CLI event.");
            if predicate(&event) {
                return event;
            }
        }
    }
    fn reply(&mut self, text: &str) -> Value {
        self.send(text);
        let answer = self.until(|e| e["type"] == "reply");
        if answer.get("delivery").is_some() {
            self.until(|e| e["type"] == "completed" && e["genie"] == answer["genie"]);
        }
        answer
    }
    fn team(&mut self) {
        self.reply("Mike");
        let hired = self.reply("Mike, hire Annie as my product manager");
        if hired["delivery"].is_object() {
            self.send(
                &serde_json::json!({"type":"delivered","delivery":hired["delivery"]}).to_string(),
            );
        }
    }
    fn complete(&mut self) {
        self.send("Annie, please reply");
        self.until(|e| e["type"] == "completed" && e["genie"] == "Annie");
    }
    fn stop(mut self) {
        self.send("quit");
        self.child.wait().unwrap();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn saved(directory: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(directory.join("local/state.json")).unwrap()).unwrap()
}

#[test]
fn notifications_during_rpc_and_restart_keep_the_lifetime_thread() {
    let directory = tempfile::tempdir().unwrap();
    let mut first = Client::open(directory.path(), "");
    first.team();
    first.complete();
    let before = saved(directory.path());
    first.stop();
    let mut second = Client::open(directory.path(), "");
    second.reply("Talk to Annie");
    second.complete();
    let after = saved(directory.path());
    assert_eq!(
        before["genies"][1]["thread_id"],
        after["genies"][1]["thread_id"]
    );
    let history: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("history.json")).unwrap())
            .unwrap();
    assert_eq!(history["next"], 3);
    assert_eq!(
        history["threads"]["thread-2"]["turns"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        !std::fs::read_to_string(directory.path().join("local/state.json"))
            .unwrap()
            .contains("fixture reply")
    );
}

#[test]
fn failed_resume_does_not_create_a_new_conversation() {
    let directory = tempfile::tempdir().unwrap();
    let mut first = Client::open(directory.path(), "");
    first.team();
    first.complete();
    let before = saved(directory.path());
    first.stop();
    let mut second = Client::open(directory.path(), "fail-resume");
    second.send("Talk to Annie");
    assert!(
        second.until(|e| e["type"] == "error")["text"]
            .as_str()
            .unwrap()
            .contains("thread not found")
    );
    assert_eq!(
        before["genies"][1]["thread_id"],
        saved(directory.path())["genies"][1]["thread_id"]
    );
    let history: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("history.json")).unwrap())
            .unwrap();
    assert_eq!(history["next"], 3);
}

#[test]
fn failed_deletion_keeps_the_name_reserved() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::open(directory.path(), "fail-delete");
    client.team();
    client.complete();
    client.reply("Talk to Mike");
    client.reply("Mike, retire Annie");
    assert!(
        client.reply("Yes, retire Annie")["text"]
            .as_str()
            .unwrap()
            .contains("deletion refused")
    );
    assert!(
        client.reply("Mike, hire Annie as my designer")["text"]
            .as_str()
            .unwrap()
            .contains("already hired")
    );
    assert!(saved(directory.path())["genies"][1]["thread_id"].is_string());
}

#[test]
fn approval_cannot_be_resolved_by_the_wrong_genie() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::open(directory.path(), "");
    client.team();
    client.send("Annie, needs approval");
    client.until(|e| e["type"] == "approval");
    client.reply("Talk to Mike");
    client.send("approve once");
    assert!(
        client.until(|e| e["type"] == "error")["text"]
            .as_str()
            .unwrap()
            .contains("current genie")
    );
    client.reply("Annie, approve once");
    let result = client.until(|e| e["type"] == "reply" && e["text"] == "approved");
    assert_eq!(result["genie"], "Annie");
}

#[test]
fn busy_genie_acknowledges_and_steers_the_same_turn() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::open(directory.path(), "");
    client.team();
    client.reply("Mike, hire Allan as my engineer");
    client.send("Allan, needs approval");
    client.until(|e| e["type"] == "approval");
    client.reply("Talk to Mike");
    let busy = client.reply("I want to speak to Allan...");
    assert_eq!(busy["text"], "Allan is busy but listening.");
    let ack = client.reply("For the web page we're working on, use cold colors as the style");
    assert_eq!(ack["text"], "I got it.");
    let combined = client.reply("I want to speak to Allan, use cool blue colors");
    assert_eq!(combined["text"], "I got it.");
    let history: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("history.json")).unwrap())
            .unwrap();
    assert_eq!(history["next"], 3);
    assert_eq!(history["steers"].as_array().unwrap().len(), 2);
    assert_eq!(
        history["steers"][0]["expectedTurnId"],
        history["steers"][1]["expectedTurnId"]
    );
    assert!(
        history["steers"][0]["input"][1]["text"]
            .as_str()
            .unwrap()
            .contains("cold colors")
    );
    client.reply("Allan, approve once");
    let answer = client.until(|e| {
        e["type"] == "reply"
            && e["text"]
                .as_str()
                .is_some_and(|text| text.contains("cool blue colors"))
    });
    assert_eq!(answer["genie"], "Allan");
}

#[test]
fn intermediate_progress_is_not_returned_to_the_human() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::open(directory.path(), "");
    client.team();
    client.send("Annie, please reply");
    let replies = std::cell::RefCell::new(vec![]);
    client.until(|event| {
        if event["type"] == "reply" {
            replies.borrow_mut().push(event["text"].clone());
        }
        event["type"] == "completed"
    });
    assert_eq!(
        *replies.borrow(),
        vec![Value::String("fixture reply".into())]
    );
}

#[test]
fn greeting_hires_an_available_name_without_asking_for_a_role() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::open(directory.path(), "");
    let hired = client.reply("Hi Mike, pls hire a Genie for me");
    assert_eq!(hired["text"], "We hired Lucas, Human.");
    assert_eq!(saved(directory.path())["genies"][1]["role"], "");
    assert_eq!(
        client.reply("Hi pls hire a new Genie for me")["text"],
        "We hired Annie, Human."
    );
    client.reply("Mike, set Lucas's role to my product manager");
    client.send("Lucas, please reply");
    client.until(|e| e["type"] == "completed");
    let thread = saved(directory.path())["genies"][1]["thread_id"].clone();
    client.reply("Mike, clear Lucas's role");
    client.send("Lucas, please reply");
    client.until(|e| e["type"] == "completed");
    assert_eq!(saved(directory.path())["genies"][1]["thread_id"], thread);
    let history: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("history.json")).unwrap())
            .unwrap();
    let worker_inputs = history["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["threadId"] == thread)
        .collect::<Vec<_>>();
    assert!(
        worker_inputs[0]["input"][0]["text"]
            .as_str()
            .unwrap()
            .contains("product manager")
    );
    assert!(
        worker_inputs[1]["input"][0]["text"]
            .as_str()
            .unwrap()
            .contains("none")
    );
}

#[test]
fn a_spoken_result_remains_unread_until_its_exact_playback_is_acknowledged() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::open_playback(directory.path(), "", true);
    client.team();
    client.send("Annie, please reply");
    let answer = client.until(|e| e["type"] == "reply" && e["delivery"].is_object());
    client.until(|e| e["type"] == "completed");
    assert_eq!(saved(directory.path())["inbox"][1]["read"], false);
    // Interrupted playback sends no acknowledgement, so the manager can read it later.
    client.reply("Talk to Mike");
    let read = client.reply("Read Annie's message");
    assert_eq!(read["genie"], "Mike");
    assert_ne!(read["delivery"], answer["delivery"]);
    client.send(&serde_json::json!({"type":"delivered", "delivery":{"thread_id":"wrong-thread", "turn_id":"wrong-turn", "item_id":"wrong-item"}}).to_string());
    client.until(|e| e["type"] == "error");
    assert_eq!(saved(directory.path())["inbox"][1]["read"], false);
    client.send(&serde_json::json!({"type":"delivered", "delivery":read["delivery"]}).to_string());
    assert!(
        client.reply("Mike, any messages?")["text"]
            .as_str()
            .unwrap()
            .contains("0 unread")
    );
    assert_eq!(saved(directory.path())["inbox"][1]["read"], true);
    // Frontend text is framed separately from acknowledgements, including literal JSON/code.
    client.send(
        &serde_json::json!({"type":"input", "text":"Annie, explain {\"enabled\":true}"})
            .to_string(),
    );
    client.until(|e| e["type"] == "completed");
    let history: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("history.json")).unwrap())
            .unwrap();
    assert_eq!(
        history["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .rfind(|i| i["threadId"] == "thread-2")
            .unwrap()["input"][1]["text"],
        "explain {\"enabled\":true}"
    );
}

#[test]
fn invalid_interpretation_does_not_forward_work_or_replace_state() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::open(directory.path(), "");
    client.team();
    let before = saved(directory.path());
    client.send("unrouteable destination");
    client.until(|e| e["type"] == "error");
    assert_eq!(saved(directory.path()), before);
    let history: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("history.json")).unwrap())
            .unwrap();
    assert_eq!(history["inputs"].as_array().unwrap().len(), 1);
    assert!(
        history["threads"].as_object().unwrap().len() == 1,
        "ephemeral interpreter threads are released"
    );
}

#[test]
fn hr_conversation_resumes_with_management_tools_and_keeps_its_identity() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::open(directory.path(), "");
    client.team();
    let thread = saved(directory.path())["genies"][0]["thread_id"].clone();
    client.stop();
    let mut resumed = Client::open(directory.path(), "");
    resumed.reply("Mike, hire Ben");
    let state = saved(directory.path());
    assert_eq!(state["genies"][0]["thread_id"], thread);
    assert_eq!(state["genies"][2]["name"], "Ben");
}

#[test]
fn management_requires_the_real_hr_tool_invocation() {
    for mode in ["wrong-tool-owner", "unassociated-tool"] {
        let directory = tempfile::tempdir().unwrap();
        let mut client = Client::open(directory.path(), mode);
        client.reply("Mike");
        let answer = client.reply("Mike, hire Annie");
        assert!(answer["text"].as_str().unwrap().contains("active HR"));
        assert_eq!(
            saved(directory.path())["genies"].as_array().unwrap().len(),
            1
        );
    }
}

#[test]
fn hr_cannot_delete_memory_without_a_named_human_confirmation() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::open(directory.path(), "");
    client.team();
    assert!(
        client.reply("try retiring without confirmation")["text"]
            .as_str()
            .unwrap()
            .contains("explicitly")
    );
    client.reply("Mike, retire Annie");
    assert!(
        client.reply("yes")["text"]
            .as_str()
            .unwrap()
            .contains("explicitly")
    );
    assert!(
        client.reply("Yes, do not retire Annie")["text"]
            .as_str()
            .unwrap()
            .contains("explicitly")
    );
    assert_eq!(
        saved(directory.path())["genies"].as_array().unwrap().len(),
        2
    );
    client.reply("Yes, retire Annie");
    assert_eq!(
        saved(directory.path())["genies"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn compound_hiring_and_delegation_use_the_new_genies_lifetime_conversation() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::open(directory.path(), "");
    client.reply("Mike");
    client.reply("hire Annie then ask her to reply");
    assert!(
        saved(directory.path())["inbox"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["owner"] == 2 && m["read"] == false)
    );
    let state = saved(directory.path());
    assert_eq!(state["active"], state["manager"]);
    assert!(state["genies"][1]["thread_id"].is_string());
    assert_eq!(
        client.reply("Read Annie's message")["text"],
        "fixture reply"
    );
}

#[test]
fn background_hr_hiring_preserves_the_foreground_and_duplicate_calls_execute_once() {
    for mode in ["", "replay-tool"] {
        let directory = tempfile::tempdir().unwrap();
        let mut client = Client::open(directory.path(), mode);
        client.team();
        client.send("delayed hire");
        client.until(|e| e["type"] == "working" && e["genie"] == "Mike");
        client.reply("Talk to Annie");
        client.until(|e| e["type"] == "completed" && e["genie"] == "Mike");
        let state = saved(directory.path());
        assert_eq!(state["active"], state["genies"][1]["id"]);
        assert_eq!(state["genies"].as_array().unwrap().len(), 3);
    }
}

#[test]
fn interrupted_hr_readouts_preserve_original_message_receipts_across_restart() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::open_playback(directory.path(), "", true);
    client.team();
    client.send("Annie, please reply");
    let original = client.until(|e| e["type"] == "reply" && e["genie"] == "Annie");
    client.until(|e| e["type"] == "completed" && e["genie"] == "Annie");
    client.reply("Talk to Mike");
    let interrupted = client.reply("Read Annie's message");
    client.stop();
    let mut resumed = Client::open_playback(directory.path(), "", true);
    resumed.send(
        &serde_json::json!({"type":"delivered","delivery":interrupted["delivery"]}).to_string(),
    );
    resumed.until(|e| e["type"] == "error");
    let replayed = resumed.reply("Read Mike's message");
    assert_eq!(replayed["text"], "fixture reply");
    resumed
        .send(&serde_json::json!({"type":"delivered","delivery":replayed["delivery"]}).to_string());
    resumed.reply("Mike, any messages?");
    let state = saved(directory.path());
    assert!(
        state["inbox"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["item_id"] == original["delivery"]["item_id"]
                && m["thread_id"] == original["delivery"]["thread_id"])
            .unwrap()["read"]
            .as_bool()
            .unwrap()
    );
}

#[test]
fn a_short_answer_to_hr_keeps_its_owner_even_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::open(directory.path(), "");
    client.reply("Mike");
    client.reply("ask me which genie to hire");
    let thread = saved(directory.path())["genies"][0]["thread_id"].clone();
    client.stop();
    let mut resumed = Client::open(directory.path(), "");
    resumed.reply("Ruby");
    let state = saved(directory.path());
    assert_eq!(state["active"], state["manager"]);
    assert_eq!(state["genies"][0]["thread_id"], thread);
    assert_eq!(state["genies"][1]["name"], "Ruby");
}

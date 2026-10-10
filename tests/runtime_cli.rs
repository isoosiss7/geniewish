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
        let mut child = Command::new(env!("CARGO_BIN_EXE_geniewish"))
            .args(["--live", "--codex"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app_server.py"))
            .arg("--state-dir")
            .arg(directory.join("local"))
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
        self.until(|e| e["type"] == "reply")
    }
    fn team(&mut self) {
        self.reply("Mike");
        self.reply("Mike, hire Annie as my product manager");
    }
    fn complete(&mut self) {
        self.send("Annie, please reply");
        self.until(|e| e["type"] == "completed");
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
    assert_eq!(history["next"], 2);
    assert_eq!(
        history["threads"]["thread-1"]["turns"]
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
    assert_eq!(history["next"], 2);
}

#[test]
fn failed_deletion_keeps_the_name_reserved() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::open(directory.path(), "fail-delete");
    client.team();
    client.complete();
    client.reply("Talk to Mike");
    client.reply("Mike, retire Annie");
    client.send("Yes, retire Annie");
    assert!(
        client.until(|e| e["type"] == "error")["text"]
            .as_str()
            .unwrap()
            .contains("deletion refused")
    );
    client.send("Mike, hire Annie as my designer");
    assert!(
        client.until(|e| e["type"] == "error")["text"]
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
    assert_eq!(history["next"], 2);
    assert_eq!(history["steers"].as_array().unwrap().len(), 2);
    assert_eq!(
        history["steers"][0]["expectedTurnId"],
        history["steers"][1]["expectedTurnId"]
    );
    assert!(
        history["steers"][0]["input"][0]["text"]
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

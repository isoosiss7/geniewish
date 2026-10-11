use geniewish::management::{named_confirmation, save_report};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::fs::{PermissionsExt, symlink},
    process::{Command, Stdio},
};

#[test]
fn reports_use_unique_private_files_and_reject_paths() {
    let directory = tempfile::tempdir().unwrap();
    let first = save_report(directory.path(), "plan.md", "Original").unwrap();
    let second = save_report(directory.path(), "plan.md", "New report").unwrap();
    assert_ne!(first, second);
    assert_eq!(std::fs::read_to_string(first).unwrap(), "Original");
    assert_eq!(std::fs::read_to_string(&second).unwrap(), "New report");
    assert_eq!(
        std::fs::metadata(second).unwrap().permissions().mode() & 0o777,
        0o600
    );
    for bad in [
        "../secret",
        "/tmp/secret",
        ".hidden",
        "folder/report.md",
        "",
    ] {
        assert!(save_report(directory.path(), bad, "content").is_err());
    }
    symlink(
        directory.path().join("plan.md"),
        directory.path().join("linked.md"),
    )
    .unwrap();
    let third = save_report(directory.path(), "linked.md", "Another report").unwrap();
    assert_ne!(third, directory.path().join("linked.md"));
    assert_eq!(
        std::fs::read_to_string(directory.path().join("plan.md")).unwrap(),
        "Original"
    );
}

#[test]
fn explicit_retirement_consent_cannot_be_inferred_from_yes_or_negation() {
    assert!(named_confirmation("Yes, retire Annie", "Annie"));
    assert!(named_confirmation("Yes please retire Annie!", "Annie"));
    for input in [
        "yes",
        "Yes do not retire Annie",
        "Yes retire Allan",
        "Maybe retire Annie",
        "Annie",
    ] {
        assert!(!named_confirmation(input, "Annie"));
    }
}

#[test]
fn stdio_mcp_adapter_lists_tools_and_reports_a_disconnected_parent() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_geniewish"))
        .args(["--hr-tools", "/tmp/geniewish-missing-test-parent", "1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut call = |id, method: &str, params: Value| {
        writeln!(
            input,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
        .unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    assert_eq!(
        call(1, "initialize", json!({}))["result"]["serverInfo"]["name"],
        "geniewish"
    );
    assert_eq!(
        call(4, "resources/list", json!({}))["error"]["code"],
        -32601
    );
    assert_eq!(call(5, "ping", json!({}))["result"], json!({}));
    let catalog = call(2, "tools/list", json!({}));
    assert_eq!(catalog["result"]["tools"].as_array().unwrap().len(), 2);
    assert!(call(3,"tools/call",json!({"name":"manage_genies","arguments":{"action":"hire","name":null,"role":"","text":""}}))["result"]["isError"].as_bool().unwrap());
    drop(input);
    assert!(child.wait().unwrap().success());
}

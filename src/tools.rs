//! A local stdio MCP adapter. Only the parent client owns state and executes actions.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    pub owner: u64,
    pub id: String,
    pub tool: String,
    pub arguments: Value,
}

pub struct Request {
    pub call: Call,
    pub reply: oneshot::Sender<Value>,
}

pub struct Bridge {
    pub path: PathBuf,
    pub requests: mpsc::Receiver<Request>,
    task: JoinHandle<()>,
    directory: PathBuf,
}

impl Bridge {
    pub fn open() -> Result<Self> {
        // A short private path also stays within macOS's Unix socket path limit.
        let directory = PathBuf::from("/tmp").join(format!(
            "gw-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        std::fs::create_dir(&directory)?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        let path = directory.join("hr.sock");
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let (sender, requests) = mpsc::channel(8);
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let sender = sender.clone();
                tokio::spawn(async move {
                    let result = async {
                        let (read, mut write) = socket.into_split();
                        let mut socket = BufReader::new(read.take(256 * 1024 + 1));
                        let mut line = String::new();
                        tokio::time::timeout(Duration::from_secs(5), socket.read_line(&mut line))
                            .await??;
                        ensure!(line.len() <= 256 * 1024, "Tool request is too large.");
                        let call: Call = serde_json::from_str(&line)?;
                        let (reply, result) = oneshot::channel();
                        sender
                            .send(Request { call, reply })
                            .await
                            .context("Client disconnected")?;
                        let result =
                            tokio::time::timeout(Duration::from_secs(120), result).await??;
                        write
                            .write_all(serde_json::to_string(&result)?.as_bytes())
                            .await?;
                        write.write_all(b"\n").await?;
                        Ok::<_, anyhow::Error>(())
                    }
                    .await;
                    if result.is_err() {
                        crate::diagnostics::record("hr.connection_error", json!({}));
                    }
                });
            }
        });
        Ok(Self {
            path,
            requests,
            task,
            directory,
        })
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

pub fn catalog() -> Value {
    json!({"tools":[
        {"name":"manage_genies","description":"Manage Human's team. Rust executes actions and returns actual results. Use roster/inbox for authoritative state. Hire chooses a name if name is null; role is optional and stays blank. retire requests confirmation; confirm_retire only works after Human explicitly says yes, retire NAME. switch changes the foreground. message sends or steers work to an existing work genie; preserve Human's requirements. read returns the original oldest unread update; relay its substance in the final reply. Never claim success before the tool succeeds.","inputSchema":{"type":"object","additionalProperties":false,"required":["action","name","role","text"],"properties":{
            "action":{"type":"string","enum":["hire","set_role","retire","confirm_retire","cancel_retirement","roster","inbox","read","switch","message"]},
            "name":{"type":["string","null"]},"role":{"type":"string"},"text":{"type":"string"}}}},
        {"name":"save_report","description":"Save a detailed HR report in the designated Desktop output folder. Returns the actual unique filename after writing. Keep the spoken reply brief and mention that filename.","inputSchema":{"type":"object","additionalProperties":false,"required":["filename","content"],"properties":{"filename":{"type":"string"},"content":{"type":"string"}}}}
    ]})
}

/// Internal subprocess mode. Never acquires the state lock or opens account files.
pub async fn serve(path: &Path, owner: u64) -> Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut output = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        let message: Value = serde_json::from_str(&line)?;
        let Some(id) = message.get("id") else {
            continue;
        };
        let result = match message["method"].as_str().unwrap_or("") {
            "initialize" => Ok(
                json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"geniewish","version":env!("CARGO_PKG_VERSION")}}),
            ),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(catalog()),
            "tools/call" => {
                let request = Call {
                    owner,
                    id: format!("{}-{id}", std::process::id()),
                    tool: message["params"]["name"].as_str().unwrap_or("").into(),
                    arguments: message["params"]["arguments"].clone(),
                };
                async {
                    let mut socket = UnixStream::connect(path).await?;
                    socket.write_all(serde_json::to_string(&request)?.as_bytes()).await?;
                    socket.write_all(b"\n").await?;
                    let mut response = String::new();
                    tokio::time::timeout(Duration::from_secs(120), BufReader::new(socket).read_line(&mut response)).await??;
                    ensure!(!response.is_empty(), "The geniewish client disconnected; action outcome is unknown. Do not retry automatically.");
                    Ok(serde_json::from_str::<Value>(&response)?)
                }.await
            }
            _ => Err(anyhow::anyhow!("Unsupported MCP request")),
        };
        let response = match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(error) if message["method"] == "tools/call" => {
                json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":error.to_string()}],"isError":true}})
            }
            Err(error) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":error.to_string()}})
            }
        };
        output
            .write_all(serde_json::to_string(&response)?.as_bytes())
            .await?;
        output.write_all(b"\n").await?;
        output.flush().await?;
    }
    Ok(())
}

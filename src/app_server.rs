use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{collections::VecDeque, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
};

/// One process connection. Notifications encountered during an RPC are retained.
pub struct AppServer {
    _child: Child,
    input: BufWriter<ChildStdin>,
    output: Lines<BufReader<ChildStdout>>,
    deferred: VecDeque<Value>,
    next_id: u64,
}

impl AppServer {
    pub async fn connect(executable: &str) -> Result<Self> {
        crate::diagnostics::record("runtime.connect", json!({"executable":executable}));
        let mut child = Command::new(executable)
            .args([
                "app-server",
                "--listen",
                "stdio://",
                "-c",
                "features.memories=false",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .context("Could not start the installed work runtime.")?;
        let input = BufWriter::new(child.stdin.take().context("Missing runtime stdin.")?);
        let output =
            BufReader::new(child.stdout.take().context("Missing runtime stdout.")?).lines();
        let mut server = Self {
            _child: child,
            input,
            output,
            deferred: VecDeque::new(),
            next_id: 1,
        };
        server.call("initialize", json!({"clientInfo":{"name":"geniewish","title":"geniewish","version":env!("CARGO_PKG_VERSION")}})).await?;
        server
            .send(json!({"method":"initialized","params":{}}))
            .await?;
        Ok(server)
    }

    pub async fn send(&mut self, message: Value) -> Result<()> {
        self.input
            .write_all(serde_json::to_string(&message)?.as_bytes())
            .await?;
        self.input.write_all(b"\n").await?;
        self.input.flush().await?;
        Ok(())
    }

    async fn read(&mut self) -> Result<Value> {
        let line = self
            .output
            .next_line()
            .await?
            .context("The work runtime disconnected. Saved conversations were not reset.")?;
        let value: Value =
            serde_json::from_str(&line).context("The runtime returned invalid JSON.")?;
        if value.get("method").is_some() {
            crate::diagnostics::record(
                "runtime.event",
                json!({"method":value["method"],"request":value.get("id").is_some(),"thread":value["params"]["threadId"],"turn":value["params"]["turnId"]}),
            );
        }
        Ok(value)
    }

    pub async fn next_event(&mut self) -> Result<Value> {
        if let Some(event) = self.deferred.pop_front() {
            return Ok(event);
        }
        self.read().await
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let start = std::time::Instant::now();
        crate::diagnostics::record("runtime.rpc_start", json!({"method":method,"id":id}));
        self.send(json!({"id":id,"method":method,"params":params}))
            .await?;
        let result = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let message = self.read().await?;
                if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                    if let Some(error) = message.get("error") {
                        crate::diagnostics::record(
                            "runtime.rpc_error",
                            json!({"method":method,"id":id,"code":error["code"]}),
                        );
                        bail!("{method} failed: {error}");
                    }
                    return message
                        .get("result")
                        .cloned()
                        .context("Runtime response has no result.");
                }
                self.deferred.push_back(message);
            }
        })
        .await
        .context(
            "Runtime request timed out. It may have taken effect; do not automatically replay it.",
        )?;
        crate::diagnostics::record(
            "runtime.rpc_end",
            json!({"method":method,"id":id,"elapsed_ms":start.elapsed().as_millis(),"success":result.is_ok()}),
        );
        result
    }

    pub async fn respond(&mut self, id: Value, result: Value) -> Result<()> {
        self.send(json!({"id":id,"result":result})).await
    }
    pub async fn unsupported(&mut self, id: Value) -> Result<()> {
        self.send(json!({"id":id,"error":{"code":-32601,"message":"This request is not yet supported by geniewish."}})).await
    }
}

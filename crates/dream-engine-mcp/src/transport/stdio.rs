use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use dream_engine_process::ChildContainment;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use super::{McpError, McpTransport};
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Stdio transport: communicates with MCP server via child process stdin/stdout
pub struct StdioTransport {
    stdin: Mutex<BufWriter<ChildStdin>>,
    stdout: Mutex<BufReader<ChildStdout>>,
    child: Mutex<Child>,
    child_id: Option<u32>,
    containment: ChildContainment,
    next_id: AtomicU64,
}

impl StdioTransport {
    /// Spawn a child process and return the transport
    pub async fn spawn(command: &str, args: &[String], env: &HashMap<String, String>) -> Result<Self, McpError> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.kill_on_drop(true)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .envs(env);
        dream_engine_config::shell::suppress_console_window(&mut cmd);
        ChildContainment::configure(&mut cmd);

        let mut child = cmd
            .spawn()
            .map_err(|e| McpError::Transport(format!("Failed to spawn '{}': {}", command, e)))?;
        let child_id = child.id();
        let containment = ChildContainment::attach_owned(&mut child)
            .map_err(|e| McpError::Transport(format!("Failed to contain MCP process: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Transport("Failed to capture child stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Transport("Failed to capture child stdout".into()))?;

        Ok(Self {
            stdin: Mutex::new(BufWriter::new(stdin)),
            stdout: Mutex::new(BufReader::new(stdout)),
            child: Mutex::new(child),
            child_id,
            containment,
            next_id: AtomicU64::new(1),
        })
    }

    /// Get the next request ID
    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Send a JSON-RPC message (request or notification) via stdin
    async fn send(&self, req: &JsonRpcRequest) -> Result<(), McpError> {
        let json =
            serde_json::to_string(req).map_err(|e| McpError::Transport(format!("JSON serialize error: {}", e)))?;

        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(json.as_bytes())
            .await
            .map_err(|e| McpError::Transport(format!("Write to stdin failed: {}", e)))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| McpError::Transport(format!("Write newline failed: {}", e)))?;
        stdin
            .flush()
            .await
            .map_err(|e| McpError::Transport(format!("Flush stdin failed: {}", e)))?;
        Ok(())
    }

    /// Read a single JSON-RPC response from stdout
    async fn read_response(&self) -> Result<JsonRpcResponse, McpError> {
        let mut stdout = self.stdout.lock().await;
        let mut line = String::new();

        // Read lines until we get a non-empty one (skip blank lines)
        loop {
            line.clear();
            let bytes_read = stdout
                .read_line(&mut line)
                .await
                .map_err(|e| McpError::Transport(format!("Read from stdout failed: {}", e)))?;

            if bytes_read == 0 {
                return Err(McpError::Transport("Child process stdout closed".into()));
            }

            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let response: JsonRpcResponse = serde_json::from_str(trimmed).map_err(|e| {
                    McpError::Transport(format!("Failed to parse JSON-RPC response: {} — raw: {}", e, trimmed))
                })?;
                return Ok(response);
            }
        }
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn request(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        self.send(req).await?;
        let response = self.read_response().await?;

        // Check for JSON-RPC error in response
        if let Some(err) = &response.error {
            return Err(McpError::JsonRpc {
                code: err.code,
                message: err.message.clone(),
            });
        }

        Ok(response)
    }

    async fn notify(&self, req: &JsonRpcRequest) -> Result<(), McpError> {
        self.send(req).await
    }

    async fn close(&self) -> Result<(), McpError> {
        let mut child = self.child.lock().await;
        self.containment
            .terminate(&mut child, self.child_id)
            .map_err(|e| McpError::Transport(format!("Failed to stop MCP process tree: {e}")))?;
        child
            .wait()
            .await
            .map_err(|e| McpError::Transport(format!("Failed to reap MCP process: {e}")))?;
        Ok(())
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        if let Err(error) = self.containment.terminate(self.child.get_mut(), self.child_id) {
            tracing::warn!(target: "dream_engine_mcp", %error, "failed to reap MCP process tree on drop");
        }
    }
}

#[cfg(test)]
#[path = "stdio_test.rs"]
mod stdio_test;

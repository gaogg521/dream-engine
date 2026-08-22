use std::io;
use std::sync::Arc;

use dream_engine_agent::output::OutputSink;
use dream_engine_protocol::events::{ProtocolEvent, ToolStatus};
use dream_engine_protocol::writer::ProtocolEmitter;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug)]
pub(super) enum AgentEvent {
    StreamStart,
    TextDelta(String),
    Thinking(String),
    Info(String),
    Error(String),
    ToolCall {
        call_id: String,
        name: String,
        input: String,
    },
    ToolResult {
        call_id: String,
        name: String,
        is_error: bool,
        content: String,
    },
    ApprovalRequested {
        call_id: String,
        name: String,
        description: String,
        input: String,
    },
    ToolRunning {
        call_id: String,
        name: String,
    },
    ProtocolToolResult {
        call_id: String,
        name: String,
        is_error: bool,
        content: String,
    },
    ToolCancelled {
        call_id: String,
        name: String,
        reason: String,
    },
}

pub(super) struct TuiSink {
    tx: UnboundedSender<AgentEvent>,
}

impl TuiSink {
    pub(super) fn shared(tx: UnboundedSender<AgentEvent>) -> Arc<dyn OutputSink> {
        Arc::new(Self { tx })
    }

    fn send(&self, event: AgentEvent) {
        let _ = self.tx.send(event);
    }
}

impl OutputSink for TuiSink {
    fn emit_text_delta(&self, text: &str, _msg_id: &str) {
        self.send(AgentEvent::TextDelta(text.to_string()));
    }

    fn emit_thinking(&self, text: &str, _msg_id: &str) {
        self.send(AgentEvent::Thinking(text.to_string()));
    }

    fn emit_tool_call(&self, tool_use_id: &str, name: &str, input: &str) {
        self.send(AgentEvent::ToolCall {
            call_id: tool_use_id.to_string(),
            name: name.to_string(),
            input: input.to_string(),
        });
    }

    fn emit_tool_result(&self, tool_use_id: &str, name: &str, is_error: bool, content: &str) {
        self.send(AgentEvent::ToolResult {
            call_id: tool_use_id.to_string(),
            name: name.to_string(),
            is_error,
            content: content.to_string(),
        });
    }

    fn emit_stream_start(&self, _msg_id: &str) {
        self.send(AgentEvent::StreamStart);
    }

    fn emit_stream_end(
        &self,
        _msg_id: &str,
        _turns: usize,
        _input_tokens: u64,
        _output_tokens: u64,
        _cache_creation_tokens: u64,
        _cache_read_tokens: u64,
    ) {
    }

    fn emit_error(&self, msg: &str) {
        self.send(AgentEvent::Error(msg.to_string()));
    }

    fn emit_info(&self, msg: &str) {
        self.send(AgentEvent::Info(msg.to_string()));
    }
}

pub(super) struct TuiProtocolEmitter {
    tx: UnboundedSender<AgentEvent>,
}

impl TuiProtocolEmitter {
    pub(super) fn shared(tx: UnboundedSender<AgentEvent>) -> Arc<dyn ProtocolEmitter> {
        Arc::new(Self { tx })
    }

    fn send(&self, event: AgentEvent) {
        let _ = self.tx.send(event);
    }
}

impl ProtocolEmitter for TuiProtocolEmitter {
    fn emit(&self, event: &ProtocolEvent) -> io::Result<()> {
        match event {
            ProtocolEvent::ToolRequest { call_id, tool, .. } => {
                let input = serde_json::to_string_pretty(&tool.args).unwrap_or_else(|_| tool.args.to_string());
                self.send(AgentEvent::ApprovalRequested {
                    call_id: call_id.clone(),
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    input,
                });
            }
            ProtocolEvent::ToolRunning { call_id, tool_name, .. } => self.send(AgentEvent::ToolRunning {
                call_id: call_id.clone(),
                name: tool_name.clone(),
            }),
            ProtocolEvent::ToolResult {
                call_id,
                tool_name,
                status,
                output,
                ..
            } => self.send(AgentEvent::ProtocolToolResult {
                call_id: call_id.clone(),
                name: tool_name.clone(),
                is_error: matches!(status, ToolStatus::Error),
                content: output.clone(),
            }),
            ProtocolEvent::ToolCancelled { call_id, reason, .. } => self.send(AgentEvent::ToolCancelled {
                call_id: call_id.clone(),
                name: "tool".to_string(),
                reason: reason.clone(),
            }),
            _ => {}
        }
        Ok(())
    }
}

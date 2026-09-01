//! Native Ollama provider: talks to a local daemon's `/api/chat` endpoint
//! instead of the OpenAI-compatible `/v1/chat/completions` shim.
//!
//! Why native: the `/v1` shim ignores `num_ctx`, so the daemon keeps its own
//! default context window (4096 unless OLLAMA_CONTEXT_LENGTH is set) and 400s
//! with "context overflow" as soon as a conversation outgrows it — no client
//! configuration can fix that through the shim. The native endpoint accepts
//! `options.num_ctx`, which is the only way to make the user's configured
//! context window actually take effect server-side.

use async_trait::async_trait;
use serde_json::{Map, Value};
use tokio::sync::mpsc;

use dream_engine_config::compat::ProviderCompat;
use dream_engine_types::llm::{LlmEvent, LlmRequest};
use dream_engine_types::message::{StopReason, TokenUsage};

use crate::composed::ComposedProvider;
use crate::error::{ProviderError, provider_error_from_json_body};
use crate::openai_messages::generate_call_id;
use crate::transport::{OllamaTransport, ProviderTransport};
use crate::{LlmProvider};

pub struct OllamaProvider {
    inner: ComposedProvider,
}

impl OllamaProvider {
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat) -> Self {
        let transport = ProviderTransport::Ollama(OllamaTransport::new(api_key, base_url));
        let inner = ComposedProvider::new(transport, compat);

        Self { inner }
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.inner.stream(request).await
    }
}

/// Rewrite an OpenAI Chat Completions body (as projected by
/// [`crate::projector::OpenAiProjector`]) into the Ollama `/api/chat` shape.
///
/// Field mapping:
/// - `max_tokens` / `max_completion_tokens` → `options.num_predict`
/// - configured context window → `options.num_ctx`
/// - `thinking: {"type": "enabled"|"disabled"}` → `think: true|false`
/// - `stream_options` / `reasoning_effort` are dropped (unknown to `/api/chat`)
/// - assistant `tool_calls[].function.arguments` are parsed from the OpenAI
///   string form into the object form Ollama's decoder requires
pub(crate) fn to_ollama_chat_body(openai_body: &Value, num_ctx: Option<u32>) -> Value {
    let mut body = Map::new();
    if let Some(model) = openai_body.get("model") {
        body.insert("model".into(), model.clone());
    }
    let messages = openai_body
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| messages.iter().map(message_to_ollama).collect::<Vec<_>>())
        .unwrap_or_default();
    body.insert("messages".into(), Value::Array(messages));
    if let Some(tools) = openai_body.get("tools") {
        body.insert("tools".into(), tools.clone());
    }
    body.insert("stream".into(), Value::Bool(true));

    let mut options = Map::new();
    let num_predict = openai_body
        .get("max_tokens")
        .or_else(|| openai_body.get("max_completion_tokens"))
        .and_then(Value::as_u64);
    if let Some(num_predict) = num_predict {
        options.insert("num_predict".into(), json_u64(num_predict));
    }
    if let Some(num_ctx) = num_ctx {
        options.insert("num_ctx".into(), json_u64(u64::from(num_ctx)));
    }
    if !options.is_empty() {
        body.insert("options".into(), Value::Object(options));
    }

    match openai_body.pointer("/thinking/type").and_then(Value::as_str) {
        Some("enabled") => {
            body.insert("think".into(), Value::Bool(true));
        }
        Some("disabled") => {
            body.insert("think".into(), Value::Bool(false));
        }
        _ => {}
    }

    Value::Object(body)
}

fn json_u64(value: u64) -> Value {
    Value::Number(serde_json::Number::from(value))
}

/// Convert one OpenAI-shaped history message for `/api/chat`. The only
/// structural difference that matters is tool-call arguments: OpenAI carries
/// them as a serialized JSON string, Ollama as an object.
fn message_to_ollama(message: &Value) -> Value {
    let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) else {
        return message.clone();
    };
    let mut message = message.clone();
    let converted: Vec<Value> = tool_calls
        .iter()
        .map(|tool_call| {
            let Some(arguments) = tool_call.pointer("/function/arguments") else {
                return tool_call.clone();
            };
            let parsed = arguments
                .as_str()
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .unwrap_or_else(|| arguments.clone());
            let mut tool_call = tool_call.clone();
            tool_call["function"]["arguments"] = parsed;
            tool_call
        })
        .collect();
    message["tool_calls"] = Value::Array(converted);
    message
}

/// Streaming state for the NDJSON `/api/chat` response.
pub(crate) struct OllamaStreamState {
    /// Whether any tool call was emitted this turn — decides between
    /// `StopReason::ToolUse` and `StopReason::EndTurn` on the final frame.
    emitted_tool_use: bool,
    /// Error payload carried by an in-stream `{"error": ...}` line.
    stream_error: Option<ProviderError>,
}

impl OllamaStreamState {
    pub(crate) fn new() -> Self {
        Self {
            emitted_tool_use: false,
            stream_error: None,
        }
    }

    pub(crate) fn take_stream_error(&mut self) -> Option<ProviderError> {
        self.stream_error.take()
    }
}

/// Parse one NDJSON line from the `/api/chat` stream into events.
///
/// Frame shapes (stream=true):
/// - content: `{"message":{"role":"assistant","content":"..."},"done":false}`
/// - thinking: `{"message":{"thinking":"..."},"done":false}`
/// - tool call: `{"message":{"tool_calls":[{"function":{"name","arguments":{...}}}]},"done":false}`
///   (Ollama emits tool calls complete in one frame, not as deltas)
/// - terminal: `{"done":true,"done_reason":"stop","prompt_eval_count":N,"eval_count":M}`
pub(crate) fn parse_ollama_ndjson_line(line: &str, state: &mut OllamaStreamState) -> Vec<LlmEvent> {
    let mut events = Vec::new();

    let json: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => return events,
    };

    if json.get("error").is_some_and(|error| !error.is_null()) {
        state.stream_error = Some(
            provider_error_from_json_body(&json, line.as_bytes()).unwrap_or_else(|| {
                ProviderError::Parse("Ollama returned an error payload inside a successful stream".to_string())
            }),
        );
        return events;
    }

    let message = &json["message"];
    if let Some(thinking) = message["thinking"].as_str().filter(|text| !text.is_empty()) {
        events.push(LlmEvent::ThinkingDelta(thinking.to_string()));
    }
    if let Some(content) = message["content"].as_str().filter(|text| !text.is_empty()) {
        events.push(LlmEvent::TextDelta(content.to_string()));
    }
    if let Some(tool_calls) = message["tool_calls"].as_array() {
        for tool_call in tool_calls {
            let name = tool_call["function"]["name"].as_str().unwrap_or_default().to_string();
            let input = tool_call["function"]["arguments"].clone();
            let input = if input.is_null() { Value::Object(Map::new()) } else { input };
            state.emitted_tool_use = true;
            events.push(LlmEvent::ToolUse {
                id: generate_call_id(),
                name,
                input,
                extra: None,
            });
        }
    }

    if json["done"].as_bool() == Some(true) {
        let stop_reason = match json["done_reason"].as_str() {
            Some("length") => StopReason::MaxTokens,
            _ if state.emitted_tool_use => StopReason::ToolUse,
            _ => StopReason::EndTurn,
        };
        events.push(LlmEvent::Done {
            stop_reason,
            usage: TokenUsage {
                input_tokens: json["prompt_eval_count"].as_u64().unwrap_or(0),
                output_tokens: json["eval_count"].as_u64().unwrap_or(0),
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        });
    }

    events
}

#[cfg(test)]
#[path = "ollama_test.rs"]
mod ollama_test;

//! End-to-end reproduction of the output-truncation bug against a stubbed
//! OpenAI-compatible endpoint.
//!
//! Unlike the unit tests, this drives the *real* provider stack — HTTP
//! transport, SSE frame parsing, `finish_reason` mapping and the projector —
//! so it covers the layers a mocked `LlmProvider` skips.
//!
//! The stub emulates a gateway with a small output cap (DeepSeek defaults to
//! 4096 output tokens): every response is cut short with
//! `finish_reason: "length"` until the content is exhausted.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use dream_engine_agent::engine::AgentEngine;
use dream_engine_agent::output::OutputSink;
use dream_engine_agent::output::terminal::TerminalSink;
use dream_engine_config::compat::ProviderCompat;
use dream_engine_config::config::{Config, ProviderType, SessionConfig, SkillsPermissionConfig, ToolsConfig};
use dream_engine_config::hooks::HooksConfig;
use dream_engine_mcp::config::McpConfig;
use dream_engine_providers::create_provider;
use dream_engine_tools::registry::ToolRegistry;
use dream_engine_tools::write::WriteTool;
use dream_engine_types::message::StopReason;
use serde_json::json;
use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Lines of Python emitted per model turn before the stub hits its cap.
const LINES_PER_CHUNK: usize = 250;
/// Total lines the "model" wants to produce — the user's 3000-line request.
const TOTAL_LINES: usize = 3000;

fn sse_frame(value: serde_json::Value) -> String {
    format!("data: {value}\n\n")
}

/// Serves one chunk of the answer per request, cutting every chunk short with
/// `finish_reason: "length"` except the final one.
struct TruncatingEndpoint {
    calls: AtomicUsize,
}

impl Respond for TruncatingEndpoint {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let start = call * LINES_PER_CHUNK;
        let end = ((call + 1) * LINES_PER_CHUNK).min(TOTAL_LINES);
        let is_last = end >= TOTAL_LINES;

        let mut body = String::new();
        for line in start..end {
            // Deliberately includes braces, quotes and backslashes: if SSE
            // framing were sensitive to payload characters, this would break.
            let content = format!("def f_{line}():\n    return {{\"k\": \"v\\\\{line}\"}}\n");
            body.push_str(&sse_frame(json!({
                "choices": [{ "index": 0, "delta": { "content": content } }]
            })));
        }

        let finish_reason = if is_last { "stop" } else { "length" };
        body.push_str(&sse_frame(json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }]
        })));
        body.push_str(&sse_frame(json!({
            "choices": [],
            "usage": { "prompt_tokens": 100, "completion_tokens": 4096, "total_tokens": 4196 }
        })));
        body.push_str("data: [DONE]\n\n");

        ResponseTemplate::new(200)
            .set_body_raw(body, "text/event-stream")
            .append_header("content-type", "text/event-stream")
    }
}

fn stub_config(base_url: String) -> Config {
    Config {
        provider: ProviderType::OpenAI,
        provider_label: "deepseek-stub".to_string(),
        api_key: "test-key".to_string(),
        base_url,
        model: "deepseek-v4-flash".to_string(),
        // Mirrors the real defect: no per-model cap configured, so dream omits
        // max_tokens entirely and the gateway applies its own small default.
        max_tokens: None,
        max_turns: Some(10),
        max_tool_call_malformed_turns: Some(3),
        max_tool_call_failure_turns: Some(3),
        system_prompt: Some("You are a helpful assistant.".to_string()),
        thinking: None,
        prompt_caching: false,
        compat: ProviderCompat::openai_defaults(),
        tools: ToolsConfig {
            auto_approve: true,
            allow_list: vec![],
            skills: SkillsPermissionConfig::default(),
        },
        session: SessionConfig {
            enabled: false,
            directory: std::env::temp_dir().to_string_lossy().into_owned(),
            max_sessions: 1,
        },
        compact: dream_engine_config::compact::CompactConfig::default(),
        plan: dream_engine_config::plan::PlanConfig::default(),
        shell: dream_engine_config::shell::ShellConfig::default(),
        file_cache: dream_engine_config::file_cache::FileCacheConfig::default(),
        hooks: HooksConfig::default(),
        bedrock: None,
        vertex: None,
        mcp: McpConfig::default(),
        logging: dream_engine_config::logging::LoggingConfig::default(),
        vision: None,
    }
}

/// The reported bug: asking for 3000 lines from a provider with a small output
/// cap used to abort after a single failed continuation. The full answer must
/// now come back stitched together.
#[tokio::test]
async fn long_code_request_survives_repeated_output_truncation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(TruncatingEndpoint {
            calls: AtomicUsize::new(0),
        })
        .mount(&server)
        .await;

    let config = stub_config(server.uri());
    let provider = create_provider(&config);
    let output: Arc<dyn OutputSink> = Arc::new(TerminalSink::new(true));

    let mut engine =
        AgentEngine::new_with_provider(provider, config, ToolRegistry::new(), output, std::env::temp_dir());
    let result = engine
        .run("写一段3000行的Python代码，内容随意。", "")
        .await
        .expect("engine should recover the full answer");

    assert_eq!(
        result.stop_reason,
        StopReason::EndTurn,
        "a completed answer must not report MaxTokens"
    );

    let produced = result.text.matches("def f_").count();
    assert_eq!(
        produced, TOTAL_LINES,
        "every line must survive the truncation boundaries"
    );
    assert!(
        result.text.contains("def f_0()") && result.text.contains(&format!("def f_{}()", TOTAL_LINES - 1)),
        "the answer must span the first through the last line"
    );

    // Chunk boundaries are where a naive implementation drops or duplicates
    // content; check the seam between the first and second model turn.
    assert!(
        result.text.contains(&format!("def f_{}()", LINES_PER_CHUNK - 1))
            && result.text.contains(&format!("def f_{LINES_PER_CHUNK}()")),
        "no content may be lost across a truncation seam"
    );
    assert_eq!(
        result.text.matches("def f_0()").count(),
        1,
        "the continuation must not replay content the model already produced"
    );

    assert_eq!(
        server.received_requests().await.expect("requests recorded").len(),
        TOTAL_LINES.div_ceil(LINES_PER_CHUNK),
        "one request per chunk, no wasted turns"
    );
}

// ---------------------------------------------------------------------------
// truncated_write_tool_call_recovers_via_retry_with_tools_enabled
// ---------------------------------------------------------------------------

/// Serves, in order: (1) a `Write` tool call whose `content` argument is cut
/// off mid-string by `finish_reason: "length"`; (2) on retry, a complete
/// `Write` call for the same file; (3) a plain final answer once the tool
/// result comes back. Records whether the target file already existed by the
/// time the retry request arrived, to prove the truncated call in (1) never
/// touched disk.
struct TruncatedToolCallEndpoint {
    calls: AtomicUsize,
    file_path: String,
    file_existed_before_retry: Arc<Mutex<Option<bool>>>,
}

impl Respond for TruncatedToolCallEndpoint {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let mut body = String::new();

        match call {
            0 => {
                // Deliberately incomplete: no closing quote/brace, matching
                // what a real gateway sends when it cuts a stream off
                // mid-argument.
                let escaped_path = self.file_path.replace('\\', "\\\\");
                let partial_arguments =
                    format!("{{\"file_path\":\"{escaped_path}\",\"content\":\"first line\\nsecond line, still writing");
                body.push_str(&sse_frame(json!({
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "tool_calls": [{
                                "index": 0,
                                "id": "call_truncated",
                                "type": "function",
                                "function": { "name": "Write", "arguments": "" }
                            }]
                        }
                    }]
                })));
                body.push_str(&sse_frame(json!({
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "tool_calls": [{ "index": 0, "function": { "arguments": partial_arguments } }]
                        }
                    }]
                })));
                body.push_str(&sse_frame(json!({
                    "choices": [{ "index": 0, "delta": {}, "finish_reason": "length" }],
                    "usage": { "prompt_tokens": 100, "completion_tokens": 4096, "total_tokens": 4196 }
                })));
            }
            1 => {
                *self.file_existed_before_retry.lock().unwrap() = Some(std::path::Path::new(&self.file_path).exists());

                let full_arguments = json!({
                    "file_path": self.file_path,
                    "content": "first line\nsecond line\nthird line\n"
                })
                .to_string();
                body.push_str(&sse_frame(json!({
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "tool_calls": [{
                                "index": 0,
                                "id": "call_complete",
                                "type": "function",
                                "function": { "name": "Write", "arguments": full_arguments }
                            }]
                        }
                    }]
                })));
                body.push_str(&sse_frame(json!({
                    "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }],
                    "usage": { "prompt_tokens": 120, "completion_tokens": 64, "total_tokens": 184 }
                })));
            }
            _ => {
                // Post-tool-result turn: a normal final answer to end the run.
                body.push_str(&sse_frame(json!({
                    "choices": [{ "index": 0, "delta": { "content": "Done." } }]
                })));
                body.push_str(&sse_frame(json!({
                    "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 50, "completion_tokens": 10, "total_tokens": 60 }
                })));
            }
        }
        body.push_str("data: [DONE]\n\n");

        ResponseTemplate::new(200)
            .set_body_raw(body, "text/event-stream")
            .append_header("content-type", "text/event-stream")
    }
}

#[derive(Default)]
struct InfoRecordingSink {
    info_messages: Mutex<Vec<String>>,
}

impl OutputSink for InfoRecordingSink {
    fn emit_text_delta(&self, _text: &str, _msg_id: &str) {}
    fn emit_thinking(&self, _text: &str, _msg_id: &str) {}
    fn emit_tool_call(&self, _tool_use_id: &str, _name: &str, _input: &str) {}
    fn emit_tool_result(&self, _tool_use_id: &str, _name: &str, _is_error: bool, _content: &str) {}
    fn emit_stream_start(&self, _msg_id: &str) {}
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
    fn emit_error(&self, _msg: &str) {}
    fn emit_info(&self, msg: &str) {
        self.info_messages.lock().unwrap().push(msg.to_string());
    }
}

/// The reported bug: a `Write` call truncated mid-argument by the output
/// limit used to be dropped in silence — the continuation just kept talking
/// as plain text with tools disabled, so the chat looked finished while the
/// file was never created. The call must now be retried with tools enabled,
/// and the file must exist with the correct content once the retry lands.
#[tokio::test]
async fn truncated_write_tool_call_recovers_via_retry_with_tools_enabled() {
    let dir = tempdir().expect("tempdir");
    let file_path = dir.path().join("task_manager.py");
    let file_path_str = file_path.to_string_lossy().into_owned();
    let file_existed_before_retry = Arc::new(Mutex::new(None));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(TruncatedToolCallEndpoint {
            calls: AtomicUsize::new(0),
            file_path: file_path_str,
            file_existed_before_retry: Arc::clone(&file_existed_before_retry),
        })
        .mount(&server)
        .await;

    let config = stub_config(server.uri());
    let provider = create_provider(&config);
    let output = Arc::new(InfoRecordingSink::default());

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(WriteTool::new(None)));

    let mut engine =
        AgentEngine::new_with_provider(provider, config, registry, output.clone(), dir.path().to_path_buf());
    let result = engine
        .run("Write a small Python file.", "")
        .await
        .expect("engine should recover from the truncated tool call");

    assert_eq!(
        result.stop_reason,
        StopReason::EndTurn,
        "the run must finish normally once the retry succeeds"
    );

    assert_eq!(
        *file_existed_before_retry.lock().unwrap(),
        Some(false),
        "the truncated call must never have written anything to disk"
    );

    let written = std::fs::read_to_string(&file_path).expect("file should exist after the retry succeeds");
    assert_eq!(written, "first line\nsecond line\nthird line\n");

    {
        let info_messages = output.info_messages.lock().unwrap();
        assert!(
            info_messages
                .iter()
                .any(|m| m.contains("Write") && m.contains("cut off")),
            "a visible truncation notice naming the tool must be emitted, got: {info_messages:?}"
        );
    }

    let requests = server.received_requests().await.expect("requests recorded");
    assert_eq!(requests.len(), 3, "truncated turn + retry turn + post-tool-result turn");

    let retry_body: serde_json::Value = requests[1]
        .body_json()
        .expect("retry request body should be valid JSON");
    assert!(
        retry_body.get("tools").is_some(),
        "the retry turn must keep tools enabled, unlike the plain-text truncation continuation"
    );
}

// ---------------------------------------------------------------------------
// anthropic_truncated_write_tool_call_recovers_via_retry_with_tools_enabled
//
// Same scenario as the OpenAI test above, but over the Anthropic native wire
// protocol (/v1/messages, event-framed SSE). Anthropic differs from OpenAI:
// instead of dropping the half-streamed tool call, its content_block_stop used
// to collapse the incomplete input JSON into `{}` and emit a normal ToolUse —
// so the Write would run with empty arguments instead of being retried. This
// proves the collapse is now surfaced as a truncated call and recovered.
// ---------------------------------------------------------------------------

fn anthropic_frame(event: &str, value: serde_json::Value) -> String {
    format!("event: {event}\ndata: {value}\n\n")
}

fn stub_config_anthropic(base_url: String) -> Config {
    Config {
        provider: ProviderType::Anthropic,
        provider_label: "anthropic-stub".to_string(),
        model: "claude-stub".to_string(),
        compat: ProviderCompat::anthropic_defaults(),
        ..stub_config(base_url)
    }
}

struct AnthropicTruncatedToolCallEndpoint {
    calls: AtomicUsize,
    file_path: String,
    file_existed_before_retry: Arc<Mutex<Option<bool>>>,
}

impl Respond for AnthropicTruncatedToolCallEndpoint {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let mut body = String::new();

        match call {
            0 => {
                // A Write tool_use block whose input_json_delta is cut off
                // mid-string, then closed by content_block_stop and terminated
                // with stop_reason: max_tokens — exactly what a real gateway
                // sends when the output limit lands inside the arguments.
                let escaped_path = self.file_path.replace('\\', "\\\\");
                let partial = format!("{{\"file_path\":\"{escaped_path}\",\"content\":\"first line\\nstill writing");
                body.push_str(&anthropic_frame(
                    "message_start",
                    json!({"type":"message_start","message":{"usage":{"input_tokens":100}}}),
                ));
                body.push_str(&anthropic_frame(
                    "content_block_start",
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_trunc","name":"Write"}}),
                ));
                body.push_str(&anthropic_frame(
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":partial}}),
                ));
                body.push_str(&anthropic_frame(
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":0}),
                ));
                body.push_str(&anthropic_frame(
                    "message_delta",
                    json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":4096}}),
                ));
                body.push_str(&anthropic_frame("message_stop", json!({"type":"message_stop"})));
            }
            1 => {
                *self.file_existed_before_retry.lock().unwrap() = Some(std::path::Path::new(&self.file_path).exists());

                let full_input = json!({
                    "file_path": self.file_path,
                    "content": "first line\nsecond line\nthird line\n"
                });
                body.push_str(&anthropic_frame(
                    "message_start",
                    json!({"type":"message_start","message":{"usage":{"input_tokens":120}}}),
                ));
                body.push_str(&anthropic_frame(
                    "content_block_start",
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_complete","name":"Write"}}),
                ));
                body.push_str(&anthropic_frame(
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":full_input.to_string()}}),
                ));
                body.push_str(&anthropic_frame(
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":0}),
                ));
                body.push_str(&anthropic_frame(
                    "message_delta",
                    json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":64}}),
                ));
                body.push_str(&anthropic_frame("message_stop", json!({"type":"message_stop"})));
            }
            _ => {
                body.push_str(&anthropic_frame(
                    "message_start",
                    json!({"type":"message_start","message":{"usage":{"input_tokens":50}}}),
                ));
                body.push_str(&anthropic_frame(
                    "content_block_start",
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                ));
                body.push_str(&anthropic_frame(
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Done."}}),
                ));
                body.push_str(&anthropic_frame(
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":0}),
                ));
                body.push_str(&anthropic_frame(
                    "message_delta",
                    json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":10}}),
                ));
                body.push_str(&anthropic_frame("message_stop", json!({"type":"message_stop"})));
            }
        }

        ResponseTemplate::new(200)
            .set_body_raw(body, "text/event-stream")
            .append_header("content-type", "text/event-stream")
    }
}

#[tokio::test]
async fn anthropic_truncated_write_tool_call_recovers_via_retry_with_tools_enabled() {
    let dir = tempdir().expect("tempdir");
    let file_path = dir.path().join("task_manager.py");
    let file_path_str = file_path.to_string_lossy().into_owned();
    let file_existed_before_retry = Arc::new(Mutex::new(None));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(AnthropicTruncatedToolCallEndpoint {
            calls: AtomicUsize::new(0),
            file_path: file_path_str,
            file_existed_before_retry: Arc::clone(&file_existed_before_retry),
        })
        .mount(&server)
        .await;

    let config = stub_config_anthropic(server.uri());
    let provider = create_provider(&config);
    let output = Arc::new(InfoRecordingSink::default());

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(WriteTool::new(None)));

    let mut engine =
        AgentEngine::new_with_provider(provider, config, registry, output.clone(), dir.path().to_path_buf());
    let result = engine
        .run("Write a small Python file.", "")
        .await
        .expect("engine should recover from the truncated Anthropic tool call");

    assert_eq!(result.stop_reason, StopReason::EndTurn);

    assert_eq!(
        *file_existed_before_retry.lock().unwrap(),
        Some(false),
        "the truncated call must never have written anything (not even empty `{{}}` args)"
    );

    let written = std::fs::read_to_string(&file_path).expect("file should exist after the retry succeeds");
    assert_eq!(written, "first line\nsecond line\nthird line\n");

    {
        let info_messages = output.info_messages.lock().unwrap();
        assert!(
            info_messages
                .iter()
                .any(|m| m.contains("Write") && m.contains("cut off")),
            "a visible truncation notice naming the tool must be emitted, got: {info_messages:?}"
        );
    }

    let requests = server.received_requests().await.expect("requests recorded");
    assert_eq!(requests.len(), 3, "truncated turn + retry turn + post-tool-result turn");

    let retry_body: serde_json::Value = requests[1]
        .body_json()
        .expect("retry request body should be valid JSON");
    assert!(
        retry_body.get("tools").is_some(),
        "the retry turn must keep tools enabled"
    );
}

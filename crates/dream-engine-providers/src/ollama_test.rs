use super::*;
use dream_engine_types::message::StopReason;

#[test]
fn chat_body_maps_max_tokens_and_num_ctx_into_options() {
    let openai_body = serde_json::json!({
        "model": "qwen3:8b",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true,
        "max_tokens": 32000,
        "stream_options": {"include_usage": true}
    });

    let body = to_ollama_chat_body(&openai_body, Some(8192));

    assert_eq!(body["model"], "qwen3:8b");
    assert_eq!(body["stream"], true);
    assert_eq!(body["options"]["num_predict"], 32000);
    assert_eq!(body["options"]["num_ctx"], 8192);
    assert!(body.get("max_tokens").is_none());
    assert!(body.get("stream_options").is_none());
}

#[test]
fn chat_body_omits_options_when_nothing_configured() {
    let openai_body = serde_json::json!({
        "model": "qwen3:8b",
        "messages": [],
        "stream": true
    });

    let body = to_ollama_chat_body(&openai_body, None);

    assert!(body.get("options").is_none());
}

#[test]
fn chat_body_converts_tool_call_arguments_string_to_object() {
    let openai_body = serde_json::json!({
        "model": "qwen3:8b",
        "messages": [
            {"role": "user", "content": "read the file"},
            {"role": "assistant", "content": null, "tool_calls": [
                {"id": "call_1", "type": "function", "function": {"name": "Read", "arguments": "{\"path\":\"/tmp/a\"}"}}
            ]},
            {"role": "tool", "tool_call_id": "call_1", "content": "file body"}
        ],
        "stream": true
    });

    let body = to_ollama_chat_body(&openai_body, None);

    assert_eq!(
        body["messages"][1]["tool_calls"][0]["function"]["arguments"],
        serde_json::json!({"path": "/tmp/a"})
    );
    // Non-tool messages pass through untouched.
    assert_eq!(body["messages"][0], openai_body["messages"][0]);
    assert_eq!(body["messages"][2], openai_body["messages"][2]);
}

#[test]
fn chat_body_maps_thinking_to_think_flag() {
    let enabled = serde_json::json!({"model": "m", "messages": [], "thinking": {"type": "enabled"}});
    let disabled = serde_json::json!({"model": "m", "messages": [], "thinking": {"type": "disabled"}});

    assert_eq!(to_ollama_chat_body(&enabled, None)["think"], true);
    assert_eq!(to_ollama_chat_body(&disabled, None)["think"], false);
}

#[test]
fn ndjson_content_line_emits_text_delta() {
    let mut state = OllamaStreamState::new();
    let events = parse_ollama_ndjson_line(
        r#"{"model":"qwen3:8b","message":{"role":"assistant","content":"hello"},"done":false}"#,
        &mut state,
    );

    assert!(matches!(&events[..], [LlmEvent::TextDelta(text)] if text == "hello"));
}

#[test]
fn ndjson_thinking_line_emits_thinking_delta() {
    let mut state = OllamaStreamState::new();
    let events = parse_ollama_ndjson_line(
        r#"{"message":{"role":"assistant","thinking":"let me think","content":""},"done":false}"#,
        &mut state,
    );

    assert!(matches!(&events[..], [LlmEvent::ThinkingDelta(text)] if text == "let me think"));
}

#[test]
fn ndjson_tool_call_line_emits_tool_use_with_object_arguments() {
    let mut state = OllamaStreamState::new();
    let events = parse_ollama_ndjson_line(
        r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"Read","arguments":{"path":"/tmp/a"}}}]},"done":false}"#,
        &mut state,
    );

    assert!(
        matches!(&events[..], [LlmEvent::ToolUse { name, input, .. }] if name == "Read" && input["path"] == "/tmp/a")
    );
}

#[test]
fn ndjson_done_line_emits_done_with_usage() {
    let mut state = OllamaStreamState::new();
    let events = parse_ollama_ndjson_line(
        r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":120,"eval_count":34}"#,
        &mut state,
    );

    assert!(matches!(
        &events[..],
        [LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage
        }] if usage.input_tokens == 120 && usage.output_tokens == 34
    ));
}

#[test]
fn ndjson_done_after_tool_use_reports_tool_use_stop() {
    let mut state = OllamaStreamState::new();
    parse_ollama_ndjson_line(
        r#"{"message":{"role":"assistant","tool_calls":[{"function":{"name":"Read","arguments":{}}}]},"done":false}"#,
        &mut state,
    );
    let events = parse_ollama_ndjson_line(
        r#"{"done":true,"done_reason":"stop","prompt_eval_count":10,"eval_count":5}"#,
        &mut state,
    );

    assert!(matches!(
        &events[..],
        [LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            ..
        }]
    ));
}

#[test]
fn ndjson_done_reason_length_reports_max_tokens() {
    let mut state = OllamaStreamState::new();
    let events = parse_ollama_ndjson_line(r#"{"done":true,"done_reason":"length"}"#, &mut state);

    assert!(matches!(
        &events[..],
        [LlmEvent::Done {
            stop_reason: StopReason::MaxTokens,
            ..
        }]
    ));
}

#[test]
fn ndjson_error_line_is_recorded_as_stream_error() {
    let mut state = OllamaStreamState::new();
    let events = parse_ollama_ndjson_line(
        r#"{"error":"llm: context overflow - prompt exceeds the available context window"}"#,
        &mut state,
    );

    assert!(events.is_empty());
    let error = state.take_stream_error().expect("error frame should be recorded");
    assert!(matches!(error, ProviderError::PromptTooLong(_)));
}

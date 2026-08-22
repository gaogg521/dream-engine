#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use dream_engine_config::compat::{ProviderCompat, TransportCompat};
    use dream_engine_types::llm::{LlmEvent, LlmRequest};
    use dream_engine_types::message::{ContentBlock, Message, Role};
    use dream_engine_types::tool::ToolDef;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::composed::ComposedProvider;
    use crate::transport::{OpenAiTransport, ProviderTransport};
    use crate::{LlmProvider, ProviderError};

    fn test_request() -> LlmRequest {
        LlmRequest {
            model: "test-model".to_string(),
            system: "You are a test assistant.".to_string(),
            messages: vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            )],
            tools: vec![],
            max_tokens: Some(8192),
            thinking: None,
            reasoning_effort: None,
        }
    }

    #[test]
    fn composed_provider_build_request_body_matches_openai_projection() {
        let compat = ProviderCompat::openai_defaults();
        let provider = ComposedProvider::new(
            ProviderTransport::OpenAi(OpenAiTransport::new("test-key", "https://example.test")),
            compat,
        );

        let body = provider
            .build_request_body(&test_request())
            .expect("request body projection should succeed");

        assert_eq!(body["model"], "test-model");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "Hello");
    }

    #[test]
    fn openai_projection_omits_thinking_field_when_not_explicitly_requested() {
        // supports_thinking is a UI capability-exposure flag only; it must
        // not by itself cause the OpenAI-protocol path to declare thinking
        // mode. Auto-forcing this broke gateways that treat a declared
        // `thinking.type=enabled` as a strict replay contract they can't
        // satisfy on follow-up turns (see compat.rs comment on
        // `openai_defaults()` for the upstream issue this matches).
        let compat = ProviderCompat::openai_defaults();
        assert!(
            !compat.supports_thinking(),
            "openai_defaults() should not force thinking on by default"
        );
        let provider = openai_provider(compat);
        let mut req = test_request();
        req.thinking = None;
        let body = provider
            .build_request_body(&req)
            .expect("request body projection should succeed");
        assert!(body.get("thinking").is_none(), "body was: {body}");
    }

    #[test]
    fn openai_projection_sends_thinking_enabled_when_explicitly_requested() {
        let provider = openai_provider(ProviderCompat::openai_defaults());
        let mut req = test_request();
        req.thinking = Some(dream_engine_types::llm::ThinkingConfig::Enabled { budget_tokens: 8000 });
        let body = provider
            .build_request_body(&req)
            .expect("request body projection should succeed");
        assert_eq!(body["thinking"]["type"], "enabled", "body was: {body}");
    }

    #[test]
    fn test_max_tokens_field_default() {
        let provider = openai_provider(ProviderCompat::openai_defaults());
        let req = LlmRequest {
            model: "gpt-4o".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: Some(1024),
            thinking: None,
            reasoning_effort: None,
        };
        let body = provider
            .build_request_body(&req)
            .expect("request body projection should succeed");
        assert_eq!(body["max_tokens"], 1024);
        assert!(body.get("max_completion_tokens").is_none());
    }

    #[test]
    fn test_max_tokens_field_custom() {
        let compat = ProviderCompat {
            transport: TransportCompat {
                max_tokens_field: Some("max_completion_tokens".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let provider = openai_provider(compat);
        let req = LlmRequest {
            model: "gpt-4o".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: Some(2048),
            thinking: None,
            reasoning_effort: None,
        };
        let body = provider
            .build_request_body(&req)
            .expect("request body projection should succeed");
        assert_eq!(body["max_completion_tokens"], 2048);
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn test_projection_limit_maps_to_non_retryable_prompt_too_long() {
        let mut compat = ProviderCompat::openai_defaults();
        compat.tools.max_tool_count = Some(0);
        let provider = openai_provider(compat);
        let req = LlmRequest {
            model: "gpt-4o".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![ToolDef {
                name: "read".into(),
                description: "Read".into(),
                input_schema: json!({"type":"object","properties":{}}),
                deferred: false,
            }],
            max_tokens: Some(1024),
            thinking: None,
            reasoning_effort: None,
        };

        let error = provider
            .build_request_body(&req)
            .expect_err("projection limit should map to provider error");

        match &error {
            ProviderError::PromptTooLong(message) => {
                assert!(message.contains("openai tools count 1 exceeds configured limit 0"));
            }
            other => panic!("unexpected provider error: {other}"),
        }
        assert!(!error.is_retryable());
    }

    fn openai_provider(compat: ProviderCompat) -> ComposedProvider {
        ComposedProvider::new(
            ProviderTransport::OpenAi(OpenAiTransport::new("test-key", "https://example.test/v1")),
            compat,
        )
    }

    fn golden_req(messages: Vec<Message>, tools: Vec<ToolDef>) -> LlmRequest {
        LlmRequest {
            model: "test-model".to_string(),
            system: "You are a test assistant.".to_string(),
            messages,
            tools,
            max_tokens: Some(8192),
            thinking: None,
            reasoning_effort: None,
        }
    }

    macro_rules! assert_openai_json_snapshot {
        ($name:literal, $value:expr) => {
            insta::with_settings!({ prepend_module_to_snapshot => false }, {
                insta::assert_json_snapshot!(
                    concat!("dream_engine_providers__openai__tests__", $name),
                    $value
                );
            });
        };
    }

    #[test]
    fn golden_openai_basic() {
        let provider = openai_provider(ProviderCompat::openai_defaults());
        let request = golden_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            )],
            vec![],
        );
        let body = provider
            .build_request_body(&request)
            .expect("request body projection should succeed");
        assert_openai_json_snapshot!("openai_basic", body);
    }

    fn sample_tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "read".to_string(),
                description: "Read a file".to_string(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
                deferred: false,
            },
            ToolDef {
                name: "list".to_string(),
                description: "List dir".to_string(),
                input_schema: json!({"type": "object", "properties": {}}),
                deferred: false,
            },
        ]
    }

    #[test]
    fn golden_openai_with_tools() {
        let provider = openai_provider(ProviderCompat::openai_defaults());
        let request = golden_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "go".to_string() }],
            )],
            sample_tools(),
        );
        assert_openai_json_snapshot!(
            "openai_with_tools",
            provider
                .build_request_body(&request)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_openai_with_tool_result() {
        let provider = openai_provider(ProviderCompat::openai_defaults());
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                    input: json!({"path": "a.txt"}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "file contents".to_string(),
                    is_error: false,
                }],
            ),
        ];
        assert_openai_json_snapshot!(
            "openai_with_tool_result",
            provider
                .build_request_body(&golden_req(messages, vec![]))
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_openai_with_thinking() {
        let provider = openai_provider(ProviderCompat::openai_defaults());
        let messages = vec![
            Message::new(Role::User, vec![ContentBlock::Text { text: "q1".to_string() }]),
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::Thinking {
                        thinking: "let me think".to_string(),
                        signature: None,
                    },
                    ContentBlock::Text {
                        text: "answer".to_string(),
                    },
                ],
            ),
            Message::new(Role::User, vec![ContentBlock::Text { text: "q2".to_string() }]),
        ];
        assert_openai_json_snapshot!(
            "openai_with_thinking",
            provider
                .build_request_body(&golden_req(messages, vec![]))
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_openai_with_reasoning_effort() {
        let provider = openai_provider(ProviderCompat::openai_defaults());
        let mut request = golden_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "hi".to_string() }],
            )],
            vec![],
        );
        request.reasoning_effort = Some("medium".to_string());
        assert_openai_json_snapshot!(
            "openai_with_reasoning_effort",
            provider
                .build_request_body(&request)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_openai_custom_max_tokens_field() {
        let mut compat = ProviderCompat::openai_defaults();
        compat.transport.max_tokens_field = Some("max_completion_tokens".to_string());
        let provider = openai_provider(compat);
        let request = golden_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "hi".to_string() }],
            )],
            vec![],
        );
        assert_openai_json_snapshot!(
            "openai_custom_max_tokens_field",
            provider
                .build_request_body(&request)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_openai_field_controls_disabled() {
        let mut compat = ProviderCompat::openai_defaults();
        compat.transport.include_stream_options = Some(false);
        compat.tools.emit_tools = Some(false);
        compat.reasoning.supports_effort = Some(false);
        let provider = openai_provider(compat);
        let mut request = golden_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "hi".to_string() }],
            )],
            sample_tools(),
        );
        request.reasoning_effort = Some("medium".to_string());

        assert_openai_json_snapshot!(
            "openai_field_controls_disabled",
            provider
                .build_request_body(&request)
                .expect("request body projection should succeed")
        );
    }

    #[tokio::test]
    async fn composed_provider_stream_emits_openai_text_delta() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                concat!(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
                    "data: [DONE]\n\n"
                ),
                "text/event-stream",
            ))
            .mount(&server)
            .await;

        let compat = ProviderCompat::openai_defaults();
        let provider = ComposedProvider::new(
            ProviderTransport::OpenAi(OpenAiTransport::new("test-key", &server.uri())),
            compat,
        );

        let mut rx = provider.stream(&test_request()).await.expect("stream should start");

        assert!(matches!(
            rx.recv().await,
            Some(LlmEvent::TextDelta(text)) if text == "hi"
        ));
    }

    #[tokio::test]
    async fn composed_provider_retries_with_content_block_thinking_on_gateway_rejection() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_raw(
                r#"{"error":{"message":"backend error: status=400 body={\"error\":{\"message\":\"The `content[].thinking` in the thinking mode must be passed back to the API.\"}}"}}"#,
                "application/json",
            ))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                concat!(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
                    "data: [DONE]\n\n"
                ),
                "text/event-stream",
            ))
            .mount(&server)
            .await;

        let compat = ProviderCompat::openai_defaults();
        let provider = ComposedProvider::new(
            ProviderTransport::OpenAi(OpenAiTransport::new("test-key", &server.uri())),
            compat,
        );

        let request = golden_req(
            vec![
                Message::new(Role::User, vec![ContentBlock::Text { text: "q1".to_string() }]),
                Message::new(
                    Role::Assistant,
                    vec![
                        ContentBlock::Thinking {
                            thinking: "reasoning...".to_string(),
                            signature: None,
                        },
                        ContentBlock::Text {
                            text: "answer".to_string(),
                        },
                    ],
                ),
                Message::new(Role::User, vec![ContentBlock::Text { text: "q2".to_string() }]),
            ],
            vec![],
        );

        let mut rx = provider
            .stream(&request)
            .await
            .expect("stream should succeed after one retry");
        assert!(matches!(
            rx.recv().await,
            Some(LlmEvent::TextDelta(text)) if text == "ok"
        ));

        let received = server.received_requests().await.expect("wiremock records requests");
        assert_eq!(
            received.len(),
            2,
            "should send the original request then exactly one retry"
        );

        let retried_body: serde_json::Value = received[1].body_json().expect("retry body is valid json");
        let assistant_msg = retried_body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "assistant")
            .expect("retried request still carries the assistant history");
        assert!(
            assistant_msg.get("reasoning_content").is_none(),
            "retry must not send reasoning_content"
        );
        let content = assistant_msg["content"]
            .as_array()
            .expect("retry must replay thinking as a content[] array");
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "reasoning...");
    }

    /// Multi-turn tool-call history for the escalation tests below.
    fn tool_history_request() -> LlmRequest {
        golden_req(
            vec![
                Message::new(Role::User, vec![ContentBlock::Text { text: "q1".to_string() }]),
                Message::new(
                    Role::Assistant,
                    vec![
                        ContentBlock::Thinking {
                            thinking: "let me call the tool".to_string(),
                            signature: None,
                        },
                        ContentBlock::ToolUse {
                            id: "call_1".to_string(),
                            name: "Write".to_string(),
                            input: json!({"path": "a.txt"}),
                            extra: None,
                        },
                    ],
                ),
                Message::new(
                    Role::User,
                    vec![ContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: "wrote a.txt".to_string(),
                        is_error: false,
                    }],
                ),
            ],
            sample_tools(),
        )
    }

    #[tokio::test]
    async fn composed_provider_escalates_to_textualized_tool_replay_and_sticks() {
        let server = MockServer::start().await;

        // The gateway (as observed live: LiteLLM-style fronting DeepSeek V4
        // thinking) rejects EVERY shape that carries assistant tool_calls,
        // regardless of thinking replay format. Only textualized history
        // passes. Match that: fail the first three attempts, then accept.
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_raw(
                r#"{"error":{"message":"backend error: status=400 body={\"error\":{\"message\":\"The `content[].thinking` in the thinking mode must be passed back to the API.\"}}"}}"#,
                "application/json",
            ))
            .up_to_n_times(3)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                concat!(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
                    "data: [DONE]\n\n"
                ),
                "text/event-stream",
            ))
            .mount(&server)
            .await;

        let provider = ComposedProvider::new(
            ProviderTransport::OpenAi(OpenAiTransport::new("test-key", &server.uri())),
            ProviderCompat::openai_defaults(),
        );

        let request = tool_history_request();
        let mut rx = provider
            .stream(&request)
            .await
            .expect("stream should succeed after escalating to textualized replay");
        assert!(matches!(
            rx.recv().await,
            Some(LlmEvent::TextDelta(text)) if text == "ok"
        ));

        let received = server.received_requests().await.expect("wiremock records requests");
        assert_eq!(received.len(), 4, "original + three escalation retries");

        let final_body: serde_json::Value = received[3].body_json().expect("final body is valid json");
        let messages = final_body["messages"].as_array().unwrap();
        assert!(
            messages.iter().all(|m| m.get("tool_calls").is_none()),
            "textualized replay must not carry tool_calls: {final_body}"
        );
        assert!(
            messages.iter().all(|m| m["role"] != "tool"),
            "textualized replay must not carry tool role messages: {final_body}"
        );
        let assistant = messages.iter().find(|m| m["role"] == "assistant").unwrap();
        let text = assistant["content"].as_str().unwrap();
        assert!(text.contains("[tool_call Write call_1]"), "content was: {text}");
        assert!(
            messages
                .iter()
                .any(|m| m["role"] == "user" && m["content"].as_str().unwrap_or("").contains("[tool_result call_1]")),
            "tool result must be replayed as user text: {final_body}"
        );

        // Second turn on the same provider instance must go straight to the
        // learned textualized shape — exactly one more request, no re-probing.
        let mut rx2 = provider
            .stream(&request)
            .await
            .expect("second stream should succeed immediately");
        assert!(matches!(
            rx2.recv().await,
            Some(LlmEvent::TextDelta(text)) if text == "ok"
        ));
        let received = server.received_requests().await.expect("wiremock records requests");
        assert_eq!(received.len(), 5, "sticky level: second turn sends exactly one request");
        let sticky_body: serde_json::Value = received[4].body_json().expect("sticky body is valid json");
        assert!(
            sticky_body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m.get("tool_calls").is_none()),
            "sticky turn must reuse textualized replay: {sticky_body}"
        );
    }

    #[tokio::test]
    async fn composed_provider_retries_successful_json_502_then_streams() {
        let server = MockServer::start().await;
        let attempt = Arc::new(AtomicU32::new(0));
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with({
                let attempt = Arc::clone(&attempt);
                move |_request: &wiremock::Request| {
                    if attempt.fetch_add(1, Ordering::SeqCst) == 0 {
                        ResponseTemplate::new(200).set_body_json(json!({
                            "error": {
                                "message": "upstream busy",
                                "code": 502
                            }
                        }))
                    } else {
                        ResponseTemplate::new(200).set_body_raw(
                            concat!(
                                "data: {\"choices\":[{\"delta\":{\"content\":\"recovered\"}}]}\n\n",
                                "data: [DONE]\n\n"
                            ),
                            "text/event-stream",
                        )
                    }
                }
            })
            .expect(2)
            .mount(&server)
            .await;
        tokio::time::pause();

        let provider = ComposedProvider::new(
            ProviderTransport::OpenAi(OpenAiTransport::new("test-key", &server.uri())),
            ProviderCompat::openai_defaults(),
        );
        let mut rx = provider
            .stream(&test_request())
            .await
            .expect("embedded 502 should be retried");

        assert!(matches!(
            rx.recv().await,
            Some(LlmEvent::TextDelta(text)) if text == "recovered"
        ));
        assert_eq!(attempt.load(Ordering::SeqCst), 2);
        server.verify().await;
    }

    #[tokio::test]
    async fn composed_provider_returns_typed_502_after_retry_exhaustion() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "error": {
                    "message": "upstream busy",
                    "code": 502
                }
            })))
            .expect(6)
            .mount(&server)
            .await;
        tokio::time::pause();

        let provider = ComposedProvider::new(
            ProviderTransport::OpenAi(OpenAiTransport::new("test-key", &server.uri())),
            ProviderCompat::openai_defaults(),
        );

        let error = provider
            .stream(&test_request())
            .await
            .expect_err("exhausted retries should return a typed provider error");

        assert!(matches!(
            error,
            ProviderError::Api { status: 502, message } if message == "upstream busy"
        ));
        server.verify().await;
    }
}

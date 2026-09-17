use super::*;

#[cfg(test)]
mod retryable_tests {
    use super::*;

    // F1-11
    #[test]
    fn test_api_400_not_retryable() {
        assert!(
            !ProviderError::Api {
                status: 400,
                message: "empty name".into(),
            }
            .is_retryable()
        );
        assert!(
            ProviderError::RateLimited {
                retry_after_ms: 1000,
                body: None,
            }
            .is_retryable()
        );
        assert!(ProviderError::Connection("x".into()).is_retryable());
    }
}

#[cfg(test)]
mod json_error_body_tests {
    use super::*;

    #[test]
    fn json_top_level_error_shape_is_normalized() {
        let body_text = r#"{"code":"503","message":"upstream busy"}"#;
        let body = serde_json::from_str(body_text).expect("test body should be valid JSON");
        let error =
            provider_error_from_json_body(&body, body_text.as_bytes()).expect("status 503 should map to an error");

        assert!(matches!(
            error,
            ProviderError::Api { status: 503, message } if message == "upstream busy"
        ));
    }

    #[test]
    fn json_error_uses_http_status_when_provider_code_is_not_http() {
        let body_text = r#"{"error":{"code":1001,"message":"upstream busy"},"status":503}"#;
        let body = serde_json::from_str(body_text).expect("test body should be valid JSON");
        let error =
            provider_error_from_json_body(&body, body_text.as_bytes()).expect("status 503 should map to an error");

        assert!(matches!(
            error,
            ProviderError::Api { status: 503, message } if message == "upstream busy"
        ));
    }

    #[test]
    fn json_without_error_shape_is_not_mapped_to_an_error() {
        let body_text = r#"{"choices":[]}"#;
        let body = serde_json::from_str(body_text).expect("test body should be valid JSON");

        assert!(provider_error_from_json_body(&body, body_text.as_bytes()).is_none());
    }

    #[test]
    fn json_error_without_http_status_is_a_parse_error() {
        let body_text = r#"{"error":{"code":"resource_exhausted","message":"upstream busy"}}"#;
        let body = serde_json::from_str(body_text).expect("test body should be valid JSON");
        let error =
            provider_error_from_json_body(&body, body_text.as_bytes()).expect("explicit error should be surfaced");

        assert!(matches!(
            error,
            ProviderError::Parse(message) if message.contains("without an HTTP status")
        ));
    }

    #[test]
    fn null_error_field_is_not_mapped_to_an_error() {
        // Normal 200 bodies from some gateways carry `"error": null`; they
        // must pass through untouched on both the transport and SSE paths.
        let body_text = r#"{"error":null,"choices":[{"message":{"content":"hi"}}]}"#;
        let body = serde_json::from_str(body_text).expect("test body should be valid JSON");

        assert!(provider_error_from_json_body(&body, body_text.as_bytes()).is_none());
    }

    #[test]
    fn rate_limit_status_maps_to_rate_limited_with_body() {
        let body_text = r#"{"error":{"code":429,"message":"quota exceeded"}}"#;
        let body = serde_json::from_str(body_text).expect("test body should be valid JSON");
        let error =
            provider_error_from_json_body(&body, body_text.as_bytes()).expect("status 429 should map to an error");

        assert!(matches!(
            error,
            ProviderError::RateLimited { body: Some(body), .. } if body == body_text
        ));
    }

    #[test]
    fn ollama_context_overflow_maps_to_prompt_too_long() {
        // Real Ollama /v1/chat/completions 400 body when the prompt exceeds num_ctx.
        let body_text = r#"{"error":"llm: context overflow - prompt exceeds the available context window. Reduce the message length or increase the model's context size."}"#;
        let body = serde_json::from_str(body_text).expect("test body should be valid JSON");
        let error = provider_error_from_json_body(&body, body_text.as_bytes())
            .expect("context overflow should map to an error");

        assert!(matches!(error, ProviderError::PromptTooLong(message) if message.contains("context overflow")));
    }

    #[test]
    fn openai_context_length_exceeded_maps_to_prompt_too_long() {
        let body_text = r#"{"error":{"code":"context_length_exceeded","message":"This model's maximum context length is 8192 tokens. However, you requested 9000 tokens."}}"#;
        let body = serde_json::from_str(body_text).expect("test body should be valid JSON");
        let error = provider_error_from_json_body(&body, body_text.as_bytes())
            .expect("context overflow should map to an error");

        assert!(matches!(error, ProviderError::PromptTooLong(_)));
    }

    #[test]
    fn unrelated_4xx_does_not_map_to_prompt_too_long() {
        let body_text = r#"{"error":{"code":400,"message":"invalid api key"}}"#;
        let body = serde_json::from_str(body_text).expect("test body should be valid JSON");
        let error =
            provider_error_from_json_body(&body, body_text.as_bytes()).expect("status 400 should map to an error");

        assert!(matches!(error, ProviderError::Api { status: 400, .. }));
    }
}

// ── Recovering the real window from an overflow error ───────────────────────

#[test]
fn parses_the_window_openai_reports() {
    // Four numbers, and the limit is neither the smallest nor the first.
    // "Take the smallest" would learn 5500 and compact the session to nothing.
    let message = "This model's maximum context length is 128000 tokens. However, you requested \
                   130500 tokens (125000 in the messages, 5500 in the completion). Please reduce \
                   the length of the messages or completion.";
    assert_eq!(parse_context_limit(message), Some(128_000));
}

#[test]
fn parses_the_window_anthropic_reports() {
    // The number sits *before* the keyword here.
    let message = "prompt is too long: 215123 tokens > 200000 maximum";
    assert_eq!(parse_context_limit(message), Some(200_000));
}

#[test]
fn parses_a_window_stated_after_context_window() {
    assert_eq!(
        parse_context_limit("Input validation error: the context window is 32768 tokens"),
        Some(32_768)
    );
}

#[test]
fn reports_nothing_when_the_provider_states_no_number() {
    // Ollama. The caller must fall back to shrinking relative to what it sent
    // rather than inventing a limit.
    assert_eq!(
        parse_context_limit("llm: context overflow - prompt exceeds the available context window"),
        None
    );
    assert_eq!(parse_context_limit("context_length_exceeded"), None);
}

#[test]
fn rejects_numbers_too_small_or_too_large_to_be_a_window() {
    // A completion budget misread as a window would compact every turn; an id
    // misread as one would disable compaction entirely.
    assert_eq!(parse_context_limit("maximum context length is 12 tokens"), None);
    assert_eq!(parse_context_limit("maximum context length is 1699999999 tokens"), None);
}

#[test]
fn a_mid_stream_overflow_string_is_still_recognised() {
    // It arrives as a bare string once the SSE frame has lost its typing.
    assert!(is_context_overflow(
        "This model's maximum context length is 128000 tokens"
    ));
    assert!(is_context_overflow(
        "prompt is too long: 215123 tokens > 200000 maximum"
    ));
    assert!(!is_context_overflow("rate limit exceeded"));
}

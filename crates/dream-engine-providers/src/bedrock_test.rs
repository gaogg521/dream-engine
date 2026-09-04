use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use dream_engine_types::message::{ContentBlock, Message, Role};
    use dream_engine_types::tool::ToolDef;
    use serde_json::json;

    // --- Golden body snapshots (baseline for compat-split / seam-extraction refactors) ---

    fn bedrock_test_provider() -> BedrockProvider {
        BedrockProvider::new(
            "us-east-1",
            AwsCredentials::Explicit {
                access_key_id: "test-key".to_string(),
                secret_access_key: "test-secret".to_string(),
                session_token: None,
            },
            false,
            ProviderCompat::bedrock_defaults(),
            None,
            None,
        )
    }

    fn bedrock_req(messages: Vec<Message>, tools: Vec<ToolDef>) -> LlmRequest {
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

    fn bedrock_tools() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "read".to_string(),
            description: "Read".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {"path": {"type": ["string", "null"]}},
                "additionalProperties": false
            }),
            deferred: false,
        }]
    }

    macro_rules! assert_bedrock_json_snapshot {
        ($name:literal, $value:expr) => {
            insta::with_settings!({ prepend_module_to_snapshot => false }, {
                insta::assert_json_snapshot!(
                    concat!("dream_engine_providers__bedrock__tests__", $name),
                    $value
                );
            });
        };
    }

    #[test]
    fn golden_bedrock_basic() {
        let p = bedrock_test_provider();
        let r = bedrock_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            )],
            vec![],
        );
        assert_bedrock_json_snapshot!(
            "bedrock_basic",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_bedrock_with_tools() {
        let p = bedrock_test_provider();
        let r = bedrock_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "go".to_string() }],
            )],
            bedrock_tools(),
        );
        assert_bedrock_json_snapshot!(
            "bedrock_with_tools",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }

    // --- Endpoint override (enterprise model proxy / gateway) ---

    fn bedrock_state(base_url: Option<String>, bearer_token: Option<String>) -> BedrockTransportState {
        BedrockTransportState::new(
            "us-east-1",
            AwsCredentials::Explicit {
                access_key_id: "test-key".to_string(),
                secret_access_key: "test-secret".to_string(),
                session_token: None,
            },
            false,
            base_url,
            bearer_token,
        )
    }

    #[test]
    fn build_url_defaults_to_the_regional_aws_host() {
        let state = bedrock_state(None, None);
        assert_eq!(
            state.build_url("anthropic.claude-sonnet-4-20250514-v1:0"),
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/anthropic.claude-sonnet-4-20250514-v1:0/invoke-with-response-stream"
        );
    }

    #[test]
    fn build_url_honors_a_configured_base_url_and_trims_the_trailing_slash() {
        let state = bedrock_state(Some("https://gateway.internal/model-api/".to_string()), None);
        assert_eq!(
            state.build_url("claude-sonnet-4"),
            "https://gateway.internal/model-api/model/claude-sonnet-4/invoke-with-response-stream"
        );
    }

    /// Proxy mode: the channel token rides as `Authorization: Bearer` and no
    /// SigV4 headers are produced — the model proxy strips client authorization
    /// headers anyway, and the caller has no AWS credentials to sign with.
    #[test]
    fn a_bearer_token_replaces_sigv4_signing() {
        let state = bedrock_state(
            Some("https://gateway.internal/model-api/".to_string()),
            Some("onech-abc".to_string()),
        );

        let projected = state
            .build_projected_request(
                "claude-sonnet-4",
                json!({"messages": []}),
                &ProviderCompat::bedrock_defaults(),
                ResolvedToolWireShape::AnthropicInputSchema,
            )
            .expect("projected request should build");

        assert_eq!(
            projected.url,
            "https://gateway.internal/model-api/model/claude-sonnet-4/invoke-with-response-stream"
        );
        assert_eq!(
            projected.headers.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer onech-abc")
        );
        assert!(projected.headers.get("x-amz-date").is_none());
        assert!(projected.headers.get("x-amz-content-sha256").is_none());
        assert!(projected.body_bytes.is_some());
    }

    #[test]
    fn without_a_bearer_token_the_request_is_sigv4_signed_for_the_target_url() {
        let state = bedrock_state(None, None);

        let projected = state
            .build_projected_request(
                "claude-sonnet-4",
                json!({"messages": []}),
                &ProviderCompat::bedrock_defaults(),
                ResolvedToolWireShape::AnthropicInputSchema,
            )
            .expect("projected request should build");

        assert_eq!(
            projected.url,
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/claude-sonnet-4/invoke-with-response-stream"
        );
        let authorization = projected
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .expect("sigv4 request should carry an authorization header");
        assert!(authorization.starts_with("AWS4-HMAC-SHA256"), "got: {authorization}");
    }
}

use super::*;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use dream_engine_config::compat::ProviderCompat;
    use dream_engine_config::config::{CliArgs, McpServerConfig, ProviderType, TransportType, VisionModelConfig};
    use dream_engine_protocol::events::ToolCategory;
    use dream_engine_tools::Tool;
    use dream_engine_types::message::ImageInputCapability;
    use dream_engine_types::tool::ToolResult;
    use async_trait::async_trait;
    use serde_json::{Value, json};

    use crate::output::OutputSink;
    use crate::output::null_sink::NullSink;
    use crate::tool_policy::ToolPolicy;

    use super::*;

    struct DeferredTestTool(&'static str);

    #[async_trait]
    impl Tool for DeferredTestTool {
        fn name(&self) -> &str {
            self.0
        }

        fn description(&self) -> &str {
            "deferred test tool"
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }

        fn is_concurrency_safe(&self, _input: &Value) -> bool {
            true
        }

        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult {
                content: "unused".to_string(),
                is_error: false,
            }
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Info
        }

        fn is_deferred(&self) -> bool {
            true
        }
    }

    fn test_config() -> Config {
        Config::resolve(&CliArgs {
            provider: Some("anthropic".to_string()),
            api_key: Some("sk-test".to_string()),
            base_url: None,
            model: Some("claude-sonnet-4-20250514".to_string()),
            max_tokens: Some(4096),
            thinking: None,
            thinking_budget: None,
            max_turns: None,
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: None,
        })
        .unwrap()
    }

    #[test]
    fn mcp_servers_with_runtime_env_uses_server_env_as_override() {
        let mut config = test_config();
        config.mcp.servers.insert(
            "stdio".to_string(),
            McpServerConfig {
                transport: TransportType::Stdio,
                command: Some("server".to_string()),
                args: None,
                env: Some(HashMap::from([
                    ("OVERRIDE".to_string(), "server".to_string()),
                    ("SERVER_ONLY".to_string(), "1".to_string()),
                ])),
                url: None,
                headers: None,
                deferred: None,
                startup_timeout_ms: None,
            },
        );

        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let bootstrap = AgentBootstrap::new(config, "/tmp", output).runtime_env(vec![
            ("OVERRIDE".to_string(), "runtime".to_string()),
            ("RUNTIME_ONLY".to_string(), "1".to_string()),
        ]);

        let servers = bootstrap.mcp_servers_with_runtime_env();
        let env = servers
            .get("stdio")
            .and_then(|server| server.env.as_ref())
            .expect("stdio server env should exist");

        assert_eq!(env.get("OVERRIDE").map(String::as_str), Some("server"));
        assert_eq!(env.get("SERVER_ONLY").map(String::as_str), Some("1"));
        assert_eq!(env.get("RUNTIME_ONLY").map(String::as_str), Some("1"));
    }

    #[tokio::test]
    async fn tool_search_snapshot_excludes_policy_denied_tools() {
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let bootstrap = AgentBootstrap::new(test_config(), "/tmp", output)
            .tool_policy(ToolPolicy::allow_only(["ToolSearch", "AllowedDeferred"]));
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DeferredTestTool("AllowedDeferred")));
        registry.register(Box::new(DeferredTestTool("DeniedDeferred")));

        bootstrap.register_tool_search(&mut registry);

        let tool_search = registry.get("ToolSearch").expect("ToolSearch should be registered");
        let allowed = tool_search.execute(json!({"query": "AllowedDeferred"})).await;
        let denied = tool_search.execute(json!({"query": "DeniedDeferred"})).await;

        assert!(allowed.content.contains("AllowedDeferred"));
        assert!(denied.content.starts_with("No deferred tools matching"));
        assert!(!denied.content.contains("\"name\": \"DeniedDeferred\""));
    }

    fn text_only_config() -> Config {
        let mut config = test_config();
        config.model = "deepseek-v4-flash".to_string();
        config.compat.image_input = Some(ImageInputCapability::Unsupported);
        config.vision = None;
        config
    }

    fn vision_delegate() -> VisionModelConfig {
        VisionModelConfig {
            provider_label: "openai".to_string(),
            provider: ProviderType::OpenAI,
            api_key: "sk-vision".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o".to_string(),
            compat: ProviderCompat::openai_official_defaults(),
        }
    }

    /// Registration must not be conditional on a vision model existing: without
    /// the tool present the model has nothing to call and reports back to the
    /// user by inventing the image contents.
    #[test]
    fn builtin_registry_registers_read_image_even_without_a_vision_model() {
        let config = text_only_config();
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let bootstrap = AgentBootstrap::new(config, "/tmp", output);
        let provider = create_provider(bootstrap.config());

        let registry = bootstrap.build_builtin_registry(Path::new("/tmp"), &provider);

        assert!(registry.tool_names().iter().any(|name| name == "ReadImage"));
    }

    #[test]
    fn vision_backend_is_absent_for_a_text_only_model_with_no_delegate() {
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let bootstrap = AgentBootstrap::new(text_only_config(), "/tmp", output);
        let provider = create_provider(bootstrap.config());

        assert!(bootstrap.resolve_vision_backend(&provider).is_none());
    }

    #[test]
    fn vision_backend_uses_the_configured_delegate_for_a_text_only_model() {
        let mut config = text_only_config();
        config.vision = Some(vision_delegate());
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let bootstrap = AgentBootstrap::new(config, "/tmp", output);
        let provider = create_provider(bootstrap.config());

        assert!(bootstrap.resolve_vision_backend(&provider).is_some());
    }

    /// A vision-capable main model can read the image itself, so `ReadImage`
    /// stays usable without any extra configuration.
    #[test]
    fn vision_backend_falls_back_to_a_vision_capable_main_model() {
        let mut config = text_only_config();
        config.compat.image_input = Some(ImageInputCapability::Supported);
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let bootstrap = AgentBootstrap::new(config, "/tmp", output);
        let provider = create_provider(bootstrap.config());

        assert!(bootstrap.resolve_vision_backend(&provider).is_some());
    }
}

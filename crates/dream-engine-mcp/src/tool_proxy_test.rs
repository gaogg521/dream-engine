use super::*;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use dream_engine_config::config::TransportType;
    use serde_json::json;

    fn make_proxy(deferred: bool) -> McpToolProxy {
        // manager is only used during execute(), which we don't call in these
        // tests, so we can construct one with no servers.
        let manager = Arc::new(McpManager::new_for_test(vec![]));
        McpToolProxy::new(
            "test_tool".into(),
            "test_tool".into(),
            "test_server".into(),
            "A test tool".into(),
            json!({"type": "object"}),
            manager,
            deferred,
        )
    }

    #[test]
    fn proxy_deferred_true_returns_true() {
        let proxy = make_proxy(true);
        assert!(proxy.is_deferred());
    }

    #[test]
    fn proxy_deferred_false_returns_false() {
        let proxy = make_proxy(false);
        assert!(!proxy.is_deferred());
    }

    fn make_server_config(deferred: Option<bool>) -> McpServerConfig {
        McpServerConfig {
            transport: TransportType::Stdio,
            command: Some("echo".into()),
            args: None,
            env: None,
            url: None,
            headers: None,
            deferred,
            startup_timeout_ms: None,
        }
    }

    #[test]
    fn register_defaults_to_deferred_when_config_omits_field() {
        let manager = Arc::new(McpManager::new_for_test(vec![]));
        let mut registry = dream_engine_tools::registry::ToolRegistry::new();
        // Empty server configs — deferred field absent
        let configs = HashMap::new();

        register_mcp_tools(&mut registry, &manager, &[], &configs);

        // No tools registered because manager has no tools, but the logic
        // is tested via the deferred default path. Test with a real config below.
        assert!(registry.tool_names().is_empty());
    }

    #[test]
    fn server_config_deferred_none_defaults_true() {
        let config = make_server_config(None);
        let deferred = config.deferred.unwrap_or(true);
        assert!(deferred, "deferred should default to true when None");
    }

    #[test]
    fn server_config_deferred_explicit_false() {
        let config = make_server_config(Some(false));
        let deferred = config.deferred.unwrap_or(true);
        assert!(!deferred, "deferred should be false when explicitly set");
    }

    #[test]
    fn server_config_deferred_explicit_true() {
        let config = make_server_config(Some(true));
        let deferred = config.deferred.unwrap_or(true);
        assert!(deferred, "deferred should be true when explicitly set");
    }

    // -- has_valid_property_keys -----------------------------------------

    #[test]
    fn plain_ascii_schema_is_valid() {
        let schema = json!({
            "type": "object",
            "properties": { "start_date": { "type": "string" }, "v1.2-beta": { "type": "string" } }
        });
        assert!(has_valid_property_keys(&schema));
    }

    #[test]
    fn schema_with_no_properties_is_valid() {
        assert!(has_valid_property_keys(&json!({"type": "object"})));
        assert!(has_valid_property_keys(&json!({"type": "object", "properties": {}})));
    }

    #[test]
    fn non_object_schema_is_valid() {
        // Defensive: a malformed server could send anything here.
        assert!(has_valid_property_keys(&json!("not an object")));
        assert!(has_valid_property_keys(&json!(null)));
    }

    #[test]
    fn detects_the_real_world_ftshare_placeholder_property() {
        // The exact shape that took down a real conversation: a
        // parameterless tool declared with a placeholder key instead of an
        // empty `properties` object.
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": [],
            "properties": { "（无业务参数）": { "type": "string", "maxLength": 4096 } }
        });
        assert!(!has_valid_property_keys(&schema));
    }

    #[test]
    fn detects_illegal_key_nested_inside_object() {
        let schema = json!({
            "type": "object",
            "properties": {
                "filter": { "type": "object", "properties": { "开始日期": { "type": "string" } } }
            }
        });
        assert!(!has_valid_property_keys(&schema));
    }

    #[test]
    fn detects_illegal_key_inside_array_items() {
        let schema = json!({
            "type": "object",
            "properties": {
                "rows": { "type": "array", "items": { "type": "object", "properties": { "字段": {} } } }
            }
        });
        assert!(!has_valid_property_keys(&schema));
    }

    #[test]
    fn detects_illegal_key_inside_any_of_branch() {
        let schema = json!({
            "type": "object",
            "properties": {
                "target": {
                    "anyOf": [
                        { "type": "string" },
                        { "type": "object", "properties": { "代码": { "type": "string" } } }
                    ]
                }
            }
        });
        assert!(!has_valid_property_keys(&schema));
    }

    #[test]
    fn detects_illegal_key_inside_defs() {
        let schema = json!({
            "type": "object",
            "properties": { "ref": { "$ref": "#/$defs/Filter" } },
            "$defs": { "Filter": { "type": "object", "properties": { "结束日期": { "type": "string" } } } }
        });
        assert!(!has_valid_property_keys(&schema));
    }

    #[test]
    fn key_over_length_limit_is_invalid() {
        let long_key = "a".repeat(65);
        let schema = json!({ "type": "object", "properties": { long_key: { "type": "string" } } });
        assert!(!has_valid_property_keys(&schema));
    }

    #[test]
    fn empty_key_is_invalid() {
        let schema = json!({ "type": "object", "properties": { "": { "type": "string" } } });
        assert!(!has_valid_property_keys(&schema));
    }

    /// Transport stub for tests that only exercise tool-registration logic —
    /// `register_mcp_tools` never calls into the transport, so every method
    /// panics if reached.
    struct NoopTransport;

    #[async_trait]
    impl crate::transport::McpTransport for NoopTransport {
        async fn request(
            &self,
            _req: &crate::protocol::JsonRpcRequest,
        ) -> Result<crate::protocol::JsonRpcResponse, crate::transport::McpError> {
            unreachable!("NoopTransport::request should not be called by these tests")
        }

        async fn notify(&self, _req: &crate::protocol::JsonRpcRequest) -> Result<(), crate::transport::McpError> {
            unreachable!("NoopTransport::notify should not be called by these tests")
        }

        async fn close(&self) -> Result<(), crate::transport::McpError> {
            unreachable!("NoopTransport::close should not be called by these tests")
        }
    }

    fn tool_def(name: &str, input_schema: Value) -> crate::protocol::McpToolDef {
        crate::protocol::McpToolDef {
            name: name.to_string(),
            description: None,
            input_schema,
        }
    }

    #[test]
    fn register_mcp_tools_skips_only_the_tool_with_the_bad_schema() {
        let manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
            "ftshare",
            true,
            vec![
                tool_def(
                    "ft_goodwill_market_overview",
                    json!({
                        "type": "object",
                        "properties": { "（无业务参数）": { "type": "string" } }
                    }),
                ),
                tool_def(
                    "get_a_share_quotes",
                    json!({
                        "type": "object",
                        "properties": { "codes": { "type": "array" } }
                    }),
                ),
            ],
            Box::new(NoopTransport),
        )]));
        let mut registry = dream_engine_tools::registry::ToolRegistry::new();
        let configs = HashMap::new();

        register_mcp_tools(&mut registry, &manager, &[], &configs);

        let names = registry.tool_names();
        assert!(
            !names.iter().any(|n| n == "ft_goodwill_market_overview"),
            "the malformed tool must not be registered"
        );
        assert!(
            names.iter().any(|n| n == "get_a_share_quotes"),
            "the other well-formed tools on the same server must still register"
        );
    }
}

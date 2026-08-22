use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::config::McpServerConfig;
use super::manager::McpManager;
use dream_engine_protocol::events::ToolCategory;
use dream_engine_tools::Tool;
use dream_engine_types::tool::{JsonSchema, ToolResult};

/// Wraps an MCP server tool as a local Tool trait implementation.
/// Uses naming convention "mcp__{server}__{tool}" when collisions exist,
/// otherwise uses the tool's original name.
pub struct McpToolProxy {
    /// Display name used for registration (may be prefixed)
    display_name: String,
    /// Original tool name on the MCP server
    tool_name: String,
    /// Server this tool belongs to
    server_name: String,
    description: String,
    input_schema: JsonSchema,
    manager: Arc<McpManager>,
    /// Whether this tool's schema should be deferred (sent as name-only stub).
    deferred: bool,
}

impl McpToolProxy {
    pub fn new(
        display_name: String,
        tool_name: String,
        server_name: String,
        description: String,
        input_schema: JsonSchema,
        manager: Arc<McpManager>,
        deferred: bool,
    ) -> Self {
        Self {
            display_name,
            tool_name,
            server_name,
            description,
            input_schema,
            manager,
            deferred,
        }
    }
}

#[async_trait]
impl Tool for McpToolProxy {
    fn name(&self) -> &str {
        &self.display_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> JsonSchema {
        self.input_schema.clone()
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // MCP tools are assumed not concurrency-safe
        false
    }

    fn is_deferred(&self) -> bool {
        self.deferred
    }

    async fn execute(&self, input: Value) -> ToolResult {
        match self.manager.call_tool(&self.server_name, &self.tool_name, input).await {
            Ok(content) => ToolResult {
                content,
                is_error: false,
            },
            Err(e) => ToolResult {
                content: format!("MCP tool error: {}", e),
                is_error: true,
            },
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Mcp
    }

    fn describe(&self, input: &Value) -> String {
        format!(
            "MCP {}/{}: {}",
            self.server_name,
            self.tool_name,
            serde_json::to_string(input).unwrap_or_default()
        )
    }
}

/// Whether a tool's `inputSchema` uses only property keys the LLM provider
/// APIs accept (`^[a-zA-Z0-9_.-]{1,64}$`, matching Anthropic's Messages API
/// rule — the strictest of the providers this app talks to). A single
/// non-conforming tool would otherwise take down every tool call in the
/// session, so a bad tool must be dropped individually rather than losing
/// the whole server.
///
/// Real-world case that motivated this: a financial-data MCP server (ftshare)
/// declared a parameterless tool as `properties: { "（无业务参数）": {...} }`
/// — a human-readable placeholder where an empty object belongs. Walks the
/// composition keywords a real MCP server is likely to emit (`properties`,
/// `items`, `anyOf`/`oneOf`/`allOf`, `$defs`/`definitions`) since the
/// provider validates the fully-resolved schema, not just its top level.
fn has_valid_property_keys(schema: &Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return true;
    };

    if let Some(props) = obj.get("properties").and_then(Value::as_object) {
        for key in props.keys() {
            if key.is_empty()
                || key.len() > 64
                || !key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
            {
                return false;
            }
        }
        if props.values().any(|child| !has_valid_property_keys(child)) {
            return false;
        }
    }

    if let Some(items) = obj.get("items")
        && !has_valid_property_keys(items)
    {
        return false;
    }

    for keyword in ["anyOf", "oneOf", "allOf"] {
        if let Some(branches) = obj.get(keyword).and_then(Value::as_array)
            && branches.iter().any(|branch| !has_valid_property_keys(branch))
        {
            return false;
        }
    }

    for keyword in ["$defs", "definitions"] {
        if let Some(defs) = obj.get(keyword).and_then(Value::as_object)
            && defs.values().any(|def| !has_valid_property_keys(def))
        {
            return false;
        }
    }

    true
}

/// Register all MCP tools into the tool registry, handling name collisions.
///
/// Strategy:
/// - If tool name doesn't collide with built-in or other MCP tools → use as-is
/// - If collision detected → prefix with "mcp__{server_name}__"
///
/// Each tool's deferred flag is read from the server's config:
/// `McpServerConfig::deferred` — defaults to `true` when absent.
///
/// Tools whose schema the provider API would reject are skipped individually
/// (see [`has_valid_property_keys`]) so one malformed tool doesn't cost the
/// server's other tools.
pub fn register_mcp_tools(
    registry: &mut dream_engine_tools::registry::ToolRegistry,
    manager: &Arc<McpManager>,
    builtin_names: &[String],
    server_configs: &HashMap<String, McpServerConfig>,
) {
    let all_tools = manager.all_tools();

    // Determine which names need prefixing
    for (server_name, tool_def) in &all_tools {
        let original_name = &tool_def.name;

        if !has_valid_property_keys(&tool_def.input_schema) {
            tracing::warn!(
                target: "dream_engine_mcp",
                server = %server_name,
                tool = %original_name,
                "mcp tool schema has a property key the provider API would reject; skipping this tool only"
            );
            continue;
        }

        // Check collision with built-in tools
        let collides_builtin = builtin_names.iter().any(|n| n == original_name);

        // Check collision with other MCP servers' tools
        let cross_server_collision = manager.tool_name_count(original_name) > 1;

        let display_name = if collides_builtin || cross_server_collision {
            format!("mcp__{}_{}", server_name, original_name)
        } else {
            original_name.clone()
        };

        // MCP tools are deferred by default; server config can override.
        let deferred = server_configs
            .get(*server_name)
            .and_then(|c| c.deferred)
            .unwrap_or(true);

        let proxy = McpToolProxy::new(
            display_name,
            original_name.clone(),
            server_name.to_string(),
            tool_def.description.clone().unwrap_or_default(),
            tool_def.input_schema.clone(),
            Arc::clone(manager),
            deferred,
        );

        registry.register(Box::new(proxy));
    }
}

/// Register tools from a single newly-connected MCP server.
/// Uses the same collision-detection logic as `register_mcp_tools`.
pub fn register_single_server_tools(
    registry: &mut dream_engine_tools::registry::ToolRegistry,
    manager: &Arc<McpManager>,
    server_name: &str,
    builtin_names: &[String],
    deferred: bool,
) {
    let all_tools = manager.all_tools();
    let server_tools: Vec<_> = all_tools.iter().filter(|(sn, _)| *sn == server_name).collect();

    for (_, tool_def) in &server_tools {
        let original_name = &tool_def.name;

        if !has_valid_property_keys(&tool_def.input_schema) {
            tracing::warn!(
                target: "dream_engine_mcp",
                server = %server_name,
                tool = %original_name,
                "mcp tool schema has a property key the provider API would reject; skipping this tool only"
            );
            continue;
        }

        let collides_builtin = builtin_names.iter().any(|n| n == original_name);
        let cross_server_collision = manager.tool_name_count(original_name) > 1;

        let display_name = if collides_builtin || cross_server_collision {
            format!("mcp__{}_{}", server_name, original_name)
        } else {
            original_name.clone()
        };

        let proxy = McpToolProxy::new(
            display_name,
            original_name.clone(),
            server_name.to_string(),
            tool_def.description.clone().unwrap_or_default(),
            tool_def.input_schema.clone(),
            Arc::clone(manager),
            deferred,
        );

        registry.register(Box::new(proxy));
    }
}

#[cfg(test)]
#[path = "tool_proxy_test.rs"]
mod tool_proxy_test;

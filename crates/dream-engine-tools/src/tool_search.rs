use async_trait::async_trait;
use serde_json::{Value, json};

use dream_engine_protocol::events::ToolCategory;
use dream_engine_types::tool::{JsonSchema, ToolDef, ToolResult};

use crate::Tool;
use crate::registry::{LoadedSchemaSet, mark_schemas_loaded};

/// Built-in tool that searches for deferred tools and loads their full schema.
/// Core tool (never deferred itself) — always available to the LLM.
pub struct ToolSearchTool {
    /// Snapshot of all tool definitions (taken at construction time).
    tool_defs: Vec<ToolDef>,
    /// Shared promoted-schema set from the owning registry. Matched tools are
    /// inserted here so subsequent requests declare their full schema —
    /// schema-constrained providers cannot use a schema that only exists as
    /// tool-result text.
    loaded_schemas: Option<LoadedSchemaSet>,
}

impl ToolSearchTool {
    pub fn new(tool_defs: Vec<ToolDef>) -> Self {
        Self {
            tool_defs,
            loaded_schemas: None,
        }
    }

    /// Construct with the registry's promoted-schema handle so successful
    /// searches promote the matched deferred tools to full declaration.
    pub fn with_loaded_schemas(tool_defs: Vec<ToolDef>, loaded_schemas: LoadedSchemaSet) -> Self {
        Self {
            tool_defs,
            loaded_schemas: Some(loaded_schemas),
        }
    }

    /// Comma-separated names of all deferred tools in the snapshot, for the
    /// miss message — stops models from retrying free-text queries blindly.
    fn deferred_names(&self) -> String {
        self.tool_defs
            .iter()
            .filter(|d| d.deferred)
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Comma-separated names of all non-deferred tools — the ones already
    /// callable directly with full parameters. Some models (observed with
    /// GLM behind constrained-decoding gateways) over-apply the deferred-tool
    /// guidance and keep searching for core tools they already have; handing
    /// them the explicit inventory on every miss breaks that loop.
    fn directly_callable_names(&self) -> String {
        self.tool_defs
            .iter()
            .filter(|d| !d.deferred && d.name != "ToolSearch")
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "ToolSearch"
    }

    fn description(&self) -> &str {
        "Search for deferred tools and load their full schema. \
         Use this before calling any deferred tool."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Tool name or keyword to search for"
                }
            },
            "required": ["query"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let query = input["query"].as_str().unwrap_or("");
        if query.is_empty() {
            return ToolResult {
                content: "Error: query is required".to_string(),
                is_error: true,
            };
        }

        let query_lower = query.to_lowercase();
        let matches: Vec<Value> = self
            .tool_defs
            .iter()
            .filter(|d| d.deferred)
            .filter(|d| {
                d.name.to_lowercase().contains(&query_lower) || d.description.to_lowercase().contains(&query_lower)
            })
            .map(|d| {
                json!({
                    "name": d.name,
                    "description": d.description,
                    "parameters": d.input_schema
                })
            })
            .collect();

        if matches.is_empty() {
            let deferred = self.deferred_names();
            let deferred_line = if deferred.is_empty() {
                "There are NO deferred tools in this session — you never need ToolSearch.".to_string()
            } else {
                format!("The ONLY deferred tools (the only ones ToolSearch can load) are: {deferred}.")
            };
            let callable = self.directly_callable_names();
            let callable_line = if callable.is_empty() {
                String::new()
            } else {
                format!(
                    " These tools are ALREADY available right now — call them directly by name, do NOT search for them: {callable}."
                )
            };
            return ToolResult {
                content: format!(
                    "No deferred tools matching \"{query}\" found. {deferred_line}{callable_line} \
                     To run a skill (e.g. officecli, financial-model), call the Skill tool with the \
                     skill name as the `skill` argument — do NOT ToolSearch for skills. \
                     Stop searching and take the next real action now."
                ),
                is_error: false,
            };
        }

        // Promote matched tools: from the next request on they are declared
        // with their full schema, so schema-constrained providers can emit
        // real arguments instead of being forced to `{}` by the stub.
        if let Some(set) = &self.loaded_schemas {
            mark_schemas_loaded(set, matches.iter().filter_map(|m| m["name"].as_str()));
        }

        ToolResult {
            content: serde_json::to_string_pretty(&matches).unwrap_or_default(),
            is_error: false,
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }
}

#[cfg(test)]
#[path = "tool_search_test.rs"]
mod tool_search_test;

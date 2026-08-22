use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use dream_engine_types::tool::ToolDef;

use crate::Tool;

/// Shared set of deferred-tool names whose full schema has been surfaced to
/// the LLM (via ToolSearch or a failed call). Once a name is in this set the
/// registry declares that tool with its full schema instead of the stub —
/// required for schema-constrained providers (e.g. GLM behind LiteLLM-style
/// gateways) that can only generate arguments matching the declared schema.
pub type LoadedSchemaSet = Arc<Mutex<HashSet<String>>>;

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
    /// Deferred tools promoted to full-schema declaration for this session.
    loaded_schemas: LoadedSchemaSet,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            loaded_schemas: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Find a tool by name
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
    }

    /// Get all registered tool names
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    /// Shared handle to the promoted-schema set. Given to ToolSearchTool so a
    /// successful search promotes the matched tools for subsequent requests.
    pub fn loaded_schemas_handle(&self) -> LoadedSchemaSet {
        Arc::clone(&self.loaded_schemas)
    }

    /// Mark a deferred tool's schema as loaded: subsequent `to_tool_defs`
    /// calls declare it with the full schema instead of the deferred stub.
    pub fn mark_schema_loaded(&self, name: &str) {
        mark_schemas_loaded(&self.loaded_schemas, std::iter::once(name));
    }

    fn is_schema_loaded(&self, name: &str) -> bool {
        self.loaded_schemas.lock().map(|s| s.contains(name)).unwrap_or(false)
    }

    /// Generate API tool definitions for all registered tools
    pub fn to_tool_defs(&self) -> Vec<ToolDef> {
        self.tools.iter().map(|t| self.tool_def_for(t.as_ref())).collect()
    }

    /// Generate API tool definitions for tools matching a predicate.
    ///
    /// Used by plan mode to restrict the tool set sent to the LLM.
    pub fn to_tool_defs_filtered<F>(&self, filter: F) -> Vec<ToolDef>
    where
        F: Fn(&dyn Tool) -> bool,
    {
        self.tools
            .iter()
            .filter(|t| filter(t.as_ref()))
            .map(|t| self.tool_def_for(t.as_ref()))
            .collect()
    }

    fn tool_def_for(&self, t: &dyn Tool) -> ToolDef {
        ToolDef {
            name: t.name().to_string(),
            description: t.description().to_string(),
            input_schema: t.input_schema(),
            deferred: t.is_deferred() && !self.is_schema_loaded(t.name()),
        }
    }
}

/// Insert tool names into a promoted-schema set (shared helper for
/// ToolSearchTool, which holds only the handle, not the registry).
pub fn mark_schemas_loaded<'a>(set: &LoadedSchemaSet, names: impl IntoIterator<Item = &'a str>) {
    if let Ok(mut guard) = set.lock() {
        for name in names {
            guard.insert(name.to_string());
        }
    }
}

#[cfg(test)]
#[path = "registry_test.rs"]
mod registry_test;

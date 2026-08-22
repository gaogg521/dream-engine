use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn build_tool_defs() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "Read".into(),
                description: "Read a file".into(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                deferred: false,
            },
            ToolDef {
                name: "SpawnTool".into(),
                description: "Spawn sub-agents".into(),
                input_schema: json!({"type": "object", "properties": {"agents": {"type": "array"}}}),
                deferred: true,
            },
            ToolDef {
                name: "EnterPlanMode".into(),
                description: "Enter plan mode".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                deferred: true,
            },
        ]
    }

    #[tokio::test]
    async fn search_by_exact_name() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "SpawnTool"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("SpawnTool"));
        assert!(result.content.contains("Spawn sub-agents"));
        assert!(result.content.contains("parameters"));
    }

    #[tokio::test]
    async fn search_case_insensitive() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "spawntool"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("SpawnTool"));
    }

    #[tokio::test]
    async fn search_by_description_keyword() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "plan"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("EnterPlanMode"));
    }

    #[tokio::test]
    async fn search_excludes_non_deferred() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "Read"})).await;
        // "Read" is not deferred, should not appear in results
        assert!(!result.content.contains("\"name\": \"Read\"") || result.content.contains("No deferred tools"));
    }

    #[tokio::test]
    async fn search_no_match() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "nonexistent"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("No deferred tools"));
        // Miss message enumerates the actual deferred tools so the model
        // stops probing with free-text queries.
        assert!(result.content.contains("SpawnTool, EnterPlanMode"));
    }

    #[tokio::test]
    async fn search_miss_lists_directly_callable_tools() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "write file"})).await;
        assert!(!result.is_error);
        // Non-deferred tools must be named as already-available so models that
        // over-search (GLM) get a concrete inventory instead of looping.
        assert!(result.content.contains("ALREADY available"));
        assert!(result.content.contains("Read"));
        // ToolSearch must not list itself as a directly-callable action.
        assert!(!result.content.contains(": ToolSearch") && !result.content.contains(", ToolSearch"));
        // Skill guidance present.
        assert!(result.content.contains("Skill tool"));
    }

    #[tokio::test]
    async fn search_empty_query_returns_error() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": ""})).await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn search_hit_promotes_matched_tools() {
        use crate::registry::LoadedSchemaSet;
        use std::collections::HashSet;
        use std::sync::{Arc, Mutex};

        let set: LoadedSchemaSet = Arc::new(Mutex::new(HashSet::new()));
        let tool = ToolSearchTool::with_loaded_schemas(build_tool_defs(), Arc::clone(&set));
        let result = tool.execute(json!({"query": "SpawnTool"})).await;
        assert!(!result.is_error);
        let loaded = set.lock().unwrap();
        assert!(loaded.contains("SpawnTool"));
        assert!(!loaded.contains("EnterPlanMode"));
    }

    #[tokio::test]
    async fn search_miss_promotes_nothing() {
        use crate::registry::LoadedSchemaSet;
        use std::collections::HashSet;
        use std::sync::{Arc, Mutex};

        let set: LoadedSchemaSet = Arc::new(Mutex::new(HashSet::new()));
        let tool = ToolSearchTool::with_loaded_schemas(build_tool_defs(), Arc::clone(&set));
        let _ = tool.execute(json!({"query": "nonexistent"})).await;
        assert!(set.lock().unwrap().is_empty());
    }
}

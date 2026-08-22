use async_trait::async_trait;
use serde_json::{Value, json};

use dream_engine_protocol::events::ToolCategory;
use dream_engine_types::message::ContentBlock;
use dream_engine_types::tool::{JsonSchema, ToolResult};

use crate::image_source::{image_path_argument, load_image_url};
use crate::{Tool, ToolExecutionOutput};

pub struct ViewImageTool;

impl ViewImageTool {
    pub fn new() -> Self {
        Self
    }

    fn success_result(file_path: &str) -> ToolResult {
        ToolResult {
            content: format!("Image loaded from {file_path} and attached to the next model turn."),
            is_error: false,
        }
    }

    fn error_result(error: String) -> ToolResult {
        ToolResult {
            content: error,
            is_error: true,
        }
    }
}

impl Default for ViewImageTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ViewImageTool {
    fn name(&self) -> &str {
        "ViewImage"
    }

    fn description(&self) -> &str {
        "Loads an image from an absolute local file path and attaches it to the next model turn. Use this when you need to inspect an image attachment."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to a JPEG, PNG, GIF, or WebP image"
                }
            },
            "required": ["file_path"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        match image_path_argument(&input) {
            Ok(file_path) => match load_image_url(&file_path).await {
                Ok(_) => Self::success_result(&file_path),
                Err(error) => Self::error_result(error),
            },
            Err(error) => Self::error_result(error),
        }
    }

    async fn execute_with_follow_up(&self, input: Value) -> ToolExecutionOutput {
        let file_path = match image_path_argument(&input) {
            Ok(file_path) => file_path,
            Err(error) => return Self::error_result(error).into(),
        };
        match load_image_url(&file_path).await {
            Ok(image_url) => ToolExecutionOutput {
                result: Self::success_result(&file_path),
                follow_up_blocks: vec![ContentBlock::Image { image_url }],
            },
            Err(error) => Self::error_result(error).into(),
        }
    }

    fn requires_image_input(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let path = input.get("file_path").and_then(Value::as_str).unwrap_or("unknown");
        format!("View image {path}")
    }
}

#[cfg(test)]
#[path = "view_image_test.rs"]
mod view_image_test;

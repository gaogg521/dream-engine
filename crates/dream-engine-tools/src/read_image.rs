//! `ReadImage` — turn a local image into text without needing the main model to
//! be able to see images.
//!
//! `ViewImage` attaches the raw image to the *next main-model turn*, so it is
//! only advertised when the selected model accepts image input
//! (`requires_image_input() == true`). A text-only model therefore ends up with
//! an attachment path and no tool that can open it.
//!
//! This tool closes that gap from the other side: it sends the image to a
//! separately configured vision model and returns that model's **text** through
//! the ordinary tool-result channel. Nothing about it needs image support from
//! the main model, so it stays in the tool list for every model.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tracing::{debug, warn};

use dream_engine_protocol::events::ToolCategory;
use dream_engine_providers::LlmProvider;
use dream_engine_types::llm::{LlmEvent, LlmRequest};
use dream_engine_types::message::{ContentBlock, Message, Role};
use dream_engine_types::tool::{JsonSchema, ToolResult};
use dream_engine_types::usage::DelegateUsageSink;

use crate::Tool;
use crate::image_source::{image_path_argument, load_image_url};

/// Output cap for one description. Vision answers are prose, not file dumps.
const VISION_MAX_TOKENS: u32 = 4_000;

const VISION_SYSTEM_PROMPT: &str = "You are an image analysis service. You receive one image and a request, and you \
     answer with plain text only. Describe exactly what is present: layout, objects, people, colors, chart series and \
     axis labels, UI elements, and — verbatim — every piece of text you can read, preserving its original language. \
     Never speculate about content you cannot actually see; if part of the image is unreadable, say which part.";

const DEFAULT_INSTRUCTION: &str = "Describe this image in full detail and transcribe all text it contains.";

/// A vision-capable model that `ReadImage` delegates to.
///
/// The caller decides which model this is; the tool never classifies a model as
/// vision capable on its own.
pub struct VisionBackend {
    provider: Arc<dyn LlmProvider>,
    model: String,
    /// Provider label, used only to tell the user which model answered.
    label: String,
}

impl VisionBackend {
    pub fn new(provider: Arc<dyn LlmProvider>, model: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
            label: label.into(),
        }
    }
}

pub struct ReadImageTool {
    /// The model driving the conversation, named in the unavailable-path error
    /// so the user knows which model could not read the image.
    main_model: String,
    vision: Option<VisionBackend>,
    /// Where the delegate call's token usage is reported. `None` means nobody
    /// is metering (the CLI); the delegate still runs, its cost is just not
    /// accounted anywhere.
    usage_sink: Option<Arc<dyn DelegateUsageSink>>,
    /// Why no delegate is available, when the host knows a reason more specific
    /// than "none configured" — e.g. a company policy that excludes every
    /// vision-capable model the user has. Preferred over the generic advice,
    /// which in that case tells the user to do something they cannot do.
    unavailable_reason: Option<String>,
}

impl ReadImageTool {
    pub fn new(main_model: impl Into<String>, vision: Option<VisionBackend>) -> Self {
        Self {
            main_model: main_model.into(),
            vision,
            usage_sink: None,
            unavailable_reason: None,
        }
    }

    /// Report the delegate's token usage to `sink`. Without one, the call is
    /// invisible to any spend accounting the host does.
    pub fn with_usage_sink(mut self, sink: Arc<dyn DelegateUsageSink>) -> Self {
        self.usage_sink = Some(sink);
        self
    }

    /// Override the "no vision model" advice with a host-supplied reason.
    /// Ignored when it is empty.
    pub fn with_unavailable_reason(mut self, reason: Option<String>) -> Self {
        self.unavailable_reason = reason.filter(|r| !r.trim().is_empty());
        self
    }

    /// The message shown when no vision-capable model is reachable.
    ///
    /// It has to be actionable *and* explicitly forbid invention: a vague or
    /// empty tool result is exactly what makes an agent fabricate a description
    /// of an image it never saw.
    fn no_vision_model_error(&self, file_path: &str) -> ToolResult {
        // A host-supplied reason replaces the whole diagnosis-and-remedy middle
        // section, not just the remedy: the default diagnosis asserts that no
        // configured model is marked as supporting images, which is false when
        // one exists and was merely refused. The "do not invent" instruction is
        // the whole point of this message and stays on every path.
        const DEFAULT_CAUSE: &str = "No other configured model is marked as supporting images.\n\
                                     To fix this, open Settings -> Models, add or enable a model that accepts images \
                                     (for example gpt-4o, claude-sonnet-4, gemini-2.5-pro, qwen-vl-max or glm-4v), \
                                     then send the image again. If a vision model is already configured, open its \
                                     model settings and set image input to \"supported\".";
        let cause = self.unavailable_reason.as_deref().unwrap_or(DEFAULT_CAUSE);
        ToolResult {
            content: format!(
                "Cannot read '{file_path}': no vision-capable model is available in this session.\n\
                 The active model '{}' does not accept image input.\n\
                 {cause}\n\
                 Until then the contents of this image are unknown. Tell the user that image reading is unavailable \
                 and why. Do NOT guess, infer from the file name, or invent what the image shows.",
                self.main_model
            ),
            is_error: true,
        }
    }

    fn error_result(error: String) -> ToolResult {
        ToolResult {
            content: error,
            is_error: true,
        }
    }

    async fn describe(&self, vision: &VisionBackend, image: ContentBlock, instruction: &str) -> Result<String, String> {
        let request = LlmRequest {
            model: vision.model.clone(),
            system: VISION_SYSTEM_PROMPT.to_owned(),
            messages: vec![Message::now(
                Role::User,
                vec![
                    image,
                    ContentBlock::Text {
                        text: instruction.to_owned(),
                    },
                ],
            )],
            tools: Vec::new(),
            max_tokens: Some(VISION_MAX_TOKENS),
            thinking: None,
            reasoning_effort: None,
        };

        let mut stream = vision
            .provider
            .stream(&request)
            .await
            .map_err(|error| format!("Vision model '{}' could not be reached: {error}", vision.model))?;

        let mut description = String::new();
        while let Some(event) = stream.recv().await {
            match event {
                LlmEvent::TextDelta(delta) => description.push_str(&delta),
                LlmEvent::Error(error) => {
                    return Err(format!("Vision model '{}' returned an error: {error}", vision.model));
                }
                // The delegate's cost is real and belongs to whoever meters
                // this session. Dropping it here — which is what this arm used
                // to do — makes a second, billable model call invisible to
                // every spend cap and usage dashboard.
                LlmEvent::Done { usage, .. } => {
                    if let Some(sink) = self.usage_sink.as_ref() {
                        sink.on_delegate_usage(&vision.model, &usage);
                    }
                    break;
                }
                _ => {}
            }
        }

        let description = description.trim().to_owned();
        if description.is_empty() {
            // Returning an empty-but-successful result would leave the agent
            // with nothing and invite it to make the contents up.
            return Err(format!(
                "Vision model '{}' returned an empty description. The image was not read; do not guess its contents.",
                vision.model
            ));
        }
        Ok(description)
    }
}

#[async_trait]
impl Tool for ReadImageTool {
    fn name(&self) -> &str {
        "ReadImage"
    }

    fn description(&self) -> &str {
        "Reads a local image file and returns a text description of it, including a transcription of any text in the \
         image. Use this for every image path you are given — including paths listed under '[Attached files]' and \
         paths the user pastes — because you cannot open image files yourself and the Read tool only reports them as \
         binary. It works regardless of whether the current model supports image input, because the image is analyzed \
         by a separate vision model and only text comes back. Pass `prompt` to focus the analysis, e.g. 'transcribe \
         all the text', 'read the numbers in this chart', 'describe the UI layout'."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to a JPEG, PNG, GIF, or WebP image"
                },
                "prompt": {
                    "type": "string",
                    "description": "Optional question or focus for the analysis. Defaults to a full description plus text transcription."
                }
            },
            "required": ["file_path"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let file_path = match image_path_argument(&input) {
            Ok(file_path) => file_path,
            Err(error) => return Self::error_result(error),
        };

        // Validate the file before reporting a missing vision model, so a typo
        // in the path is not mistaken for a configuration problem.
        let image_url = match load_image_url(&file_path).await {
            Ok(image_url) => image_url,
            Err(error) => return Self::error_result(error),
        };

        let Some(vision) = self.vision.as_ref() else {
            warn!(
                target: "dream_engine_tools",
                main_model = %self.main_model,
                "ReadImage invoked with no vision-capable model configured",
            );
            return self.no_vision_model_error(&file_path);
        };

        let instruction = input
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|prompt| !prompt.is_empty())
            .unwrap_or(DEFAULT_INSTRUCTION);

        debug!(
            target: "dream_engine_tools",
            vision_model = %vision.model,
            vision_provider = %vision.label,
            "ReadImage delegating to vision model",
        );

        match self
            .describe(vision, ContentBlock::Image { image_url }, instruction)
            .await
        {
            Ok(description) => ToolResult {
                content: format!(
                    "Image at {file_path}, as read by vision model '{}' ({}):\n\n{description}",
                    vision.model, vision.label
                ),
                is_error: false,
            },
            Err(error) => Self::error_result(error),
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let path = input.get("file_path").and_then(Value::as_str).unwrap_or("unknown");
        format!("Read image {path}")
    }
}

#[cfg(test)]
#[path = "read_image_test.rs"]
mod read_image_test;

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::mpsc;

use dream_engine_providers::{LlmProvider, ProviderError};
use dream_engine_types::llm::{LlmEvent, LlmRequest};
use dream_engine_types::message::{ContentBlock, StopReason, TokenUsage};
use dream_engine_types::usage::DelegateUsageSink;

use super::{ReadImageTool, VisionBackend};
use crate::Tool;

/// A stand-in vision model: records the request it was given and replays a
/// scripted event sequence.
#[derive(Default)]
struct ScriptedVisionProvider {
    events: Vec<LlmEvent>,
    requests: Arc<Mutex<Vec<LlmRequest>>>,
}

impl ScriptedVisionProvider {
    fn replying(text: &str) -> Self {
        Self {
            events: vec![
                LlmEvent::TextDelta(text.to_owned()),
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    usage: TokenUsage::default(),
                },
            ],
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_events(events: Vec<LlmEvent>) -> Self {
        Self {
            events,
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl LlmProvider for ScriptedVisionProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.requests.lock().unwrap().push(request.clone());
        let (tx, rx) = mpsc::channel(16);
        for event in &self.events {
            let _ = tx.send(event.clone()).await;
        }
        Ok(rx)
    }
}

struct UnreachableVisionProvider;

#[async_trait]
impl LlmProvider for UnreachableVisionProvider {
    async fn stream(&self, _request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        Err(ProviderError::Connection("connection refused".to_owned()))
    }
}

fn write_png(directory: &TempDir) -> PathBuf {
    let path = directory.path().join("chart.png");
    let png = STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScL3WQAAAABJRU5ErkJggg==")
        .expect("decode PNG fixture");
    fs::write(&path, png).expect("write image fixture");
    path
}

#[test]
fn is_advertised_to_models_without_image_input() {
    // The whole point of this tool: unlike ViewImage it must survive the
    // engine's `requires_image_input` capability filter.
    let tool = ReadImageTool::new("deepseek-v4-flash", None);

    assert!(!tool.requires_image_input());
    assert!(
        !tool.is_deferred(),
        "must be eagerly advertised, not hidden behind ToolSearch"
    );
}

#[tokio::test]
async fn returns_the_vision_models_description_as_text() {
    let directory = TempDir::new().expect("temp dir");
    let path = write_png(&directory);
    let provider = Arc::new(ScriptedVisionProvider::replying(
        "AVGO daily candles with MACD and KDJ panes.",
    ));
    let requests = provider.requests.clone();
    let tool = ReadImageTool::new(
        "deepseek-v4-flash",
        Some(VisionBackend::new(provider, "gpt-4o", "openai")),
    );

    let result = tool.execute(json!({ "file_path": path })).await;

    assert!(!result.is_error, "{}", result.content);
    assert!(result.content.contains("AVGO daily candles with MACD and KDJ panes."));
    assert!(result.content.contains("gpt-4o"));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].model, "gpt-4o");
    assert!(requests[0].tools.is_empty(), "the vision turn must not carry tools");
    let blocks = &requests[0].messages[0].content;
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block, ContentBlock::Image { image_url } if image_url.url.starts_with("data:image/png;base64,"))),
        "the image itself must reach the vision model, not just its path"
    );
}

#[tokio::test]
async fn forwards_a_caller_supplied_focus_prompt() {
    let directory = TempDir::new().expect("temp dir");
    let path = write_png(&directory);
    let provider = Arc::new(ScriptedVisionProvider::replying("12.34"));
    let requests = provider.requests.clone();
    let tool = ReadImageTool::new(
        "deepseek-v4-flash",
        Some(VisionBackend::new(provider, "gpt-4o", "openai")),
    );

    let result = tool
        .execute(json!({ "file_path": path, "prompt": "read the closing price" }))
        .await;

    assert!(!result.is_error);
    let requests = requests.lock().unwrap();
    assert!(
        requests[0].messages[0]
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::Text { text } if text == "read the closing price"))
    );
}

#[tokio::test]
async fn reports_an_actionable_error_when_no_vision_model_is_configured() {
    let directory = TempDir::new().expect("temp dir");
    let path = write_png(&directory);
    let tool = ReadImageTool::new("deepseek-v4-flash", None);

    let result = tool.execute(json!({ "file_path": path })).await;

    assert!(
        result.is_error,
        "silence here is what makes agents invent image contents"
    );
    assert!(!result.content.trim().is_empty());
    // Names the model that failed, where to fix it, and forbids fabrication.
    assert!(result.content.contains("deepseek-v4-flash"));
    assert!(result.content.contains("Settings -> Models"));
    assert!(result.content.contains("Do NOT guess"));
}

#[tokio::test]
async fn an_empty_vision_response_is_an_error_not_an_empty_success() {
    let directory = TempDir::new().expect("temp dir");
    let path = write_png(&directory);
    let provider = Arc::new(ScriptedVisionProvider::with_events(vec![
        LlmEvent::TextDelta("   ".to_owned()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage::default(),
        },
    ]));
    let tool = ReadImageTool::new(
        "deepseek-v4-flash",
        Some(VisionBackend::new(provider, "gpt-4o", "openai")),
    );

    let result = tool.execute(json!({ "file_path": path })).await;

    assert!(result.is_error);
    assert!(result.content.contains("empty description"));
    assert!(result.content.contains("do not guess"));
}

#[tokio::test]
async fn surfaces_a_vision_stream_error() {
    let directory = TempDir::new().expect("temp dir");
    let path = write_png(&directory);
    let provider = Arc::new(ScriptedVisionProvider::with_events(vec![LlmEvent::Error(
        "rate limited".to_owned(),
    )]));
    let tool = ReadImageTool::new(
        "deepseek-v4-flash",
        Some(VisionBackend::new(provider, "gpt-4o", "openai")),
    );

    let result = tool.execute(json!({ "file_path": path })).await;

    assert!(result.is_error);
    assert!(result.content.contains("rate limited"));
}

#[tokio::test]
async fn surfaces_an_unreachable_vision_provider() {
    let directory = TempDir::new().expect("temp dir");
    let path = write_png(&directory);
    let tool = ReadImageTool::new(
        "deepseek-v4-flash",
        Some(VisionBackend::new(
            Arc::new(UnreachableVisionProvider),
            "gpt-4o",
            "openai",
        )),
    );

    let result = tool.execute(json!({ "file_path": path })).await;

    assert!(result.is_error);
    assert!(result.content.contains("could not be reached"));
}

#[tokio::test]
async fn a_bad_path_is_reported_as_a_path_problem_not_a_missing_vision_model() {
    let tool = ReadImageTool::new("deepseek-v4-flash", None);

    let result = tool.execute(json!({ "file_path": "chart.png" })).await;

    assert!(result.is_error);
    assert!(result.content.contains("absolute path"));
    assert!(!result.content.contains("Settings -> Models"));
}

#[cfg(windows)]
#[tokio::test]
async fn accepts_the_verbatim_paths_the_host_injects() {
    let directory = TempDir::new().expect("temp dir");
    let path = write_png(&directory);
    let provider = Arc::new(ScriptedVisionProvider::replying("a chart"));
    let tool = ReadImageTool::new(
        "deepseek-v4-flash",
        Some(VisionBackend::new(provider, "gpt-4o", "openai")),
    );

    let result = tool
        .execute(json!({ "file_path": format!(r"\\?\{}", path.display()) }))
        .await;

    assert!(!result.is_error, "{}", result.content);
    assert!(result.content.contains("a chart"));
}

/// Records what a host meter would have been told.
#[derive(Default)]
struct RecordingUsageSink(Mutex<Vec<(String, TokenUsage)>>);

impl DelegateUsageSink for RecordingUsageSink {
    fn on_delegate_usage(&self, model: &str, usage: &TokenUsage) {
        self.0.lock().unwrap().push((model.to_owned(), usage.clone()));
    }
}

/// The delegate is a second billable model call. Before this, its `Done`
/// event was matched as `LlmEvent::Done { .. } => break` and the usage was
/// dropped on the floor — invisible to every spend cap and usage dashboard.
#[tokio::test]
async fn reports_the_delegate_calls_token_usage_to_the_host() {
    let directory = TempDir::new().expect("temp dir");
    let path = write_png(&directory);
    let provider = Arc::new(ScriptedVisionProvider::with_events(vec![
        LlmEvent::TextDelta("a bar chart".to_owned()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage {
                input_tokens: 1_234,
                output_tokens: 56,
                cache_creation_tokens: 0,
                cache_read_tokens: 7,
            },
        },
    ]));
    let sink = Arc::new(RecordingUsageSink::default());
    let tool = ReadImageTool::new(
        "deepseek-v4-flash",
        Some(VisionBackend::new(provider, "kimi-k2-6", "moonshot")),
    )
    .with_usage_sink(sink.clone());

    let result = tool.execute(json!({ "file_path": path })).await;

    assert!(!result.is_error, "{}", result.content);
    let recorded = sink.0.lock().unwrap();
    assert_eq!(recorded.len(), 1, "exactly one delegate call was made");
    assert_eq!(
        recorded[0].0, "kimi-k2-6",
        "usage must be attributed to the model that was actually billed, not the session model"
    );
    assert_eq!(recorded[0].1.input_tokens, 1_234);
    assert_eq!(recorded[0].1.output_tokens, 56);
    assert_eq!(recorded[0].1.cache_read_tokens, 7);
}

/// A failed delegate call cost nothing to report — and must not invent a zero
/// row that would read as "this call was free".
#[tokio::test]
async fn reports_no_usage_when_the_delegate_could_not_be_reached() {
    let directory = TempDir::new().expect("temp dir");
    let path = write_png(&directory);
    let sink = Arc::new(RecordingUsageSink::default());
    let tool = ReadImageTool::new(
        "deepseek-v4-flash",
        Some(VisionBackend::new(
            Arc::new(UnreachableVisionProvider),
            "kimi-k2-6",
            "moonshot",
        )),
    )
    .with_usage_sink(sink.clone());

    let result = tool.execute(json!({ "file_path": path })).await;

    assert!(result.is_error);
    assert!(sink.0.lock().unwrap().is_empty());
}

/// When the host knows the delegate was refused by company policy, the generic
/// "add a vision model in Settings" advice is wrong — the member cannot act on
/// it. The refusal must be stated instead, and the anti-fabrication
/// instruction must survive either way.
#[tokio::test]
async fn prefers_a_host_supplied_reason_over_the_generic_advice() {
    let directory = TempDir::new().expect("temp dir");
    let path = write_png(&directory);
    let tool = ReadImageTool::new("deepseek-v4-flash", None).with_unavailable_reason(Some(
        "Your organization's model policy does not allow the vision-capable model(s) configured here (gpt-4o). Ask \
         an administrator to add one to the allowed models list."
            .to_owned(),
    ));

    let result = tool.execute(json!({ "file_path": path })).await;

    assert!(result.is_error);
    assert!(result.content.contains("administrator"));
    assert!(
        !result.content.contains("Settings -> Models"),
        "the generic remedy must be replaced, not appended: {}",
        result.content
    );
    assert!(
        !result
            .content
            .contains("no other configured model is marked as supporting images")
            && !result
                .content
                .contains("No other configured model is marked as supporting images"),
        "that diagnosis is false here — a vision model exists, it was refused: {}",
        result.content
    );
    assert!(
        result.content.contains("Do NOT guess"),
        "the anti-fabrication instruction is not optional"
    );
}

/// An empty/whitespace reason is not a reason; fall back to the advice.
#[tokio::test]
async fn ignores_a_blank_host_supplied_reason() {
    let directory = TempDir::new().expect("temp dir");
    let path = write_png(&directory);
    let tool = ReadImageTool::new("deepseek-v4-flash", None).with_unavailable_reason(Some("   ".to_owned()));

    let result = tool.execute(json!({ "file_path": path })).await;

    assert!(result.content.contains("Settings -> Models"));
}

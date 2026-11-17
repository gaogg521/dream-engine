//! Black-box integration tests for the autocompact subsystem.
//!
//! These tests correspond to TC-2.4-* in the test plan.
//! They exercise the public autocompact API with a mock LLM provider,
//! validating trigger logic, summary formatting, boundary markers,
//! circuit breaker, and PTL retry behaviour.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::mpsc;

use dream_engine_agent::compact::auto::{
    CompactError, CompactPrefix, autocompact, autocompact_with_prefix, extract_compact_metadata, is_compact_boundary,
    should_autocompact,
};
use dream_engine_agent::compact::prompt::{
    COMPACT_SYSTEM_PROMPT, build_compact_prompt, build_summary_content, format_compact_summary,
};
use dream_engine_agent::compact::state::CompactState;
use dream_engine_config::compact::CompactConfig;
use dream_engine_providers::{LlmProvider, ProviderError};
use dream_engine_types::compact::CompactTrigger;
use dream_engine_types::llm::{LlmEvent, LlmRequest};
use dream_engine_types::message::{ContentBlock, Message, Role, StopReason, TokenUsage};
use dream_engine_types::tool::ToolDef;

// ── Mock provider ───────────────────────────────────────────────────────────

/// A mock LLM provider that returns pre-configured responses in order.
struct MockProvider {
    responses: Mutex<VecDeque<Result<Vec<LlmEvent>, ProviderError>>>,
}

impl MockProvider {
    fn new(responses: Vec<Result<Vec<LlmEvent>, ProviderError>>) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from(responses)),
        }
    }

    /// Create a provider that returns a successful summary response.
    fn with_summary(summary: &str) -> Self {
        Self::new(vec![Ok(vec![
            LlmEvent::TextDelta(summary.to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage {
                    input_tokens: 50_000,
                    output_tokens: 2_000,
                    ..Default::default()
                },
            },
        ])])
    }

    /// Create a provider that returns an error.
    fn with_error(error: ProviderError) -> Self {
        Self::new(vec![Err(error)])
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    async fn stream(&self, _request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let response = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("MockProvider: no more responses queued");

        match response {
            Ok(events) => {
                let (tx, rx) = mpsc::channel(events.len() + 1);
                for event in events {
                    tx.send(event).await.ok();
                }
                Ok(rx)
            }
            Err(e) => Err(e),
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn text_msg(role: Role, content: &str) -> Message {
    Message::new(
        role,
        vec![ContentBlock::Text {
            text: content.to_string(),
        }],
    )
}

fn sample_conversation(n: usize) -> Vec<Message> {
    (0..n)
        .map(|i| {
            let role = if i % 2 == 0 { Role::User } else { Role::Assistant };
            text_msg(role, &format!("message-{i}"))
        })
        .collect()
}

fn default_config() -> CompactConfig {
    CompactConfig::default()
}

// ── TC-2.4-01: Watermark above threshold triggers ───────────────────────────

#[test]
fn tc_2_4_01_above_threshold_triggers() {
    // Default is percentage mode: threshold = 500k * 80% = 400k.
    assert!(should_autocompact(410_000, &default_config()));
}

// ── TC-2.4-02: Below threshold does not trigger ─────────────────────────────

#[test]
fn tc_2_4_02_below_threshold_does_not_trigger() {
    assert!(!should_autocompact(390_000, &default_config()));
}

// ── TC-2.4-03: Exact threshold triggers ─────────────────────────────────────

#[test]
fn tc_2_4_03_at_exact_threshold_triggers() {
    assert!(should_autocompact(800_000, &default_config()));
}

// ── TC-2.4-04: Circuit breaker initial state ────────────────────────────────

#[test]
fn tc_2_4_04_initial_state_not_broken() {
    let state = CompactState::new();
    assert_eq!(state.consecutive_failures, 0);
    assert!(!state.is_circuit_broken(&default_config()));
}

// ── TC-2.4-05: Circuit breaker trips ────────────────────────────────────────

#[test]
fn tc_2_4_05_circuit_breaker_trips() {
    let config = default_config();
    let mut state = CompactState::new();
    state.record_failure();
    state.record_failure();
    state.record_failure();
    assert!(state.is_circuit_broken(&config));
}

// ── TC-2.4-06: Circuit breaker resets ───────────────────────────────────────

#[test]
fn tc_2_4_06_circuit_breaker_resets_on_success() {
    let config = default_config();
    let mut state = CompactState::new();
    state.record_failure();
    state.record_failure();
    state.record_success();
    assert_eq!(state.consecutive_failures, 0);
    assert!(!state.is_circuit_broken(&config));
}

// ── TC-2.4-07: Circuit breaker blocks autocompact ───────────────────────────

#[tokio::test]
async fn tc_2_4_07_circuit_breaker_blocks_autocompact() {
    let provider = MockProvider::with_summary("<summary>should not be called</summary>");
    let messages = sample_conversation(10);
    let config = default_config();
    let mut state = CompactState::new();
    state.record_failure();
    state.record_failure();
    state.record_failure();

    let result = autocompact(&provider, &messages, "test-model", &config, &mut state).await;
    assert!(matches!(result, Err(CompactError::CircuitBroken { .. })));
}

// ── TC-2.4-08: Prompt contains all 9 sections ──────────────────────────────

#[test]
fn tc_2_4_08_prompt_contains_all_sections() {
    let prompt = build_compact_prompt();
    for i in 1..=9 {
        assert!(prompt.contains(&format!("{i}.")), "Missing section {i}");
    }
    assert!(prompt.contains("CRITICAL: Respond with TEXT ONLY"));
}

// ── TC-2.4-09: Summary formatting (normal) ──────────────────────────────────

#[test]
fn tc_2_4_09_format_strips_analysis_extracts_summary() {
    let raw = "<analysis>thinking</analysis>\n<summary>result</summary>";
    assert_eq!(format_compact_summary(raw), "Summary:\nresult");
}

// ── TC-2.4-10: Summary formatting (no analysis) ────────────────────────────

#[test]
fn tc_2_4_10_format_without_analysis() {
    let raw = "<summary>result</summary>";
    assert_eq!(format_compact_summary(raw), "Summary:\nresult");
}

// ── TC-2.4-11: Summary formatting (no tags) ────────────────────────────────

#[test]
fn tc_2_4_11_format_graceful_degradation() {
    let raw = "plain text without tags";
    assert_eq!(format_compact_summary(raw), "plain text without tags");
}

// ── TC-2.4-12: Post-compact message structure ───────────────────────────────

#[tokio::test]
async fn tc_2_4_12_post_compact_message_structure() {
    let summary = "<analysis>thinking</analysis>\n<summary>Detailed summary here</summary>";
    let provider = MockProvider::with_summary(summary);
    let messages = sample_conversation(20);
    let config = default_config();
    let mut state = CompactState::new();
    state.last_input_tokens = 170_000;

    let result = autocompact(&provider, &messages, "test-model", &config, &mut state)
        .await
        .expect("autocompact should succeed");

    // Should have 2 messages: boundary + summary
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages_summarized, 20);

    // First message is the boundary marker
    assert!(is_compact_boundary(&result.messages[0]));
    assert_eq!(result.messages[0].role, Role::User);

    // Second message is the summary
    assert_eq!(result.messages[1].role, Role::User);
    match &result.messages[1].content[0] {
        ContentBlock::Text { text } => {
            assert!(text.contains("Detailed summary here"));
            assert!(text.contains("This session is being continued"));
        }
        _ => panic!("expected Text block"),
    }
}

// ── TC-2.4-13: Boundary marker metadata ─────────────────────────────────────

#[tokio::test]
async fn tc_2_4_13_boundary_metadata() {
    let provider = MockProvider::with_summary("<summary>summary</summary>");
    let messages = sample_conversation(15);
    let config = default_config();
    let mut state = CompactState::new();
    state.last_input_tokens = 170_000;

    let result = autocompact(&provider, &messages, "test-model", &config, &mut state)
        .await
        .expect("autocompact should succeed");

    let metadata = extract_compact_metadata(&result.messages[0]).expect("should have metadata");
    assert_eq!(metadata.trigger, CompactTrigger::Auto);
    assert_eq!(metadata.pre_compact_tokens, 170_000);
    assert_eq!(metadata.messages_summarized, 15);
}

// ── TC-2.4-14: Disabled config skips (tested via should_autocompact) ────────

#[test]
fn tc_2_4_14_disabled_config_skips() {
    let config = CompactConfig {
        enabled: false,
        ..default_config()
    };
    assert!(!should_autocompact(999_999, &config));
}

// ── TC-2.4-15: Prompt forbids tool calls ────────────────────────────────────

#[test]
fn tc_2_4_15_prompt_forbids_tool_calls() {
    let prompt = build_compact_prompt();
    assert!(prompt.contains("Do NOT call any tools"));
}

// ── TC-2.4-16: Success resets failure counter ───────────────────────────────

#[tokio::test]
async fn tc_2_4_16_success_resets_failure_counter() {
    let provider = MockProvider::with_summary("<summary>summary</summary>");
    let messages = sample_conversation(10);
    let config = default_config();
    let mut state = CompactState::new();
    state.consecutive_failures = 2;
    state.last_input_tokens = 170_000;

    autocompact(&provider, &messages, "test-model", &config, &mut state)
        .await
        .expect("autocompact should succeed");

    assert_eq!(state.consecutive_failures, 0);
}

// ── TC-2.4-17: Failure increments failure counter ───────────────────────────

#[tokio::test]
async fn tc_2_4_17_failure_increments_counter() {
    let provider = MockProvider::with_error(ProviderError::Api {
        status: 500,
        message: "Internal error".to_string(),
    });
    let messages = sample_conversation(10);
    let config = default_config();
    let mut state = CompactState::new();

    let result = autocompact(&provider, &messages, "test-model", &config, &mut state).await;
    assert!(result.is_err());
    assert_eq!(state.consecutive_failures, 1);
}

// ── TC-2.4-18: PTL retry succeeds on second attempt ────────────────────────

#[tokio::test]
async fn tc_2_4_18_ptl_retry_succeeds() {
    let provider = MockProvider::new(vec![
        // First attempt: prompt too long
        Err(ProviderError::PromptTooLong("prompt exceeds limit".to_string())),
        // Second attempt (after truncation): success
        Ok(vec![
            LlmEvent::TextDelta("<summary>retried summary</summary>".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage::default(),
            },
        ]),
    ]);

    let messages = sample_conversation(20);
    let config = default_config();
    let mut state = CompactState::new();
    state.last_input_tokens = 170_000;

    let result = autocompact(&provider, &messages, "test-model", &config, &mut state)
        .await
        .expect("autocompact should succeed after retry");

    assert_eq!(result.messages.len(), 2);
    assert_eq!(state.consecutive_failures, 0);

    // Verify summary content
    match &result.messages[1].content[0] {
        ContentBlock::Text { text } => {
            assert!(text.contains("retried summary"));
        }
        _ => panic!("expected Text block"),
    }
}

// ── TC-2.4-19: PTL retry exhausted ─────────────────────────────────────────

#[tokio::test]
async fn tc_2_4_19_ptl_retry_exhausted() {
    let provider = MockProvider::new(vec![
        Err(ProviderError::PromptTooLong("too long 1".to_string())),
        Err(ProviderError::PromptTooLong("too long 2".to_string())),
        Err(ProviderError::PromptTooLong("too long 3".to_string())),
    ]);

    let messages = sample_conversation(20);
    let config = default_config();
    let mut state = CompactState::new();

    let result = autocompact(&provider, &messages, "test-model", &config, &mut state).await;
    assert!(matches!(result, Err(CompactError::PromptTooLong { .. })));
    assert_eq!(state.consecutive_failures, 1);
}

// ── TC-2.4-20: PTL retry truncates messages ─────────────────────────────────

#[tokio::test]
async fn tc_2_4_20_ptl_retry_truncates_messages() {
    // Track the request message count on each attempt
    let request_counts: std::sync::Arc<Mutex<Vec<usize>>> = std::sync::Arc::new(Mutex::new(Vec::new()));
    let counts_clone = request_counts.clone();

    // Custom mock that records message counts
    struct CountingProvider {
        counts: std::sync::Arc<Mutex<Vec<usize>>>,
        attempt: Mutex<u32>,
    }

    #[async_trait]
    impl LlmProvider for CountingProvider {
        async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            // Scope the lock so the MutexGuard is dropped before the await
            let current_attempt = {
                let mut attempt = self.attempt.lock().unwrap();
                self.counts.lock().unwrap().push(request.messages.len());
                let val = *attempt;
                *attempt += 1;
                val
            };

            if current_attempt == 0 {
                return Err(ProviderError::PromptTooLong("too long".to_string()));
            }

            // Second attempt: succeed
            let (tx, rx) = mpsc::channel(2);
            tx.send(LlmEvent::TextDelta("<summary>truncated summary</summary>".to_string()))
                .await
                .ok();
            tx.send(LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage::default(),
            })
            .await
            .ok();
            Ok(rx)
        }
    }

    let provider = CountingProvider {
        counts: counts_clone,
        attempt: Mutex::new(0),
    };

    let messages = sample_conversation(20);
    let config = default_config();
    let mut state = CompactState::new();
    state.last_input_tokens = 170_000;

    autocompact(&provider, &messages, "test-model", &config, &mut state)
        .await
        .expect("should succeed after retry");

    let counts = request_counts.lock().unwrap();
    assert_eq!(counts.len(), 2, "should have 2 attempts");

    // First attempt: 20 conversation + 1 prompt = 21
    assert_eq!(counts[0], 21);

    // Second attempt: truncated (~20% dropped from 20 = 4 dropped) + 1 prompt
    // 20 - 4 = 16, + 1 prompt = 17
    assert_eq!(counts[1], 17);
}

// ── Additional edge cases ───────────────────────────────────────────────────

#[tokio::test]
async fn empty_response_fails() {
    // Provider returns Done without any TextDelta
    let provider = MockProvider::new(vec![Ok(vec![LlmEvent::Done {
        stop_reason: StopReason::EndTurn,
        usage: TokenUsage::default(),
    }])]);

    let messages = sample_conversation(10);
    let config = default_config();
    let mut state = CompactState::new();

    let result = autocompact(&provider, &messages, "test-model", &config, &mut state).await;
    assert!(matches!(result, Err(CompactError::EmptyResponse)));
    assert_eq!(state.consecutive_failures, 1);
}

#[tokio::test]
async fn stream_error_fails() {
    let provider = MockProvider::new(vec![Ok(vec![
        LlmEvent::TextDelta("partial".to_string()),
        LlmEvent::Error("connection reset".to_string()),
    ])]);

    let messages = sample_conversation(10);
    let config = default_config();
    let mut state = CompactState::new();

    let result = autocompact(&provider, &messages, "test-model", &config, &mut state).await;
    assert!(matches!(result, Err(CompactError::StreamError(_))));
    assert_eq!(state.consecutive_failures, 1);
}

#[test]
fn summary_content_auto_has_continuation() {
    let content = build_summary_content("Summary:\ntest", true);
    assert!(content.contains("Continue the conversation"));
    assert!(content.contains("as if the break never happened"));
}

#[test]
fn summary_content_manual_no_continuation() {
    let content = build_summary_content("Summary:\ntest", false);
    assert!(!content.contains("Continue the conversation"));
}

// ── Shared-prefix compaction ────────────────────────────────────────────────
//
// The summary request is the largest input a session sends. These tests pin
// the property that keeps it cheap — it is an exact extension of the live
// conversation — and the two fallbacks that exist only because of it.

/// Records every request it receives, then answers from a queue.
struct RecordingProvider {
    inner: MockProvider,
    requests: Mutex<Vec<LlmRequest>>,
}

impl RecordingProvider {
    fn new(responses: Vec<Result<Vec<LlmEvent>, ProviderError>>) -> Self {
        Self {
            inner: MockProvider::new(responses),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<LlmRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.requests.lock().unwrap().push(request.clone());
        self.inner.stream(request).await
    }
}

fn live_prefix() -> CompactPrefix {
    CompactPrefix {
        system: "LIVE-SYSTEM-PROMPT".to_string(),
        tools: vec![ToolDef {
            name: "Read".to_string(),
            description: "Read a file".to_string(),
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
            deferred: false,
        }],
    }
}

fn summary_response(text: &str, usage: TokenUsage) -> Result<Vec<LlmEvent>, ProviderError> {
    Ok(vec![
        LlmEvent::TextDelta(text.to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage,
        },
    ])
}

fn tool_call_response(usage: TokenUsage) -> Result<Vec<LlmEvent>, ProviderError> {
    Ok(vec![
        LlmEvent::ToolUse {
            id: "call_1".to_string(),
            name: "Read".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage,
        },
    ])
}

fn message_text(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn shared_prefix_request_is_an_exact_extension_of_the_live_conversation() {
    let provider = RecordingProvider::new(vec![summary_response("<summary>ok</summary>", TokenUsage::default())]);
    let messages = sample_conversation(6);
    let prefix = live_prefix();
    let mut state = CompactState::new();

    autocompact_with_prefix(
        &provider,
        &messages,
        "test-model",
        &default_config(),
        &mut state,
        Some(&prefix),
    )
    .await
    .expect("autocompact should succeed");

    let requests = provider.requests();
    assert_eq!(requests.len(), 1, "a healthy compaction is one request");
    let request = &requests[0];

    // Everything a provider caches on — system, tools, history — is untouched.
    assert_eq!(request.system, "LIVE-SYSTEM-PROMPT");
    assert_eq!(request.tools.len(), 1);
    assert_eq!(request.tools[0].name, "Read");
    assert_eq!(request.messages.len(), messages.len() + 1);
    for (sent, original) in request.messages.iter().zip(&messages) {
        assert_eq!(sent.role, original.role);
        assert_eq!(message_text(sent), message_text(original));
    }

    // Only the instruction at the end is new, and it still forbids tools.
    let last = request.messages.last().unwrap();
    assert_eq!(last.role, Role::User);
    assert_eq!(message_text(last), build_compact_prompt());
    assert!(
        request.thinking.is_none(),
        "thinking is not mirrored into the summary request"
    );
}

#[tokio::test]
async fn standalone_form_keeps_its_own_system_prompt_and_no_tools() {
    let provider = RecordingProvider::new(vec![summary_response("<summary>ok</summary>", TokenUsage::default())]);
    let messages = sample_conversation(6);
    let mut state = CompactState::new();

    autocompact(&provider, &messages, "test-model", &default_config(), &mut state)
        .await
        .expect("autocompact should succeed");

    let requests = provider.requests();
    assert_eq!(requests[0].system, COMPACT_SYSTEM_PROMPT);
    assert!(requests[0].tools.is_empty());
}

#[tokio::test]
async fn result_reports_what_the_summary_request_consumed() {
    let usage = TokenUsage {
        input_tokens: 400_000,
        output_tokens: 3_000,
        cache_creation_tokens: 1_000,
        cache_read_tokens: 390_000,
    };
    let provider = RecordingProvider::new(vec![summary_response("<summary>ok</summary>", usage)]);
    let prefix = live_prefix();
    let mut state = CompactState::new();

    let result = autocompact_with_prefix(
        &provider,
        &sample_conversation(6),
        "test-model",
        &default_config(),
        &mut state,
        Some(&prefix),
    )
    .await
    .expect("autocompact should succeed");

    assert_eq!(result.usage.input_tokens, 400_000);
    assert_eq!(result.usage.output_tokens, 3_000);
    assert_eq!(result.usage.cache_creation_tokens, 1_000);
    assert_eq!(result.usage.cache_read_tokens, 390_000);
}

#[tokio::test]
async fn tool_call_instead_of_text_falls_back_to_the_standalone_request() {
    // With the live tools declared the model can answer with a tool call,
    // which carries no summary. That must not fail the compaction.
    let wasted = TokenUsage {
        input_tokens: 400_000,
        output_tokens: 50,
        cache_read_tokens: 390_000,
        ..Default::default()
    };
    let standalone = TokenUsage {
        input_tokens: 380_000,
        output_tokens: 2_000,
        ..Default::default()
    };
    let provider = RecordingProvider::new(vec![
        tool_call_response(wasted),
        summary_response("<summary>from fallback</summary>", standalone),
    ]);
    let prefix = live_prefix();
    let mut state = CompactState::new();

    let result = autocompact_with_prefix(
        &provider,
        &sample_conversation(6),
        "test-model",
        &default_config(),
        &mut state,
        Some(&prefix),
    )
    .await
    .expect("the fallback should rescue the compaction");

    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].system, "LIVE-SYSTEM-PROMPT");
    assert_eq!(requests[1].system, COMPACT_SYSTEM_PROMPT);
    assert!(requests[1].tools.is_empty());
    assert_eq!(state.consecutive_failures, 0);
    assert!(message_text(&result.messages[1]).contains("from fallback"));

    // Both requests were billed, including the one whose output was discarded.
    assert_eq!(result.usage.input_tokens, 780_000);
    assert_eq!(result.usage.output_tokens, 2_050);
    assert_eq!(result.usage.cache_read_tokens, 390_000);
}

#[tokio::test]
async fn shared_prefix_refused_as_too_long_retries_standalone_without_truncating() {
    // The live system prompt and tools make this request bigger than the
    // standalone one. Losing history to fit them would be the wrong trade
    // while the standalone form still fits as is.
    let provider = RecordingProvider::new(vec![
        Err(ProviderError::PromptTooLong("prompt exceeds limit".to_string())),
        summary_response("<summary>fits</summary>", TokenUsage::default()),
    ]);
    let messages = sample_conversation(10);
    let prefix = live_prefix();
    let mut state = CompactState::new();

    autocompact_with_prefix(
        &provider,
        &messages,
        "test-model",
        &default_config(),
        &mut state,
        Some(&prefix),
    )
    .await
    .expect("the standalone retry should succeed");

    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].system, COMPACT_SYSTEM_PROMPT);
    assert!(requests[1].tools.is_empty());
    assert_eq!(
        requests[1].messages.len(),
        messages.len() + 1,
        "no history was dropped for the retry"
    );
    assert_eq!(state.consecutive_failures, 0);
}

#[tokio::test]
async fn standalone_request_that_is_still_too_long_truncates_as_before() {
    let provider = RecordingProvider::new(vec![
        Err(ProviderError::PromptTooLong("shared".to_string())),
        Err(ProviderError::PromptTooLong("standalone".to_string())),
        summary_response("<summary>after truncation</summary>", TokenUsage::default()),
    ]);
    let messages = sample_conversation(10);
    let prefix = live_prefix();
    let mut state = CompactState::new();

    autocompact_with_prefix(
        &provider,
        &messages,
        "test-model",
        &default_config(),
        &mut state,
        Some(&prefix),
    )
    .await
    .expect("truncation retry should succeed");

    let requests = provider.requests();
    assert_eq!(requests.len(), 3);
    // 10 messages, oldest 20% dropped, summary prompt re-added.
    assert_eq!(requests[2].messages.len(), 8 + 1);
}

#[tokio::test]
async fn an_empty_reply_without_a_tool_call_is_not_retried() {
    // Only a tool call explains an empty reply to the tool-bearing request. A
    // reply that is just empty gives no reason to expect the standalone form to
    // do better, and sending a window-sized history again to find out is the
    // cost this whole path exists to avoid.
    let provider = RecordingProvider::new(vec![summary_response("", TokenUsage::default())]);
    let prefix = live_prefix();
    let mut state = CompactState::new();

    let result = autocompact_with_prefix(
        &provider,
        &sample_conversation(6),
        "test-model",
        &default_config(),
        &mut state,
        Some(&prefix),
    )
    .await;

    assert!(matches!(result, Err(CompactError::EmptyResponse)));
    assert_eq!(provider.requests().len(), 1, "the history was sent once");
    assert_eq!(state.consecutive_failures, 1);
}

#[tokio::test]
async fn no_text_from_either_request_fails_the_compaction() {
    let provider = RecordingProvider::new(vec![
        tool_call_response(TokenUsage::default()),
        tool_call_response(TokenUsage::default()),
    ]);
    let prefix = live_prefix();
    let mut state = CompactState::new();

    let result = autocompact_with_prefix(
        &provider,
        &sample_conversation(6),
        "test-model",
        &default_config(),
        &mut state,
        Some(&prefix),
    )
    .await;

    assert!(matches!(result, Err(CompactError::EmptyResponse)));
    assert_eq!(state.consecutive_failures, 1);
}

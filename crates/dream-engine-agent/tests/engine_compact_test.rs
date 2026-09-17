//! Black-box integration tests for engine compaction integration (TC-2.6-*).
//!
//! These tests exercise the full `AgentEngine::run()` loop and verify
//! that the compaction pipeline (microcompact → autocompact → emergency)
//! is correctly wired into the agentic loop.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;

use dream_engine_agent::engine::AgentEngine;
use dream_engine_agent::error::AgentError;
use dream_engine_agent::output::OutputSink;
use dream_engine_agent::output::terminal::TerminalSink;
use dream_engine_agent::session::SessionManager;
use dream_engine_config::compact::CompactConfig;
use dream_engine_providers::{LlmProvider, ProviderError};
use dream_engine_tools::registry::ToolRegistry;
use dream_engine_types::llm::{LlmEvent, LlmRequest};
use dream_engine_types::message::{StopReason, TokenUsage};
use tempfile::tempdir;

use common::test_config;

// ── Helpers ────────────────────────────────────────────────────────────────

fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

/// A mock provider that returns configurable per-turn events.
/// Tracks the number of stream() calls for order verification.
struct CompactMockProvider {
    turns: Mutex<VecDeque<Vec<LlmEvent>>>,
    call_count: Mutex<usize>,
}

impl CompactMockProvider {
    fn new(turns: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            turns: Mutex::new(VecDeque::from(turns)),
            call_count: Mutex::new(0),
        }
    }

    fn call_count(&self) -> usize {
        *self.call_count.lock().unwrap()
    }
}

#[async_trait]
impl LlmProvider for CompactMockProvider {
    async fn stream(&self, _request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        *self.call_count.lock().unwrap() += 1;
        let events = self.turns.lock().unwrap().pop_front().unwrap_or_else(|| {
            vec![LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage::default(),
            }]
        });

        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for event in events {
                let _ = tx.send(event).await;
            }
        });
        Ok(rx)
    }
}

/// Build events for a simple text response with configurable input_tokens.
fn text_turn(text: &str, input_tokens: u64) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text.to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage {
                input_tokens,
                output_tokens: 100,
                ..Default::default()
            },
        },
    ]
}

/// Build events for a summary LLM call (used by autocompact internally).
fn summary_turn(summary_text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(summary_text.to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage {
                input_tokens: 5_000,
                output_tokens: 2_000,
                ..Default::default()
            },
        },
    ]
}

// ── TC-2.6-01: First turn does not trigger compaction ──────────────────────

#[tokio::test]
async fn tc_2_6_01_first_turn_no_compaction() {
    // On the first turn context_tokens is 0, so neither autocompact
    // nor emergency should fire.
    let provider = Arc::new(CompactMockProvider::new(vec![text_turn("Hello", 50_000)]));

    let config = test_config();
    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider.clone(), config, registry, output, std::env::temp_dir());
    let result = engine.run("Hi", "msg-1").await.expect("should succeed");

    assert_eq!(result.text, "Hello");
    assert_eq!(result.turns, 1);
    // Only one call to stream() — no compaction call
    assert_eq!(provider.call_count(), 1);
}

// ── TC-2.6-03: Emergency truncation returns error ──────────────────────────

#[tokio::test]
async fn tc_2_6_03_emergency_returns_error() {
    // Emergency is the last safety net — it fires when autocompact is
    // disabled or circuit-broken.  We disable compact so only emergency
    // is active, then push the provider turn total above the emergency limit.
    //
    // Turn 1: tool use, returns a turn total above the emergency threshold
    // Turn 2: emergency fires before the API call → ContextTooLong
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "t1".to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: TokenUsage {
                input_tokens: 198_000, // above emergency limit (197k)
                output_tokens: 100,
                ..Default::default()
            },
        },
    ];
    // Turn 2 events are queued but should never be consumed
    let turn2 = text_turn("Should not reach", 50_000);

    let provider = Arc::new(CompactMockProvider::new(vec![turn1, turn2]));
    let mut config = test_config();
    config.compact.enabled = false; // disable auto/micro so emergency is the only gate
    config.compact.context_window = 200_000;
    config.compact.emergency_buffer = 3_000;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new("mock_tool", "result", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider.clone(), config, registry, output, std::env::temp_dir());
    let err = engine.run("Do something", "msg-1").await.unwrap_err();

    match err {
        AgentError::ContextTooLong { input_tokens, limit } => {
            // 198_000 input + 100 output + one token estimated from "result".
            assert_eq!(input_tokens, 198_101);
            assert_eq!(limit, 197_000);
        }
        other => panic!("expected ContextTooLong, got: {:?}", other),
    }

    // Only one call to stream() — second call blocked by emergency
    assert_eq!(provider.call_count(), 1);
}

// ── TC-2.6-04: Autocompact then continue ───────────────────────────────────

#[tokio::test]
async fn tc_2_6_04_autocompact_then_continue() {
    // Turn 1: tool use, returns input_tokens=170k. The window is pinned to 200k
    // so the default 80% trigger lands at 160k and 170k is above it.
    // Before turn 2: autocompact fires → LLM summary call → messages replaced
    // Turn 2 (after compact): text response with low input_tokens
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "t1".to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: TokenUsage {
                input_tokens: 170_000,
                output_tokens: 100,
                ..Default::default()
            },
        },
    ];
    let compact_summary = summary_turn("<summary>Conversation summary</summary>");
    let turn2_after_compact = text_turn("Continuing after compact", 10_000);

    let provider = Arc::new(CompactMockProvider::new(vec![
        turn1,
        compact_summary,
        turn2_after_compact,
    ]));

    let mut config = test_config();
    config.compact = CompactConfig {
        context_window: 200_000,
        ..CompactConfig::default()
    };

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new("mock_tool", "result", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider.clone(), config, registry, output, std::env::temp_dir());
    let result = engine
        .run("Start work", "msg-1")
        .await
        .expect("should succeed after compact");

    assert_eq!(result.text, "Continuing after compact");
    assert_eq!(result.turns, 2);
    // 3 calls: turn1 + compact summary + turn2
    assert_eq!(provider.call_count(), 3);
}

// ── TC-2.6-05: Session save includes compacted messages ────────────────────

#[tokio::test]
async fn tc_2_6_05_session_save_after_compact() {
    let dir = tempdir().expect("tempdir");

    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "t1".to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: TokenUsage {
                input_tokens: 170_000,
                output_tokens: 100,
                ..Default::default()
            },
        },
    ];
    let compact_summary = summary_turn("<summary>Session summary</summary>");
    let turn2 = text_turn("After compact", 10_000);

    let provider = Arc::new(CompactMockProvider::new(vec![turn1, compact_summary, turn2]));

    let mut config = test_config();
    // Window pinned to 200k so the default 80% trigger lands at 160k and the
    // 170k turn below is above it.
    config.compact = CompactConfig {
        context_window: 200_000,
        ..CompactConfig::default()
    };
    config.session.enabled = true;
    config.session.directory = dir.path().to_string_lossy().into_owned();

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new("mock_tool", "result", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    engine.init_session("test", "/tmp", None).expect("init session");

    engine.run("Start", "msg-1").await.expect("should succeed");

    // Load the saved session
    let mgr = SessionManager::new(dir.path().to_path_buf(), 10);
    let session = mgr.load("latest").expect("load session");

    // After compaction + turn2, messages should include the compact boundary,
    // summary, and the post-compact assistant/user messages.
    // The exact count depends on implementation, but should be small (not
    // the full pre-compact count).
    assert!(
        session.messages.len() < 10,
        "session should have compacted messages, got {}",
        session.messages.len()
    );

    // Verify at least one message contains compact boundary marker
    let has_boundary = session.messages.iter().any(|m| {
        m.content.iter().any(|b| {
            matches!(b, dream_engine_types::message::ContentBlock::Text { text } if text.contains("[Conversation compacted]"))
        })
    });
    assert!(has_boundary, "session should contain compact boundary marker");
}

// ── TC-2.6-06: Disabled skips all except emergency ─────────────────────────

#[tokio::test]
async fn tc_2_6_06_disabled_skips_micro_auto() {
    // With compact disabled, a text response that reports high usage
    // should not trigger autocompact (only emergency if at limit).
    let provider = Arc::new(CompactMockProvider::new(vec![
        // Returns high but not emergency-level tokens
        text_turn("Normal response", 170_000),
    ]));

    let mut config = test_config();
    config.compact.enabled = false;

    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider.clone(), config, registry, output, std::env::temp_dir());
    let result = engine.run("Hi", "msg-1").await.expect("should succeed");

    assert_eq!(result.text, "Normal response");
    // Only 1 call — no compact summary call
    assert_eq!(provider.call_count(), 1);
}

#[tokio::test]
async fn tc_2_6_06b_disabled_still_fires_emergency() {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "t1".to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: TokenUsage {
                input_tokens: 198_000,
                output_tokens: 100,
                ..Default::default()
            },
        },
    ];

    let provider = Arc::new(CompactMockProvider::new(vec![turn1, text_turn("unreachable", 0)]));

    let mut config = test_config();
    config.compact.enabled = false;
    config.compact.context_window = 200_000;
    config.compact.emergency_buffer = 3_000;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new("mock_tool", "result", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let err = engine.run("Go", "msg-1").await.unwrap_err();

    assert!(
        matches!(err, AgentError::ContextTooLong { .. }),
        "emergency should fire even when disabled"
    );
}

// ── TC-2.6-07: input_tokens correctly tracked ──────────────────────────────

#[tokio::test]
async fn tc_2_6_07_input_tokens_tracked() {
    // Two turns: first returns 50k tokens, second returns 60k tokens.
    // We verify that the engine updates compact state after each turn.
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "t1".to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: TokenUsage {
                input_tokens: 50_000,
                output_tokens: 100,
                ..Default::default()
            },
        },
    ];
    let turn2 = text_turn("Done", 60_000);

    let provider = Arc::new(CompactMockProvider::new(vec![turn1, turn2]));

    let config = test_config();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new("mock_tool", "result", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let result = engine.run("Work", "msg-1").await.expect("should succeed");

    assert_eq!(result.turns, 2);
    // Total usage should accumulate: 50k + 60k = 110k input tokens
    assert_eq!(result.usage.input_tokens, 110_000);
}

// ── TC-2.6-02: Execution order — micro before auto ────────────────────────

#[tokio::test]
async fn tc_2_6_02_micro_before_auto_execution_order() {
    // Build a scenario where both microcompact and autocompact trigger
    // in the same compaction cycle.  A custom provider captures the
    // messages sent to the autocompact LLM call so we can verify that
    // microcompact already cleared old tool results before autocompact
    // was invoked.

    let captured: Arc<Mutex<Option<Vec<dream_engine_types::message::Message>>>> = Arc::new(Mutex::new(None));
    let capture_ref = captured.clone();

    struct OrderProvider {
        regular_count: Mutex<usize>,
        captured: Arc<Mutex<Option<Vec<dream_engine_types::message::Message>>>>,
    }

    #[async_trait]
    impl LlmProvider for OrderProvider {
        async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            let is_compact = request.tools.is_empty();

            if is_compact {
                // Capture messages that autocompact sends to the LLM
                *self.captured.lock().unwrap() = Some(request.messages.clone());

                let events = vec![
                    LlmEvent::TextDelta("<summary>Order test summary</summary>".to_string()),
                    LlmEvent::Done {
                        stop_reason: StopReason::EndTurn,
                        usage: TokenUsage {
                            input_tokens: 5_000,
                            output_tokens: 2_000,
                            ..Default::default()
                        },
                    },
                ];
                let (tx, rx) = mpsc::channel(64);
                tokio::spawn(async move {
                    for e in events {
                        let _ = tx.send(e).await;
                    }
                });
                return Ok(rx);
            }

            let count = {
                let mut c = self.regular_count.lock().unwrap();
                let v = *c;
                *c += 1;
                v
            };

            // Turns 0-6: tool use.  Turn 6 reports high input_tokens
            // so that micro and auto both trigger in the SAME cycle
            // (turn 7's run_compaction).
            // Turn 7 (after compact): text to end the run.
            //
            // micro_keep_recent = 3 → count threshold = 6.
            // After 7 tool-use turns: 7 > 6 → micro fires.
            // After turn 6: context_tokens = 170k > 167k → auto fires.
            let events = if count < 7 {
                let input_tokens = if count == 6 { 170_000 } else { 10_000 };
                vec![
                    LlmEvent::ToolUse {
                        id: format!("t{count}"),
                        name: "mock_tool".to_string(),
                        input: serde_json::json!({}),
                        extra: None,
                    },
                    LlmEvent::Done {
                        stop_reason: StopReason::ToolUse,
                        usage: TokenUsage {
                            input_tokens,
                            output_tokens: 100,
                            ..Default::default()
                        },
                    },
                ]
            } else {
                vec![
                    LlmEvent::TextDelta("Done after compact".to_string()),
                    LlmEvent::Done {
                        stop_reason: StopReason::EndTurn,
                        usage: TokenUsage {
                            input_tokens: 5_000,
                            output_tokens: 100,
                            ..Default::default()
                        },
                    },
                ]
            };

            let (tx, rx) = mpsc::channel(64);
            tokio::spawn(async move {
                for e in events {
                    let _ = tx.send(e).await;
                }
            });
            Ok(rx)
        }
    }

    let provider = Arc::new(OrderProvider {
        regular_count: Mutex::new(0),
        captured: capture_ref,
    });

    let mut config = test_config();
    config.compact = CompactConfig {
        microcompact_enabled: true,
        micro_keep_recent: 3,
        compactable_tools: vec!["mock_tool".into()],
        context_window: 200_000,
        emergency_buffer: 3_000,
        ..Default::default()
    };
    config.max_turns = Some(20);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new("mock_tool", "tool output data", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let result = engine.run("Start", "msg-1").await.expect("should succeed");

    assert_eq!(result.text, "Done after compact");

    // Verify: the messages that autocompact received should contain
    // tool results cleared by microcompact (proving micro ran first
    // within the SAME compaction cycle).
    let msgs = captured.lock().unwrap();
    let msgs = msgs.as_ref().expect("autocompact should have been called");

    let cleared_count = msgs
        .iter()
        .flat_map(|m| m.content.iter())
        .filter(|b| {
            matches!(
                b,
                dream_engine_types::message::ContentBlock::ToolResult { content, .. }
                    if content == dream_engine_agent::compact::micro::CLEARED_TOOL_RESULT
            )
        })
        .count();

    // 7 tool results total, keep_recent=3 → 4 cleared by micro
    // before auto received the messages.
    assert_eq!(
        cleared_count, 4,
        "microcompact should have cleared 4 tool results before autocompact ran"
    );
}

// ── TC-2.6-E2E-02: Microcompact + autocompact cooperative scenario ────────

#[tokio::test]
async fn tc_2_6_e2e_02_micro_and_auto_cooperative() {
    // Verify that microcompact and autocompact cooperate in the same
    // compaction cycle.  Microcompact frees some tokens from old tool
    // results, and autocompact still fires because the context estimate
    // (which is not reduced by micro) remains above threshold.

    let compact_call_count: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let counter_ref = compact_call_count.clone();

    struct CoopProvider {
        regular_count: Mutex<usize>,
        compact_calls: Arc<Mutex<usize>>,
    }

    #[async_trait]
    impl LlmProvider for CoopProvider {
        async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            let is_compact = request.tools.is_empty();

            if is_compact {
                *self.compact_calls.lock().unwrap() += 1;

                let events = vec![
                    LlmEvent::TextDelta("<summary>Cooperative summary</summary>".to_string()),
                    LlmEvent::Done {
                        stop_reason: StopReason::EndTurn,
                        usage: TokenUsage {
                            input_tokens: 5_000,
                            output_tokens: 2_000,
                            ..Default::default()
                        },
                    },
                ];
                let (tx, rx) = mpsc::channel(64);
                tokio::spawn(async move {
                    for e in events {
                        let _ = tx.send(e).await;
                    }
                });
                return Ok(rx);
            }

            let count = {
                let mut c = self.regular_count.lock().unwrap();
                let v = *c;
                *c += 1;
                v
            };

            // 7 tool-use turns (count 0-6).  Turn 6 returns high tokens.
            // micro_keep_recent = 3 → count threshold = 6.
            // After 7 tool results: 7 > 6 → micro fires.
            // After turn 6: context_tokens = 170k > 167k → auto fires.
            let events = if count < 7 {
                let input_tokens = if count == 6 { 170_000 } else { 10_000 };
                vec![
                    LlmEvent::ToolUse {
                        id: format!("t{count}"),
                        name: "mock_tool".to_string(),
                        input: serde_json::json!({}),
                        extra: None,
                    },
                    LlmEvent::Done {
                        stop_reason: StopReason::ToolUse,
                        usage: TokenUsage {
                            input_tokens,
                            output_tokens: 100,
                            ..Default::default()
                        },
                    },
                ]
            } else {
                vec![
                    LlmEvent::TextDelta("After cooperative compact".to_string()),
                    LlmEvent::Done {
                        stop_reason: StopReason::EndTurn,
                        usage: TokenUsage {
                            input_tokens: 5_000,
                            output_tokens: 100,
                            ..Default::default()
                        },
                    },
                ]
            };

            let (tx, rx) = mpsc::channel(64);
            tokio::spawn(async move {
                for e in events {
                    let _ = tx.send(e).await;
                }
            });
            Ok(rx)
        }
    }

    let provider = Arc::new(CoopProvider {
        regular_count: Mutex::new(0),
        compact_calls: counter_ref,
    });

    let mut config = test_config();
    config.compact = CompactConfig {
        microcompact_enabled: true,
        micro_keep_recent: 3,
        compactable_tools: vec!["mock_tool".into()],
        context_window: 200_000,
        emergency_buffer: 3_000,
        ..Default::default()
    };
    config.max_turns = Some(20);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new("mock_tool", "tool output data", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let result = engine.run("Work", "msg-1").await.expect("should succeed");

    assert_eq!(result.text, "After cooperative compact");

    // Autocompact was called exactly once (micro freed tokens but
    // did not reduce context_tokens, so auto still fired).
    let calls = *compact_call_count.lock().unwrap();
    assert_eq!(
        calls, 1,
        "autocompact should fire exactly once despite microcompact running first"
    );

    // Total turns: 7 tool-use + 1 post-compact text = 8 engine turns,
    // plus 1 internal compact LLM call = 9 provider calls.
    assert_eq!(result.turns, 8);
}

// ── TC-2.6-E2E-03: Circuit breaker after repeated failures ─────────────────

#[tokio::test]
async fn tc_2_6_e2e_03_circuit_breaker_stops_retries() {
    // Simulate: 3 turns where autocompact would trigger but fails each time.
    // After 3 failures the circuit breaker trips and autocompact stops.
    //
    // We use a provider that always fails the compact summary call with
    // a generic API error, but succeeds for regular model turns.

    struct CircuitBreakerProvider {
        call_index: Mutex<usize>,
    }

    #[async_trait]
    impl LlmProvider for CircuitBreakerProvider {
        async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            let idx = {
                let mut i = self.call_index.lock().unwrap();
                let v = *i;
                *i += 1;
                v
            };

            // Compact summary calls have no tools defined and include the
            // compact prompt in messages. We detect them by checking tools.is_empty().
            let is_compact_call = request.tools.is_empty();

            if is_compact_call {
                return Err(ProviderError::Api {
                    status: 500,
                    message: "Internal error".to_string(),
                });
            }

            // Regular model turns: tool use on odd calls, text on even
            let events = if idx % 2 == 0 {
                // Tool use turn → keeps the loop going
                vec![
                    LlmEvent::ToolUse {
                        id: format!("t{idx}"),
                        name: "mock_tool".to_string(),
                        input: serde_json::json!({}),
                        extra: None,
                    },
                    LlmEvent::Done {
                        stop_reason: StopReason::ToolUse,
                        usage: TokenUsage {
                            input_tokens: 170_000, // above autocompact threshold
                            output_tokens: 100,
                            ..Default::default()
                        },
                    },
                ]
            } else {
                // Text turn → ends the loop
                vec![
                    LlmEvent::TextDelta("Final".to_string()),
                    LlmEvent::Done {
                        stop_reason: StopReason::EndTurn,
                        usage: TokenUsage {
                            input_tokens: 170_000,
                            output_tokens: 100,
                            ..Default::default()
                        },
                    },
                ]
            };

            let (tx, rx) = mpsc::channel(64);
            tokio::spawn(async move {
                for event in events {
                    let _ = tx.send(event).await;
                }
            });
            Ok(rx)
        }
    }

    let provider = Arc::new(CircuitBreakerProvider {
        call_index: Mutex::new(0),
    });

    let mut config = test_config();
    config.compact = CompactConfig {
        max_failures: 3,
        // Set emergency very high so it doesn't interfere
        context_window: 500_000,
        emergency_buffer: 3_000,
        ..Default::default()
    };
    config.max_turns = Some(10);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new("mock_tool", "result", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output, std::env::temp_dir());
    let result = engine.run("Work", "msg-1").await.expect("should succeed");

    assert_eq!(result.text, "Final");
}

// ── Compaction reaches the user in a language the host can translate ────────

/// Records coded tips without falling back to English, the way an embedding
/// host with a translation catalogue does.
#[derive(Default)]
struct CodedTipSink {
    coded: Mutex<Vec<(String, serde_json::Value, String)>>,
    plain: Mutex<Vec<String>>,
}

impl OutputSink for CodedTipSink {
    fn emit_text_delta(&self, _text: &str, _msg_id: &str) {}
    fn emit_thinking(&self, _text: &str, _msg_id: &str) {}
    fn emit_tool_call(&self, _tool_use_id: &str, _name: &str, _input: &str) {}
    fn emit_tool_result(&self, _tool_use_id: &str, _name: &str, _is_error: bool, _content: &str) {}
    fn emit_stream_start(&self, _msg_id: &str) {}
    fn emit_stream_end(&self, _msg_id: &str, _turns: usize, _i: u64, _o: u64, _cc: u64, _cr: u64) {}
    fn emit_error(&self, _msg: &str) {}
    fn emit_info(&self, msg: &str) {
        self.plain.lock().unwrap().push(msg.to_string());
    }
    fn emit_info_coded(&self, code: &str, params: serde_json::Value, fallback: &str) {
        self.coded
            .lock()
            .unwrap()
            .push((code.to_string(), params, fallback.to_string()));
    }
}

#[tokio::test]
async fn autocompact_announces_itself_with_a_code_the_host_can_translate() {
    // Compaction must not be silent — the user's history just got summarized —
    // but the announcement used to be English prose a translated UI could do
    // nothing with, alongside a raw "Autocompact threshold: N tokens (80% of
    // M)" line that is diagnostics, not a user message.
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "t1".to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: TokenUsage {
                input_tokens: 170_000,
                output_tokens: 100,
                ..Default::default()
            },
        },
    ];
    let provider = Arc::new(CompactMockProvider::new(vec![
        turn1,
        summary_turn("<summary>Summary</summary>"),
        text_turn("Continuing", 10_000),
    ]));

    let mut config = test_config();
    // 200k window puts the default 80% trigger at 160k, below the 170k above.
    config.compact = CompactConfig {
        context_window: 200_000,
        ..CompactConfig::default()
    };

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new("mock_tool", "result", false)));

    let sink = Arc::new(CodedTipSink::default());
    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        registry,
        Arc::clone(&sink) as Arc<dyn OutputSink>,
        std::env::temp_dir(),
    );
    engine.run("Start", "msg-1").await.expect("run should succeed");

    let coded = sink.coded.lock().unwrap();
    let done = coded
        .iter()
        .find(|(code, _, _)| code == "AUTOCOMPACT_DONE")
        .expect("compaction must announce itself");
    assert_eq!(
        done.1["count"],
        serde_json::json!(3),
        "the host renders the count itself"
    );
    assert_eq!(done.1["tokens"], serde_json::json!(170_101));
    assert!(
        done.2.contains("Autocompact"),
        "a host without a catalogue still needs readable English"
    );

    let plain = sink.plain.lock().unwrap();
    assert!(
        plain.is_empty(),
        "compaction must not also emit untranslatable plain tips: {plain:?}"
    );
}

// ── Recovering when the provider says the prompt does not fit ───────────────

/// Rejects the first generation with a provider overflow error, then behaves.
struct OverflowThenOkProvider {
    rejected: Mutex<bool>,
    turns: Mutex<VecDeque<Vec<LlmEvent>>>,
    calls: Mutex<usize>,
}

impl OverflowThenOkProvider {
    fn new(turns: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            rejected: Mutex::new(false),
            turns: Mutex::new(VecDeque::from(turns)),
            calls: Mutex::new(0),
        }
    }
}

#[async_trait]
impl LlmProvider for OverflowThenOkProvider {
    async fn stream(&self, _request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        *self.calls.lock().unwrap() += 1;
        let mut rejected = self.rejected.lock().unwrap();
        if !*rejected {
            *rejected = true;
            return Err(ProviderError::PromptTooLong(
                "This model's maximum context length is 200000 tokens. However, you requested \
                 240000 tokens (235000 in the messages, 5000 in the completion)."
                    .to_string(),
            ));
        }
        drop(rejected);
        let events = self.turns.lock().unwrap().pop_front().unwrap_or_else(|| {
            vec![LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage::default(),
            }]
        });
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for event in events {
                let _ = tx.send(event).await;
            }
        });
        Ok(rx)
    }
}

#[tokio::test]
async fn a_provider_overflow_is_recovered_instead_of_failing_the_turn() {
    // The situation a user who has never heard of a context window lands in:
    // the model's real window is smaller than the one assumed for it, so the
    // local estimate says there is room, autocompact never fires, and the
    // provider rejects the turn. Nothing about that is actionable for them.
    let provider = Arc::new(OverflowThenOkProvider::new(vec![
        summary_turn("<summary>Summary</summary>"),
        text_turn("Recovered and answered", 50_000),
    ]));

    let mut config = test_config();
    // The wrong assumption: a window far larger than the model really has.
    config.compact = CompactConfig {
        context_window: 1_000_000,
        ..CompactConfig::default()
    };

    let sink = Arc::new(CodedTipSink::default());
    let mut engine = AgentEngine::new_with_provider(
        Arc::clone(&provider) as Arc<dyn LlmProvider>,
        config,
        ToolRegistry::new(),
        Arc::clone(&sink) as Arc<dyn OutputSink>,
        std::env::temp_dir(),
    );

    let result = engine
        .run("Keep going", "msg-1")
        .await
        .expect("an overflow the provider can describe must not end the turn");
    assert_eq!(result.text, "Recovered and answered");

    let coded = sink.coded.lock().unwrap();
    let learned = coded
        .iter()
        .find(|(code, _, _)| code == "CONTEXT_WINDOW_LEARNED")
        .expect("the user should be told the window was smaller than assumed");
    assert_eq!(
        learned.1["tokens"],
        serde_json::json!(200_000),
        "the window stated by the provider is the one to believe, not the 240000 we sent"
    );
    assert!(
        coded.iter().any(|(code, _, _)| code == "AUTOCOMPACT_DONE"),
        "recovery has to actually free context, not just relabel the failure"
    );
}

#[tokio::test]
async fn an_overflow_with_no_stated_limit_still_narrows_and_recovers() {
    // Ollama reports no number at all. The size of the prompt just refused is
    // still an upper bound on the real window, which is evidence rather than a
    // guess — and without using it, the next turn would repeat the failure.
    struct NoNumberOverflow {
        rejected: Mutex<bool>,
        turns: Mutex<VecDeque<Vec<LlmEvent>>>,
    }

    #[async_trait]
    impl LlmProvider for NoNumberOverflow {
        async fn stream(&self, _request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            let mut rejected = self.rejected.lock().unwrap();
            if !*rejected {
                *rejected = true;
                return Err(ProviderError::PromptTooLong(
                    "llm: context overflow - prompt exceeds the available context window".to_string(),
                ));
            }
            drop(rejected);
            let events = self.turns.lock().unwrap().pop_front().unwrap_or_default();
            let (tx, rx) = mpsc::channel(64);
            tokio::spawn(async move {
                for event in events {
                    let _ = tx.send(event).await;
                }
            });
            Ok(rx)
        }
    }

    let provider = Arc::new(NoNumberOverflow {
        rejected: Mutex::new(false),
        turns: Mutex::new(VecDeque::from(vec![
            summary_turn("<summary>Summary</summary>"),
            text_turn("Recovered", 4_000),
        ])),
    });

    let mut config = test_config();
    config.compact = CompactConfig {
        context_window: 1_000_000,
        ..CompactConfig::default()
    };

    let sink = Arc::new(CodedTipSink::default());
    let mut engine = AgentEngine::new_with_provider(
        provider as Arc<dyn LlmProvider>,
        config,
        ToolRegistry::new(),
        Arc::clone(&sink) as Arc<dyn OutputSink>,
        std::env::temp_dir(),
    );

    let result = engine.run("Keep going", "msg-1").await.expect("should recover");
    assert_eq!(result.text, "Recovered");
    assert!(
        sink.coded
            .lock()
            .unwrap()
            .iter()
            .any(|(code, _, _)| code == "AUTOCOMPACT_DONE"),
        "with no number to learn, compaction alone has to carry the recovery"
    );
}

// ── The output budget has to fit the window too ─────────────────────────────

/// Captures the `max_tokens` of every request it is handed.
struct RecordingProvider {
    requested: Mutex<Vec<Option<u32>>>,
    turns: Mutex<VecDeque<Vec<LlmEvent>>>,
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.requested.lock().unwrap().push(request.max_tokens);
        let events = self.turns.lock().unwrap().pop_front().unwrap_or_default();
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for event in events {
                let _ = tx.send(event).await;
            }
        });
        Ok(rx)
    }
}

async fn max_tokens_asked_for(context_window: usize) -> Option<u32> {
    let provider = Arc::new(RecordingProvider {
        requested: Mutex::new(Vec::new()),
        turns: Mutex::new(VecDeque::from(vec![text_turn("done", 1_000)])),
    });
    let mut config = test_config();
    config.max_tokens = Some(32_000);
    config.compact = CompactConfig {
        context_window,
        ..CompactConfig::default()
    };
    let mut engine = AgentEngine::new_with_provider(
        Arc::clone(&provider) as Arc<dyn LlmProvider>,
        config,
        ToolRegistry::new(),
        silent_output(),
        std::env::temp_dir(),
    );
    engine.run("hi", "msg-1").await.expect("run");
    let asked = provider.requested.lock().unwrap();
    asked[0]
}

#[tokio::test]
async fn the_output_budget_is_capped_to_what_the_window_can_hold() {
    // The bug this guards, seen on a real rejection: a 32768-token model with
    // a 32000-token output default leaves 768 tokens for everything else, so
    // every request was refused — "you requested about 44055 tokens (7451 of
    // text input, 4604 of tool input, 32000 in the output)" — regardless of how
    // short the conversation was. No amount of compaction could have fixed it.
    let asked = max_tokens_asked_for(32_768).await.expect("a budget must still be sent");
    assert!(
        asked < 32_000,
        "the 32000-token default has to be cut down on a 32768-token window, got {asked}"
    );
    assert!(
        (asked as usize) < 32_768,
        "the budget alone must not consume the whole window, got {asked}"
    );
}

#[tokio::test]
async fn a_large_window_leaves_the_requested_budget_alone() {
    // The cap must only bind where the window is genuinely tight; clamping a
    // 1M-window model down would shorten every answer for no reason.
    assert_eq!(max_tokens_asked_for(1_000_000).await, Some(32_000));
}

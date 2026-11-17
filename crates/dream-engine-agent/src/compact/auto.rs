//! Autocompact: context-threshold-triggered LLM summarization.
//!
//! When the best-known context size exceeds the configured threshold, this module
//! calls the LLM to produce a structured summary of the conversation,
//! then replaces the full history with a compact boundary marker and the
//! summary.  A circuit breaker prevents runaway retries.

use dream_engine_config::compact::CompactConfig;
use dream_engine_providers::{LlmProvider, ProviderError};
use dream_engine_types::compact::{CompactMetadata, CompactTrigger};
use dream_engine_types::llm::{LlmEvent, LlmRequest};
use dream_engine_types::message::{ContentBlock, Message, Role, TokenUsage};
use dream_engine_types::tool::ToolDef;
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::prompt::{
    COMPACT_MAX_OUTPUT_TOKENS, COMPACT_SYSTEM_PROMPT, build_compact_prompt, build_summary_content,
    format_compact_summary,
};
use super::state::CompactState;

/// Maximum number of prompt-too-long retries.
const MAX_PTL_RETRIES: u32 = 2;

/// Content prefix for the compact boundary marker message.
pub const BOUNDARY_PREFIX: &str = "[Conversation compacted]";

// ── Public types ────────────────────────────────────────────────────────────

/// Result of a successful autocompact operation.
#[derive(Debug, Clone)]
pub struct CompactResult {
    /// Post-compact messages that replace the original conversation.
    /// Contains a boundary marker and a summary message.
    pub messages: Vec<Message>,
    /// How many original messages were summarized.
    pub messages_summarized: usize,
    /// Best-known context token count before compaction.
    pub pre_compact_tokens: u64,
    /// Tokens the summary request(s) consumed, including an attempt whose
    /// output was thrown away. This is what the provider billed, not the size
    /// of the new context: callers add it to cumulative totals only.
    pub usage: TokenUsage,
}

/// The live conversation's system prompt and tool declarations.
///
/// Providers cache by request prefix, and these two come first. A summary
/// request that carries its own tiny system prompt and no tools therefore
/// diverges from the conversation at the very first token, and the whole
/// history — close to the context window by definition, since that is what
/// triggers compaction — is billed as uncached input. Sending the live ones
/// instead keeps the request an exact extension of the conversation, so only
/// the summary instruction at the end is new.
#[derive(Debug, Clone)]
pub struct CompactPrefix {
    pub system: String,
    pub tools: Vec<ToolDef>,
}

/// Errors specific to autocompact.
#[derive(Debug, thiserror::Error)]
pub enum CompactError {
    #[error("LLM provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("Prompt too long after {attempts} retries")]
    PromptTooLong { attempts: u32 },
    #[error("Empty response from LLM")]
    EmptyResponse,
    #[error("Stream error: {0}")]
    StreamError(String),
    #[error("Circuit breaker tripped after {failures} consecutive failures")]
    CircuitBroken { failures: u32 },
}

// ── Trigger check ───────────────────────────────────────────────────────────

/// Check if autocompact should trigger based on the best-known context size.
///
/// When `autocompact_threshold_pct` is set, threshold = context_window * pct / 100.
/// Otherwise falls back to: `threshold = context_window - output_reserve - autocompact_buffer`
pub fn should_autocompact(context_tokens: u64, config: &CompactConfig) -> bool {
    if !config.enabled {
        return false;
    }
    let threshold = if let Some(pct) = config.autocompact_threshold_pct {
        config.context_window * pct as usize / 100
    } else {
        let effective_window = config.context_window.saturating_sub(config.output_reserve);
        effective_window.saturating_sub(config.autocompact_buffer)
    };
    context_tokens as usize >= threshold
}

// ── Core autocompact ────────────────────────────────────────────────────────

/// Execute autocompact with a standalone summary request (its own system
/// prompt, no tools).
///
/// Prefer [`autocompact_with_prefix`] whenever the live conversation's system
/// prompt and tools are at hand: this form shares no prompt-cache prefix with
/// the conversation it summarizes.
pub async fn autocompact(
    provider: &dyn LlmProvider,
    messages: &[Message],
    model: &str,
    config: &CompactConfig,
    state: &mut CompactState,
) -> Result<CompactResult, CompactError> {
    autocompact_with_prefix(provider, messages, model, config, state, None).await
}

/// Execute autocompact: call LLM to summarize the conversation.
///
/// 1. Build a summary prompt and send conversation + prompt to the LLM. With a
///    `prefix`, the request reuses the live system prompt and tools so the
///    provider can serve the whole history from its prompt cache.
/// 2. If that request is refused as too long, or the model answers with a tool
///    call instead of text, fall back to the standalone request below. Both
///    can only happen because the prefix is bigger / tool-bearing; neither is
///    a reason to fail a compaction that the standalone form can still do.
/// 3. If the prompt is still too long, truncate oldest 20% messages and retry
///    (up to [`MAX_PTL_RETRIES`] times).
/// 4. Parse the `<summary>` from the response.
/// 5. Return a [`CompactResult`] with boundary marker + summary messages.
///
/// On failure, increments `state.consecutive_failures`.
/// On success, resets the failure counter.
pub async fn autocompact_with_prefix(
    provider: &dyn LlmProvider,
    messages: &[Message],
    model: &str,
    config: &CompactConfig,
    state: &mut CompactState,
    prefix: Option<&CompactPrefix>,
) -> Result<CompactResult, CompactError> {
    // Circuit breaker check
    if state.is_circuit_broken(config) {
        return Err(CompactError::CircuitBroken {
            failures: state.consecutive_failures,
        });
    }

    let pre_compact_tokens = state.last_input_tokens;
    let messages_summarized = messages.len();

    // Build messages for the compact LLM call: conversation + summary prompt
    let prompt = build_compact_prompt();
    let mut conv_messages = messages.to_vec();
    conv_messages.push(Message::new(Role::User, vec![ContentBlock::Text { text: prompt }]));

    let mut ptl_attempts = 0u32;
    // Cleared once the shared-prefix request has been given up on.
    let mut shared_prefix = prefix;
    let mut usage = TokenUsage::default();

    let summary_text = loop {
        let request = match shared_prefix {
            Some(live) => LlmRequest {
                model: model.to_string(),
                system: live.system.clone(),
                messages: conv_messages.clone(),
                tools: live.tools.clone(),
                max_tokens: Some(COMPACT_MAX_OUTPUT_TOKENS),
                // Deliberately not mirrored from the conversation: a thinking
                // budget at or above the summary's output cap is rejected by
                // providers that require `max_tokens` to exceed it.
                thinking: None,
                reasoning_effort: None,
            },
            None => LlmRequest {
                model: model.to_string(),
                system: COMPACT_SYSTEM_PROMPT.to_string(),
                messages: conv_messages.clone(),
                tools: vec![],
                max_tokens: Some(COMPACT_MAX_OUTPUT_TOKENS),
                thinking: None,
                reasoning_effort: None,
            },
        };

        match provider.stream(&request).await {
            Ok(rx) => match collect_stream_text(rx).await {
                Ok(response) => {
                    // Billed whether or not the text is used.
                    usage.accumulate(&response.usage);
                    if response.text.trim().is_empty() && response.called_tool && shared_prefix.is_some() {
                        // The tools are declared on this request, so the model
                        // may answer with a tool call despite the prompt. That
                        // yields no text, and the standalone request, which
                        // declares none, cannot do it. A reply that is merely
                        // empty is not this case: repeating a request of this
                        // size cannot be expected to fare better.
                        warn!(
                            target: "dream_engine_agent",
                            "shared-prefix compaction answered with a tool call; retrying without tools"
                        );
                        shared_prefix = None;
                        continue;
                    }
                    break response.text;
                }
                Err(e) => {
                    state.record_failure();
                    return Err(e);
                }
            },
            // The live system prompt and tools are part of this request and
            // not of the standalone one, so it can be refused for length when
            // the standalone form would fit. Try that before truncating.
            Err(ProviderError::PromptTooLong(_)) if shared_prefix.is_some() => {
                warn!(
                    target: "dream_engine_agent",
                    "shared-prefix compaction refused as too long; retrying without the live prefix"
                );
                shared_prefix = None;
            }
            Err(ProviderError::PromptTooLong(_)) if ptl_attempts < MAX_PTL_RETRIES => {
                ptl_attempts += 1;
                // Remove the summary prompt (last msg), truncate, re-add prompt
                let conversation_part = &conv_messages[..conv_messages.len() - 1];
                match truncate_for_retry(conversation_part) {
                    Some(mut truncated) => {
                        truncated.push(Message::new(
                            Role::User,
                            vec![ContentBlock::Text {
                                text: build_compact_prompt(),
                            }],
                        ));
                        conv_messages = truncated;
                    }
                    None => {
                        state.record_failure();
                        return Err(CompactError::PromptTooLong { attempts: ptl_attempts });
                    }
                }
            }
            Err(ProviderError::PromptTooLong(_)) => {
                state.record_failure();
                return Err(CompactError::PromptTooLong { attempts: ptl_attempts });
            }
            Err(e) => {
                state.record_failure();
                return Err(CompactError::Provider(e));
            }
        }
    };

    if summary_text.trim().is_empty() {
        state.record_failure();
        return Err(CompactError::EmptyResponse);
    }

    // Format and build post-compact messages
    let formatted = format_compact_summary(&summary_text);
    let summary_content = build_summary_content(&formatted, true);

    let metadata = CompactMetadata {
        trigger: CompactTrigger::Auto,
        pre_compact_tokens,
        messages_summarized,
    };

    let boundary_text = format!(
        "{BOUNDARY_PREFIX}\n{}",
        serde_json::to_string(&metadata).expect("CompactMetadata serialization cannot fail")
    );

    let boundary_msg = Message::new(Role::User, vec![ContentBlock::Text { text: boundary_text }]);

    let summary_msg = Message::new(Role::User, vec![ContentBlock::Text { text: summary_content }]);

    state.record_success();

    // Production-visible on purpose: this single request is the largest input
    // a session sends, and whether it was served from cache is exactly what
    // cost reports cannot show once it is missing from the session totals.
    info!(
        target: "dream_engine_agent",
        shared_prefix = shared_prefix.is_some(),
        input_tokens = usage.input_tokens,
        cache_read_tokens = usage.cache_read_tokens,
        cache_creation_tokens = usage.cache_creation_tokens,
        output_tokens = usage.output_tokens,
        messages_summarized,
        "autocompact summary request finished"
    );

    Ok(CompactResult {
        messages: vec![boundary_msg, summary_msg],
        messages_summarized,
        pre_compact_tokens,
        usage,
    })
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// What a summary request produced.
struct SummaryResponse {
    text: String,
    usage: TokenUsage,
    /// The model answered with a tool call. Ignored as output, but it explains
    /// an empty `text` when the request declared tools.
    called_tool: bool,
}

/// Collect all text from a streaming LLM response.
async fn collect_stream_text(mut rx: mpsc::Receiver<LlmEvent>) -> Result<SummaryResponse, CompactError> {
    let mut text = String::new();
    let mut called_tool = false;

    while let Some(event) = rx.recv().await {
        match event {
            LlmEvent::TextDelta(delta) => text.push_str(&delta),
            LlmEvent::ToolUse { .. } | LlmEvent::ToolCallTruncated { .. } => called_tool = true,
            LlmEvent::Done { usage, .. } => {
                return Ok(SummaryResponse {
                    text,
                    usage,
                    called_tool,
                });
            }
            LlmEvent::Error(e) => return Err(CompactError::StreamError(e)),
            // Thinking deltas and provider items carry no summary text.
            _ => {}
        }
    }

    // Channel closed without a Done event
    Err(CompactError::EmptyResponse)
}

/// Truncate the oldest ~20% of messages for PTL retry.
///
/// Returns `None` if there are too few messages to truncate meaningfully.
fn truncate_for_retry(messages: &[Message]) -> Option<Vec<Message>> {
    if messages.len() < 2 {
        return None;
    }

    let drop_count = (messages.len() / 5).max(1);
    if drop_count >= messages.len() {
        return None;
    }

    let remaining = &messages[drop_count..];
    let mut result = Vec::with_capacity(remaining.len() + 1);

    // Ensure the first message is User role for API compatibility
    if remaining.first().map(|m| m.role) != Some(Role::User) {
        result.push(Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "[earlier conversation truncated for compaction retry]".to_string(),
            }],
        ));
    }

    result.extend_from_slice(remaining);
    Some(result)
}

/// Check if a message is a compact boundary marker.
pub fn is_compact_boundary(message: &Message) -> bool {
    message.content.iter().any(|block| {
        if let ContentBlock::Text { text } = block {
            text.starts_with(BOUNDARY_PREFIX)
        } else {
            false
        }
    })
}

/// Extract [`CompactMetadata`] from a boundary marker message.
pub fn extract_compact_metadata(message: &Message) -> Option<CompactMetadata> {
    for block in &message.content {
        if let ContentBlock::Text { text } = block
            && let Some(json_str) = text.strip_prefix(BOUNDARY_PREFIX)
        {
            let json_str = json_str.trim_start_matches('\n');
            return serde_json::from_str(json_str).ok();
        }
    }
    None
}

#[cfg(test)]
#[path = "auto_test.rs"]
mod auto_test;

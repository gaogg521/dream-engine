//! Recovery for a model that degrades silently when the context gets long.
//!
//! The overflow path in `engine.rs` handles a provider that *rejects* a
//! prompt. Some gateways never do: they accept a 300k-token prompt and the
//! model behind them quietly stops working — empty replies, requests that run
//! into the timeout, or turns that promise an action and never issue the tool
//! call. Nothing in the response says "too long", and because the window is
//! assumed to be large (500k by default, or whatever the gateway advertises), autocompact fires too late, so the
//! session stays in that state indefinitely.
//!
//! The only evidence available is the failure itself together with how large
//! the prompt was. That is enough to act on: a failure of this kind at a large
//! context is treated like an overflow whose limit is the size just sent.

use dream_engine_providers::error::ProviderError;
use tracing::{info, warn};

use super::{AgentEngine, CompactOutcome};
use crate::error::AgentError;

/// Context size below which an empty reply or a timeout says nothing about the
/// context. Small prompts fail for unrelated reasons (a gateway warming up,
/// a rate limit), and compacting them would only throw history away.
const DEGRADED_CONTEXT_MIN_TOKENS: u64 = 128_000;

/// What the model did that suggests the context is too long for it.
#[derive(Clone, Copy, Debug)]
pub(super) enum DegradedSignal {
    /// The turn ended with no visible text and no tool call.
    EmptyReply,
    /// The request ran into the client timeout before the model answered.
    RequestTimeout,
}

impl DegradedSignal {
    fn as_str(self) -> &'static str {
        match self {
            Self::EmptyReply => "empty_reply",
            Self::RequestTimeout => "request_timeout",
        }
    }
}

/// Whether a failed request died waiting for the model rather than being
/// refused by it.
pub(super) fn is_request_timeout(error: &AgentError) -> bool {
    match error {
        AgentError::Provider(ProviderError::Http(source)) => source.is_timeout(),
        AgentError::Provider(ProviderError::Connection(message)) => message.to_ascii_lowercase().contains("timed out"),
        _ => false,
    }
}

impl AgentEngine {
    /// Whether a failure just now should be read as "this model cannot handle
    /// a context this size". At most once per run: a second failure after a
    /// compaction means the cause is something else.
    pub(super) fn context_degradation_suspected(&self) -> bool {
        !self.degraded_recovered && self.compact_state.last_input_tokens >= DEGRADED_CONTEXT_MIN_TOKENS
    }

    /// Whether the context is big enough that a missing reply plausibly means
    /// the model cannot cope with it, whether or not recovery was attempted.
    pub(super) fn context_is_large(&self) -> bool {
        self.compact_state.last_input_tokens >= DEGRADED_CONTEXT_MIN_TOKENS
    }

    /// The text that stands in for the reply the model never produced.
    ///
    /// This is the safety net behind automatic recovery. When compaction was
    /// not attempted, failed, or did not help, the user is left with a session
    /// that answers nothing; the one thing they can do is compact by hand, so
    /// say exactly that instead of a bare "no visible answer".
    pub(super) fn suspected_overlimit_message(&self) -> String {
        format!(
            "No reply was produced. This conversation (about {} tokens) is probably too long for the model. Automatic compaction did not resolve it or has not triggered yet: run /compact to compact it manually, then try again.",
            self.compact_state.last_input_tokens
        )
    }

    /// Tell the user a missing reply is probably down to the context length.
    pub(super) fn emit_suspected_overlimit(&self) {
        self.output.emit_info_coded(
            "CONTEXT_SUSPECTED_OVERLIMIT",
            serde_json::json!({ "tokens": self.compact_state.last_input_tokens }),
            &self.suspected_overlimit_message(),
        );
    }

    /// Narrow the assumed window to the size that just failed and compact.
    /// Returns `true` when the history was actually shortened, so the caller
    /// knows a retry has a chance of behaving differently.
    ///
    /// Narrowing lasts for the session only, for the same reason as
    /// `recover_from_context_overflow`: it is inferred from a failure, and
    /// persisting a wrong value would mis-scale the compaction threshold for
    /// every later conversation with that model.
    pub(super) async fn recover_from_degraded_context(&mut self, signal: DegradedSignal) -> bool {
        if !self.context_degradation_suspected() {
            return false;
        }
        self.degraded_recovered = true;

        let sent = self.compact_state.last_input_tokens;
        warn!(
            target: "dream_engine_agent",
            signal = signal.as_str(),
            context_tokens = sent,
            context_window = self.compact_config.context_window,
            "model failed at a large context; treating it as a context limit and compacting"
        );

        match self.compact_now().await {
            CompactOutcome::Compacted => {
                // Narrow only once compaction has actually freed the room. Done
                // first, a failed compaction would leave a window smaller than
                // the context in hand, and the engine's own hard limit would
                // then refuse every further request with "context nearly full".
                let learned = sent as usize;
                if learned < self.compact_config.context_window {
                    info!(
                        target: "dream_engine_agent",
                        previous = self.compact_config.context_window,
                        learned,
                        "narrowing context window after a degraded response"
                    );
                    self.compact_config.context_window = learned;
                }

                self.output.emit_info_coded(
                    "CONTEXT_DEGRADED_COMPACT",
                    serde_json::json!({ "tokens": sent }),
                    &format!(
                        "The model stopped responding properly at about {sent} tokens of context. \
                         The conversation was compacted automatically and the request is being retried. \
                         If this keeps happening, set a smaller context window for this model in its settings."
                    ),
                );
                true
            }
            CompactOutcome::Failed(error) => {
                self.output.emit_info_coded(
                    "AUTOCOMPACT_FAILED",
                    serde_json::json!({}),
                    &format!("Autocompact failed ({error}). Use /compact or start a new conversation."),
                );
                false
            }
            CompactOutcome::CircuitBroken => false,
        }
    }
}

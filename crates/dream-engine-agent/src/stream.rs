use dream_engine_types::message::{ContentBlock, StopReason, TokenUsage};

/// Everything a single provider stream produces in one model turn.
///
/// Collected by `AgentEngine::consume_stream` so the main loop deals with a
/// single named value instead of six mutable locals.
pub(crate) struct StreamOutcome {
    pub(crate) assistant_text: String,
    pub(crate) thinking_text: String,
    pub(crate) thinking_signature: Option<String>,
    pub(crate) provider_items: Vec<ContentBlock>,
    pub(crate) tool_calls: Vec<ContentBlock>,
    /// Tool calls whose arguments were still streaming when the output
    /// limit cut the response off — `(tool_use_id, tool_name)`. Never
    /// executed; only used to surface the truncation and retry with tools
    /// enabled instead of silently dropping the attempt.
    pub(crate) truncated_tool_calls: Vec<(String, String)>,
    pub(crate) stop_reason: StopReason,
    pub(crate) usage: TokenUsage,
}

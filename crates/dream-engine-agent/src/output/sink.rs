use dream_engine_types::message::TokenUsage;

/// Abstraction over output channels (terminal vs JSON stream protocol)
pub trait OutputSink: Send + Sync {
    /// Stream text delta from LLM
    fn emit_text_delta(&self, text: &str, msg_id: &str);

    /// Stream thinking content from LLM
    fn emit_thinking(&self, text: &str, msg_id: &str);

    /// Announce a tool call.
    fn emit_tool_call(&self, tool_use_id: &str, name: &str, input: &str);

    /// Display tool result.
    fn emit_tool_result(&self, tool_use_id: &str, name: &str, is_error: bool, content: &str);

    /// Signal start of a new message stream
    fn emit_stream_start(&self, msg_id: &str);

    /// Signal end of a message stream with usage stats
    fn emit_stream_end(
        &self,
        msg_id: &str,
        turns: usize,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
    );

    /// Report the token usage of a model call a *tool* made on its own behalf
    /// (today: `ReadImage`'s vision delegate).
    ///
    /// Separate from [`Self::emit_stream_end`] because it is a different model
    /// than the session's, with its own rate — folding it into the turn total
    /// would bill the session model for tokens it never spent.
    ///
    /// Defaults to dropping it: the terminal and JSON-protocol sinks have
    /// nowhere to put it, and only an embedding host that meters spend cares.
    fn emit_delegate_usage(&self, _model: &str, _usage: &TokenUsage) {}

    /// Display error
    fn emit_error(&self, msg: &str);

    /// Display informational message
    fn emit_info(&self, msg: &str);
}

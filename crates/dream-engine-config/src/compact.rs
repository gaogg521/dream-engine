use serde::{Deserialize, Serialize};

/// Configuration for the multi-level context compaction system.
///
/// All token-related fields are in tokens (not bytes or characters).
///
/// The defaults assume a large modern context window (see
/// [`default_context_window`]) and a percentage-based compaction trigger, so
/// they stay sane at any window size. A model whose real window is smaller must
/// declare it — `context_window` is the number every threshold here is derived
/// from, and nothing else can discover it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactConfig {
    /// Context window size in tokens (e.g. 1_000_000 for Gemini, 200_000 for Claude).
    #[serde(default = "default_context_window")]
    pub context_window: usize,

    /// Tokens reserved for output generation.
    /// Subtracted from `context_window` to get the effective input budget.
    #[serde(default = "default_output_reserve")]
    pub output_reserve: usize,

    /// Buffer below the effective window that triggers autocompact.
    /// `threshold = context_window - output_reserve - autocompact_buffer`
    ///
    /// Only consulted when `autocompact_threshold_pct` is explicitly cleared;
    /// the default is percentage-based, because this field and `output_reserve`
    /// are absolute token counts and therefore only correct near the one window
    /// size they were tuned for.
    #[serde(default = "default_autocompact_buffer")]
    pub autocompact_buffer: usize,

    /// Tokens from context_window limit to trigger emergency block.
    /// `emergency_limit = context_window - emergency_buffer`
    ///
    /// Capped so the block always stays above the autocompact trigger — see
    /// `compact::emergency::emergency_limit`. At a small window this fixed
    /// count would otherwise land *below* the trigger and refuse the turn
    /// before compaction ever ran.
    #[serde(default = "default_emergency_buffer")]
    pub emergency_buffer: usize,

    /// Max consecutive autocompact failures before the circuit breaker trips.
    #[serde(default = "default_max_failures")]
    pub max_failures: u32,

    /// Maximum UTF-8 byte length of one model-facing tool result.
    ///
    /// Oversized results are truncated once, preserving their beginning and
    /// end. This keeps stored history stable for prompt caching.
    #[serde(default = "default_tool_output_max_bytes")]
    pub tool_output_max_bytes: usize,

    /// Whether to enable the legacy history-rewriting microcompact pass.
    ///
    /// Disabled by default because rewriting old tool results invalidates the
    /// stable prompt prefix. Prefer the per-result output limit instead.
    #[serde(default)]
    pub microcompact_enabled: bool,

    /// Legacy microcompact: keep the N most recent compactable tool results.
    #[serde(default = "default_micro_keep_recent")]
    pub micro_keep_recent: usize,

    /// Legacy microcompact: gap threshold in seconds for time-based trigger.
    /// When the last assistant message is older than this, microcompact fires.
    #[serde(default = "default_micro_gap_seconds")]
    pub micro_gap_seconds: u64,

    /// Tool names whose results are eligible for microcompact content clearing.
    #[serde(default = "default_compactable_tools")]
    pub compactable_tools: Vec<String>,

    /// Percentage of the context window at which autocompact runs.
    /// `threshold = context_window * pct / 100`, ignoring `output_reserve` and
    /// `autocompact_buffer`.
    ///
    /// This is the default mode (see [`default_autocompact_threshold_pct`]).
    /// `None` selects the absolute-buffer formula instead; TOML cannot express
    /// null, so that mode is only reachable programmatically (an embedding host
    /// or a test), which is deliberate — the absolute buffers are only correct
    /// near the one window size they were tuned for.
    #[serde(default = "default_autocompact_threshold_pct")]
    pub autocompact_threshold_pct: Option<u8>,

    /// Whether the compaction system is enabled.
    /// When false, legacy microcompact and autocompact are skipped
    /// (emergency truncation still applies).
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Enable prompt cache diagnostics output to user.
    /// When true, cache hit/miss info is shown via OutputSink.
    /// Default: false.
    #[serde(default)]
    pub cache_diagnostics: bool,

    #[serde(default)]
    pub compaction: dream_engine_compact::CompactLevel,

    #[serde(default)]
    pub toon: bool,
}

impl Default for CompactConfig {
    fn default() -> Self {
        Self {
            context_window: default_context_window(),
            output_reserve: default_output_reserve(),
            autocompact_buffer: default_autocompact_buffer(),
            emergency_buffer: default_emergency_buffer(),
            max_failures: default_max_failures(),
            tool_output_max_bytes: default_tool_output_max_bytes(),
            microcompact_enabled: false,
            micro_keep_recent: default_micro_keep_recent(),
            micro_gap_seconds: default_micro_gap_seconds(),
            compactable_tools: default_compactable_tools(),
            autocompact_threshold_pct: default_autocompact_threshold_pct(),
            enabled: default_true(),
            cache_diagnostics: false,
            compaction: dream_engine_compact::CompactLevel::default(),
            toon: false,
        }
    }
}

// --- Default value functions ---

/// Context window assumed when nothing declares one.
///
/// Was 200k, which is Claude's window and was simply wrong for everything else:
/// a session on a large-window model stopped at 197k tokens with "context
/// window nearly full" while the model still had most of its window free.
///
/// This is a default, not a measurement. A model whose real window is smaller
/// must declare it (`[compact] context_window`, or the host's own per-model
/// setting) — otherwise the first thing to notice the shortfall is the
/// provider, and its prompt-too-long error is far less useful than the block
/// this number drives.
fn default_context_window() -> usize {
    1_000_000
}
fn default_output_reserve() -> usize {
    20_000
}
fn default_autocompact_buffer() -> usize {
    13_000
}
fn default_emergency_buffer() -> usize {
    3_000
}
/// Autocompact trigger as a share of the window.
///
/// A percentage rather than `window - output_reserve - autocompact_buffer`
/// because those are absolute counts tuned for a 200k window: at 1M they put
/// the trigger at ~96.7%, within one large tool result of the emergency block,
/// and at an 8k local window they underflow to zero and compact every turn.
fn default_autocompact_threshold_pct() -> Option<u8> {
    Some(80)
}

fn default_max_failures() -> u32 {
    3
}
fn default_tool_output_max_bytes() -> usize {
    10_000
}
fn default_micro_keep_recent() -> usize {
    5
}
fn default_micro_gap_seconds() -> u64 {
    3600
}
fn default_compactable_tools() -> Vec<String> {
    vec![
        "Read".into(),
        "ExecCommand".into(),
        "Grep".into(),
        "Glob".into(),
        "Write".into(),
        "Edit".into(),
    ]
}
fn default_true() -> bool {
    true
}

#[cfg(test)]
#[path = "compact_test.rs"]
mod compact_test;

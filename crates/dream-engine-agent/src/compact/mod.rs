//! Context compaction guards for long conversations.
//!
//! Model-facing tool results are bounded once before entering history. The
//! remaining guards, from lightest to heaviest, are:
//! - **Legacy microcompact**: optionally clears old tool result content
//! - **Autocompact**: context-threshold-triggered LLM summarization
//! - **Emergency**: blocks API calls when near the context window limit

pub mod auto;
pub mod emergency;
pub mod estimate;
pub mod micro;
pub mod prompt;
pub mod state;

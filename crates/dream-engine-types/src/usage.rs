//! Reporting token usage from a *delegated* model call.
//!
//! A tool may call a model of its own — `ReadImage` hands the image to a
//! separate vision model — and that call costs real tokens that the turn's own
//! `Done { usage }` never accounts for. Left unreported, the spend is invisible
//! to whatever is metering the session.
//!
//! The trait lives in `dream-types` because `dream-tools` needs it and cannot
//! depend on `dream-agent` (that would be a cycle). `dream-agent` bridges it to
//! whatever `OutputSink` the embedding host installed.

use crate::message::TokenUsage;

/// Receives the token usage of a model call a tool made on its own behalf.
///
/// Implementations must not block or fail the tool: metering is best-effort,
/// and a tool result must never be lost because the meter was unavailable.
pub trait DelegateUsageSink: Send + Sync {
    /// `model` is the model that was actually billed — the delegate's, never
    /// the session model's. Attributing it to the session model would report a
    /// cost against a model that was never called.
    fn on_delegate_usage(&self, model: &str, usage: &TokenUsage);
}

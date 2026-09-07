//! A veto the host can install over every tool call, whatever the approval mode.
//!
//! # Why this is separate from `ToolApprovalManager`
//!
//! Approval answers "should a human be asked about this?", and the answer is
//! legitimately "no" in `yolo` mode — that is the whole point of the mode. But
//! a host embedding this engine may be subject to a rule it did not choose and
//! cannot let the operator switch off: One Work's desktop client runs an
//! enterprise member's turns, and their company's blocked-command list has to
//! hold even when the member has put the session in full-auto.
//!
//! Routing that through the approval manager would have meant either lying
//! about the mode (dropping the session out of `yolo` so requests surface, then
//! silently answering them) or making `yolo` mean something different for some
//! callers. Both make the mode indicator wrong. So the policy is a separate,
//! earlier question: it runs before approval is even considered, and a refusal
//! is reported as a failed tool call rather than as a denied request — the
//! model sees why and can say so, and no human is prompted for a decision that
//! was never theirs to make.
//!
//! No gate installed (the default, and the only case for a standalone CLI user)
//! means every call proceeds exactly as before.

use async_trait::async_trait;
use serde_json::Value;

/// Consulted immediately before a tool executes.
///
/// Async because the two implementations One Work ships differ: a desktop
/// member's policy is a lock read over an already-synced copy, but the same
/// engine runs on the company server, where the answer comes from the
/// database. A sync signature would have forced that one to block a runtime
/// thread. Every tool call in a turn pays for this, so an implementation
/// should still resolve without I/O wherever it can.
#[async_trait]
pub trait ToolCallGuard: Send + Sync {
    /// `Some(reason)` refuses the call and surfaces `reason` to the model;
    /// `None` lets it run.
    ///
    /// `reason` is shown to the user as the tool's output, so it should say
    /// what was refused and by what — not just "denied".
    async fn check(&self, tool_name: &str, input: &Value) -> Option<String>;
}

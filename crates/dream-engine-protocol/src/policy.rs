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

use serde_json::Value;

/// Consulted immediately before a tool executes.
///
/// Synchronous on purpose: an implementation is expected to be a lock read over
/// an already-resolved policy, and every tool call in a turn pays for it.
/// Anything that needs to await belongs in a hook, not here.
pub trait ToolPolicyGate: Send + Sync {
    /// `Some(reason)` refuses the call and surfaces `reason` to the model;
    /// `None` lets it run.
    ///
    /// `reason` is shown to the user as the tool's output, so it should say
    /// what was refused and by what — not just "denied".
    fn check(&self, tool_name: &str, input: &Value) -> Option<String>;
}

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// One option of a structured question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One structured question the agent asks the user.
///
/// The shape follows the AskUserQuestion layout hosts already render
/// (question / header / options / multiSelect), so a host can reuse the same
/// question card for every agent that asks this way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestion {
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub options: Vec<AskOption>,
    #[serde(default, rename = "multiSelect", alias = "multi_select")]
    pub multi_select: bool,
}

/// The user's answer to one question, keyed by the question text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskAnswer {
    pub question: String,
    #[serde(default)]
    pub labels: Vec<String>,
}

/// How a pending question was settled by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskUserOutcome {
    Answered(Vec<AskAnswer>),
    /// The user dismissed the question without answering.
    Declined,
}

/// Pending structured questions, each waiting on a oneshot for the host's answer.
///
/// Separate from [`ToolApprovalManager`](crate::ToolApprovalManager): an
/// approval decides whether a tool may run, while a question is the tool's
/// actual payload. Session modes that auto-approve tools must never
/// auto-answer a question.
#[derive(Default)]
pub struct AskUserManager {
    pending: Mutex<HashMap<String, oneshot::Sender<AskUserOutcome>>>,
}

impl AskUserManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a pending question. Must be called before the question is
    /// emitted, so an answer can never arrive for an unknown id.
    pub fn request(&self, request_id: &str) -> oneshot::Receiver<AskUserOutcome> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(request_id.to_string(), tx);
        }
        rx
    }

    /// Settle a pending question. Returns `false` when no live question has
    /// this id (already answered, or the turn that asked it was stopped).
    pub fn resolve(&self, request_id: &str, outcome: AskUserOutcome) -> bool {
        let Some(tx) = self
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(request_id))
        else {
            return false;
        };
        tx.send(outcome).is_ok()
    }

    /// Whether a question with this id is still waiting for an answer.
    pub fn is_pending(&self, request_id: &str) -> bool {
        self.pending
            .lock()
            .map(|pending| pending.get(request_id).is_some_and(|tx| !tx.is_closed()))
            .unwrap_or(false)
    }

    /// Forget a question whose asker gave up waiting.
    pub fn drop_pending(&self, request_id: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(request_id);
        }
    }
}

#[cfg(test)]
#[path = "ask_test.rs"]
mod ask_test;

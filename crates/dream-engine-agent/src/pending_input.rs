//! User input that arrives while a run is still in progress.
//!
//! A host that embeds the engine holds the engine for the whole of `run()`,
//! so a message the user sends mid-run cannot go through `run()` itself. It is
//! pushed here instead, and the engine folds it into the conversation at the
//! next step boundary: after a tool round, before the next model request, or —
//! if the model was about to finish — by taking one more turn to answer it.
//!
//! The inbox is *gated*. It only accepts input while the host has it open,
//! which the host does for exactly the span of a run. Closing is atomic with
//! the emptiness check ([`PendingInput::close_if_empty`]), so a message can
//! never land after the engine's last look and before the host declares the
//! run over: either the push wins and the host sees it, or the close wins and
//! the push is refused — and a refused push is the caller's cue to start a new
//! run instead.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use dream_engine_types::message::ContentBlock;

#[derive(Default)]
struct State {
    open: bool,
    queue: VecDeque<Vec<ContentBlock>>,
}

/// Cloneable handle to one engine's mid-run input inbox.
#[derive(Clone, Default)]
pub struct PendingInput {
    state: Arc<Mutex<State>>,
}

impl PendingInput {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A poisoned lock only means another holder panicked mid-push; the
        // queue itself is still a valid VecDeque, so keep using it.
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Start accepting input. Called by the host when a run begins.
    pub fn open(&self) {
        self.lock().open = true;
    }

    /// Queue one user message for the running run.
    ///
    /// Returns `false` (and keeps nothing) when the inbox is closed — no run
    /// is in progress, so the caller must deliver the message as a new run.
    pub fn push(&self, blocks: Vec<ContentBlock>) -> bool {
        let mut state = self.lock();
        if !state.open {
            return false;
        }
        state.queue.push_back(blocks);
        true
    }

    /// Take everything queued so far, oldest first.
    pub fn drain(&self) -> Vec<Vec<ContentBlock>> {
        self.lock().queue.drain(..).collect()
    }

    /// Whether anything is waiting.
    pub fn is_empty(&self) -> bool {
        self.lock().queue.is_empty()
    }

    /// Stop accepting input, but only if nothing is waiting.
    ///
    /// Returns `true` when the inbox is now closed. `false` means input
    /// arrived after the engine's last look; the inbox stays open and the
    /// host must run again to answer it.
    pub fn close_if_empty(&self) -> bool {
        let mut state = self.lock();
        if state.queue.is_empty() {
            state.open = false;
            true
        } else {
            false
        }
    }

    /// Stop accepting input unconditionally and hand back whatever was left.
    ///
    /// For runs that end without a chance to answer — cancelled, failed —
    /// where the leftovers still have to go somewhere rather than vanish.
    pub fn close_and_drain(&self) -> Vec<Vec<ContentBlock>> {
        let mut state = self.lock();
        state.open = false;
        state.queue.drain(..).collect()
    }
}

#[cfg(test)]
#[path = "pending_input_test.rs"]
mod pending_input_test;

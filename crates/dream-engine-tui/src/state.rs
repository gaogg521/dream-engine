use std::collections::{HashMap, HashSet};

use dream_engine_agent::commands::CommandSpec;
use dream_engine_types::message::{Message, TokenUsage};

use crate::app_command::application_command_specs;
use crate::command_popup::CommandPopup;
use crate::composer::Composer;
use crate::event::AgentEvent;
use crate::session_picker::SessionPicker;
use crate::transcript::{EntryKind, ToolStepStatus, TranscriptEntry, entries_from_messages};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApprovalChoice {
    Once,
    Always,
    Deny,
}

impl ApprovalChoice {
    pub(super) fn previous(self) -> Self {
        match self {
            Self::Once => Self::Deny,
            Self::Always => Self::Once,
            Self::Deny => Self::Always,
        }
    }

    pub(super) fn next(self) -> Self {
        match self {
            Self::Once => Self::Always,
            Self::Always => Self::Deny,
            Self::Deny => Self::Once,
        }
    }
}

#[derive(Debug)]
pub(super) struct ApprovalRequest {
    pub(super) call_id: String,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) input: String,
    pub(super) choice: ApprovalChoice,
}

#[derive(Debug)]
pub(super) struct AppState {
    pub(super) model: String,
    pub(super) provider: String,
    pub(super) cwd: String,
    pub(super) session_id: Option<String>,
    pub(super) no_color: bool,
    pub(super) composer: Composer,
    pub(super) popup: CommandPopup,
    pub(super) session_picker: SessionPicker,
    pub(super) transcript: Vec<TranscriptEntry>,
    pub(super) committed_transcript: usize,
    pub(super) show_welcome: bool,
    pub(super) approval: Option<ApprovalRequest>,
    pub(super) initializing: bool,
    pub(super) busy: bool,
    pub(super) spinner_frame: usize,
    pub(super) usage: TokenUsage,
    pub(super) turns: usize,
    active_assistant: Option<usize>,
    active_thinking: Option<usize>,
    active_tools: HashMap<String, usize>,
    protocol_results: HashSet<String>,
}

impl AppState {
    pub(super) fn new(model: String, provider: String, cwd: String, no_color: bool) -> Self {
        Self {
            model,
            provider,
            cwd,
            session_id: None,
            no_color,
            composer: Composer::default(),
            popup: CommandPopup::default(),
            session_picker: SessionPicker::default(),
            transcript: Vec::new(),
            committed_transcript: 0,
            show_welcome: true,
            approval: None,
            initializing: false,
            busy: false,
            spinner_frame: 0,
            usage: TokenUsage::default(),
            turns: 0,
            active_assistant: None,
            active_thinking: None,
            active_tools: HashMap::new(),
            protocol_results: HashSet::new(),
        }
    }

    pub(super) fn set_commands(&mut self, commands: Vec<CommandSpec>) {
        let mut commands = commands;
        commands.extend(application_command_specs());
        commands.sort_by(|left, right| left.name.cmp(&right.name));
        commands.dedup_by(|left, right| left.name == right.name);
        self.popup.set_commands(commands);
    }

    pub(super) fn set_history(&mut self, messages: &[Message]) {
        self.transcript = entries_from_messages(messages);
        self.committed_transcript = 0;
        self.show_welcome = messages.is_empty();
        self.active_assistant = None;
        self.active_thinking = None;
        self.active_tools.clear();
    }

    pub(super) fn begin_initialization(&mut self) {
        self.initializing = true;
    }

    pub(super) fn reset_session(
        &mut self,
        model: String,
        provider: String,
        session_id: Option<String>,
        messages: &[Message],
    ) {
        self.model = model;
        self.provider = provider;
        self.session_id = session_id;
        self.set_history(messages);
        self.approval = None;
        self.initializing = false;
        self.busy = false;
        self.spinner_frame = 0;
        self.usage = TokenUsage::default();
        self.turns = 0;
        self.composer.clear();
        self.popup.update("");
        self.session_picker.close();
        self.active_tools.clear();
        self.protocol_results.clear();
    }

    pub(super) fn pending_transcript(&self) -> &[TranscriptEntry] {
        &self.transcript[self.committed_transcript.min(self.transcript.len())..]
    }

    pub(super) fn mark_transcript_committed(&mut self) {
        self.committed_transcript = self.transcript.len();
    }

    pub(super) fn prepare_history_replay(&mut self) -> usize {
        self.committed_transcript = 0;
        for entry in &mut self.transcript {
            entry.reset_display_offset();
        }
        self.show_welcome = self.transcript.is_empty();

        let first_unstable_tool = self.transcript.iter().position(|entry| !entry.is_stable_for_history());
        [self.active_assistant, self.active_thinking, first_unstable_tool]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(self.transcript.len())
    }

    pub(super) fn mark_transcript_prefix_committed(&mut self, count: usize) {
        self.committed_transcript = count.min(self.transcript.len());
    }

    pub(super) fn commit_streaming_prefix(&mut self, complete_entries: usize, active_byte_count: usize) {
        let pending_count = self.pending_transcript().len();
        self.committed_transcript = self
            .committed_transcript
            .saturating_add(complete_entries.min(pending_count));
        if active_byte_count > 0
            && let Some(active) = self.transcript.get_mut(self.committed_transcript)
        {
            active.advance_display_offset(active_byte_count);
        }
    }

    pub(super) fn can_commit_streaming_lines(&self) -> bool {
        self.busy
            && self
                .pending_transcript()
                .last()
                .is_some_and(|entry| matches!(entry.kind, EntryKind::Assistant | EntryKind::Thinking))
    }

    pub(super) fn push_info(&mut self, label: &str, text: impl Into<String>) {
        self.transcript.push(TranscriptEntry::new(EntryKind::Info, label, text));
    }

    pub(super) fn push_error(&mut self, text: impl Into<String>) {
        self.transcript
            .push(TranscriptEntry::new(EntryKind::Error, "Error", text));
    }

    pub(super) fn begin_turn(&mut self, input: &str) {
        self.busy = true;
        self.active_assistant = None;
        self.active_thinking = None;
        if !self.popup.recognizes(input) {
            self.transcript.push(TranscriptEntry::new(EntryKind::User, "", input));
        }
    }

    pub(super) fn finish_turn(&mut self, turns: usize, usage: TokenUsage) {
        self.busy = false;
        self.turns = turns;
        self.usage = usage;
        self.active_assistant = None;
        self.active_thinking = None;
    }

    pub(super) fn cancel_turn(&mut self) {
        self.busy = false;
        self.active_assistant = None;
        self.active_thinking = None;
        self.transcript.push(TranscriptEntry::new(
            EntryKind::Info,
            "Stopped",
            "Turn cancelled by user",
        ));
    }

    pub(super) fn tick(&mut self) {
        if self.busy {
            self.spinner_frame = (self.spinner_frame + 1) % 4;
        }
    }

    pub(super) fn handle_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::StreamStart => {
                self.active_assistant = None;
                self.active_thinking = None;
            }
            AgentEvent::TextDelta(text) => self.append_stream(EntryKind::Assistant, "", text),
            AgentEvent::Thinking(text) => self.append_stream(EntryKind::Thinking, "Thinking", text),
            AgentEvent::Info(text) => self
                .transcript
                .push(TranscriptEntry::new(EntryKind::Info, "Info", text)),
            AgentEvent::Error(text) => self
                .transcript
                .push(TranscriptEntry::new(EntryKind::Error, "Error", text)),
            AgentEvent::ToolCall { call_id, name, input } => {
                self.update_tool_step(&call_id, &name, ToolStepStatus::Queued, Some(input))
            }
            AgentEvent::ToolResult {
                call_id,
                name,
                is_error,
                content,
            } => {
                if self.protocol_results.remove(&call_id) {
                    return;
                }
                let status = if is_error {
                    ToolStepStatus::Error
                } else {
                    ToolStepStatus::Success
                };
                self.update_tool_step(&call_id, &name, status, Some(content));
            }
            AgentEvent::ProtocolToolResult {
                call_id,
                name,
                is_error,
                content,
            } => {
                let status = if is_error {
                    ToolStepStatus::Error
                } else {
                    ToolStepStatus::Success
                };
                self.update_tool_step(&call_id, &name, status, Some(content));
                self.protocol_results.insert(call_id);
            }
            AgentEvent::ApprovalRequested {
                call_id,
                name,
                description,
                input,
            } => {
                self.update_tool_step(&call_id, &name, ToolStepStatus::Approval, None);
                self.approval = Some(ApprovalRequest {
                    call_id,
                    name,
                    description,
                    input,
                    choice: ApprovalChoice::Once,
                });
            }
            AgentEvent::ToolRunning { call_id, name } => {
                if self.approval.as_ref().is_some_and(|request| request.call_id == call_id) {
                    self.approval = None;
                }
                self.update_tool_step(&call_id, &name, ToolStepStatus::Running, None);
            }
            AgentEvent::ToolCancelled { call_id, name, reason } => {
                if self.approval.as_ref().is_some_and(|request| request.call_id == call_id) {
                    self.approval = None;
                }
                self.update_tool_step(&call_id, &name, ToolStepStatus::Cancelled, Some(reason));
            }
        }
    }

    fn update_tool_step(&mut self, call_id: &str, name: &str, status: ToolStepStatus, text: Option<String>) {
        if let Some(index) = self.active_tools.get(call_id).copied()
            && let Some(entry) = self.transcript.get_mut(index)
        {
            if name != "tool" {
                entry.label = name.to_string();
            }
            entry.tool_status = Some(status);
            if let Some(text) = text {
                entry.text = text;
            }
            return;
        }

        let index = self.transcript.len();
        self.transcript
            .push(TranscriptEntry::tool(name, text.unwrap_or_default(), status));
        self.active_tools.insert(call_id.to_string(), index);
    }

    fn append_stream(&mut self, kind: EntryKind, label: &str, text: String) {
        let active = match kind {
            EntryKind::Assistant => &mut self.active_assistant,
            EntryKind::Thinking => &mut self.active_thinking,
            _ => return,
        };
        if let Some(index) = *active {
            self.transcript[index].text.push_str(&text);
        } else {
            self.transcript.push(TranscriptEntry::new(kind, label, text));
            *active = Some(self.transcript.len() - 1);
        }
    }
}

#[cfg(test)]
#[path = "state_test.rs"]
mod state_test;

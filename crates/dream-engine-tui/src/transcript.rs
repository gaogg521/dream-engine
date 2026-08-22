use std::collections::HashMap;

use dream_engine_agent::compact::auto::is_compact_boundary;
use dream_engine_types::message::{ContentBlock, Message, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryKind {
    User,
    Assistant,
    Thinking,
    Tool,
    Info,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolStepStatus {
    Queued,
    Approval,
    Running,
    Success,
    Error,
    Cancelled,
}

impl ToolStepStatus {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Approval => "approval",
            Self::Running => "running",
            Self::Success => "done",
            Self::Error => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn is_terminal(self) -> bool {
        matches!(self, Self::Success | Self::Error | Self::Cancelled)
    }
}

#[derive(Debug)]
pub(super) struct TranscriptEntry {
    pub(super) kind: EntryKind,
    pub(super) label: String,
    pub(super) text: String,
    pub(super) tool_status: Option<ToolStepStatus>,
    display_offset: usize,
}

impl TranscriptEntry {
    pub(super) fn new(kind: EntryKind, label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind,
            label: label.into(),
            text: text.into(),
            tool_status: None,
            display_offset: 0,
        }
    }

    pub(super) fn tool(label: impl Into<String>, text: impl Into<String>, status: ToolStepStatus) -> Self {
        Self {
            kind: EntryKind::Tool,
            label: label.into(),
            text: text.into(),
            tool_status: Some(status),
            display_offset: 0,
        }
    }

    pub(super) fn visible_text(&self) -> &str {
        &self.text[self.display_offset.min(self.text.len())..]
    }

    pub(super) fn advance_display_offset(&mut self, byte_count: usize) {
        self.display_offset = self.display_offset.saturating_add(byte_count).min(self.text.len());
    }

    pub(super) fn reset_display_offset(&mut self) {
        self.display_offset = 0;
    }

    pub(super) fn is_stable_for_history(&self) -> bool {
        self.kind != EntryKind::Tool || self.tool_status.is_some_and(ToolStepStatus::is_terminal)
    }
}

pub(super) fn entries_from_messages(messages: &[Message]) -> Vec<TranscriptEntry> {
    let mut entries = Vec::new();
    let mut tool_entries = HashMap::new();
    let mut skip_compact_summary = false;
    for message in messages {
        if skip_compact_summary {
            skip_compact_summary = false;
            continue;
        }
        if is_compact_boundary(message) {
            skip_compact_summary = true;
            continue;
        }
        for block in &message.content {
            match block {
                ContentBlock::Text { text } => match message.role {
                    Role::User => entries.push(TranscriptEntry::new(EntryKind::User, "", text)),
                    Role::Assistant => entries.push(TranscriptEntry::new(EntryKind::Assistant, "", text)),
                    Role::System => entries.push(TranscriptEntry::new(EntryKind::Info, "System", text)),
                    Role::Tool => entries.push(TranscriptEntry::tool("Tool", text, ToolStepStatus::Success)),
                },
                ContentBlock::Thinking { thinking, .. } => {
                    entries.push(TranscriptEntry::new(EntryKind::Thinking, "Thinking", thinking));
                }
                ContentBlock::ToolUse { id, name, input, .. } => {
                    entries.push(TranscriptEntry::tool(name, input.to_string(), ToolStepStatus::Queued));
                    tool_entries.insert(id.clone(), entries.len() - 1);
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let status = if *is_error {
                        ToolStepStatus::Error
                    } else {
                        ToolStepStatus::Success
                    };
                    if let Some(index) = tool_entries.get(tool_use_id).copied() {
                        entries[index].text = content.clone();
                        entries[index].tool_status = Some(status);
                    } else {
                        entries.push(TranscriptEntry::tool("Tool result", content, status));
                    }
                }
                ContentBlock::Image { .. } => {
                    entries.push(TranscriptEntry::new(EntryKind::Info, "Image", "[attached image]"));
                }
                ContentBlock::ProviderItem { .. } => {}
            }
        }
    }
    entries
}

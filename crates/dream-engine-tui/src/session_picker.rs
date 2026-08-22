#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiSession {
    id: String,
    model: String,
    summary: String,
    updated_at: String,
    message_count: usize,
}

impl TuiSession {
    pub fn new(id: String, model: String, summary: String, updated_at: String, message_count: usize) -> Self {
        Self {
            id,
            model,
            summary,
            updated_at,
            message_count,
        }
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn model(&self) -> &str {
        &self.model
    }

    pub(crate) fn summary(&self) -> &str {
        &self.summary
    }

    pub(crate) fn updated_at(&self) -> &str {
        &self.updated_at
    }

    pub(crate) fn message_count(&self) -> usize {
        self.message_count
    }
}

#[derive(Debug, Default)]
pub(super) struct SessionPicker {
    sessions: Vec<TuiSession>,
    selected: usize,
    visible: bool,
}

impl SessionPicker {
    pub(super) fn set_sessions(&mut self, sessions: Vec<TuiSession>) {
        self.sessions = sessions;
        self.selected = 0;
        if self.sessions.is_empty() {
            self.visible = false;
        }
    }

    pub(super) fn open(&mut self) -> bool {
        self.selected = 0;
        self.visible = !self.sessions.is_empty();
        self.visible
    }

    pub(super) fn close(&mut self) {
        self.visible = false;
    }

    pub(super) fn is_visible(&self) -> bool {
        self.visible
    }

    pub(super) fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub(super) fn sessions(&self) -> &[TuiSession] {
        &self.sessions
    }

    pub(super) fn selected(&self) -> usize {
        self.selected
    }

    pub(super) fn selected_id(&self) -> Option<String> {
        self.sessions.get(self.selected).map(|session| session.id.clone())
    }

    pub(super) fn move_next(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = (self.selected + 1) % self.sessions.len();
        }
    }

    pub(super) fn move_previous(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = self.selected.checked_sub(1).unwrap_or(self.sessions.len() - 1);
        }
    }
}

#[cfg(test)]
#[path = "session_picker_test.rs"]
mod session_picker_test;

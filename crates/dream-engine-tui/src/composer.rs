use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Default)]
pub(super) struct Composer {
    text: Vec<char>,
    cursor: usize,
    history: Vec<String>,
    history_index: Option<usize>,
}

impl Composer {
    pub(super) fn text(&self) -> String {
        self.text.iter().collect()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub(super) fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.history_index = None;
    }

    pub(super) fn take(&mut self) -> String {
        let value = self.text();
        if !value.trim().is_empty() {
            self.history.push(value.clone());
        }
        self.clear();
        value
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        for character in text.chars() {
            self.text.insert(self.cursor, character);
            self.cursor += 1;
        }
        self.history_index = None;
    }

    pub(super) fn input(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.insert(self.cursor, character);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.text.remove(self.cursor);
            }
            KeyCode::Delete if self.cursor < self.text.len() => {
                self.text.remove(self.cursor);
            }
            KeyCode::Left if self.cursor > 0 => self.cursor -= 1,
            KeyCode::Right if self.cursor < self.text.len() => self.cursor += 1,
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::End => self.cursor = self.line_end(),
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.text.insert(self.cursor, '\n');
                self.cursor += 1;
            }
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.insert(self.cursor, '\n');
                self.cursor += 1;
            }
            KeyCode::Up => self.recall_older(),
            KeyCode::Down => self.recall_newer(),
            _ => return false,
        }
        true
    }

    pub(super) fn replace_command(&mut self, name: &str) {
        self.text = format!("/{name}").chars().collect();
        self.cursor = self.text.len();
        self.history_index = None;
    }

    pub(super) fn visual_cursor(&self, width: u16) -> (u16, u16) {
        let usable_width = width.max(1) as usize;
        let mut row = 0usize;
        let mut column = 0usize;
        for character in self.text.iter().take(self.cursor) {
            if *character == '\n' {
                row += 1;
                column = 0;
                continue;
            }
            let character_width = character.width().unwrap_or(0).max(1);
            if column + character_width > usable_width {
                row += 1;
                column = 0;
            }
            column += character_width;
            if column >= usable_width {
                row += 1;
                column = 0;
            }
        }
        (column as u16, row as u16)
    }

    pub(super) fn visual_height(&self, width: u16, max_height: u16) -> u16 {
        let (_, row) = self.visual_end(width);
        (row + 1).clamp(1, max_height)
    }

    fn visual_end(&self, width: u16) -> (u16, u16) {
        let usable_width = width.max(1) as usize;
        let mut row = 0usize;
        let mut column = 0usize;
        for character in &self.text {
            if *character == '\n' {
                row += 1;
                column = 0;
                continue;
            }
            let character_width = character.width().unwrap_or(0).max(1);
            if column + character_width > usable_width {
                row += 1;
                column = 0;
            }
            column += character_width;
            if column >= usable_width {
                row += 1;
                column = 0;
            }
        }
        (column as u16, row as u16)
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor]
            .iter()
            .rposition(|character| *character == '\n')
            .map_or(0, |index| index + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .iter()
            .position(|character| *character == '\n')
            .map_or(self.text.len(), |index| self.cursor + index)
    }

    fn recall_older(&mut self) {
        if self.history.is_empty() || self.text.contains(&'\n') {
            return;
        }
        let index = match self.history_index {
            Some(index) => index.saturating_sub(1),
            None => self.history.len() - 1,
        };
        self.set_history(index);
    }

    fn recall_newer(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 >= self.history.len() {
            self.clear();
        } else {
            self.set_history(index + 1);
        }
    }

    fn set_history(&mut self, index: usize) {
        self.text = self.history[index].chars().collect();
        self.cursor = self.text.len();
        self.history_index = Some(index);
    }
}

#[cfg(test)]
#[path = "composer_test.rs"]
mod composer_test;

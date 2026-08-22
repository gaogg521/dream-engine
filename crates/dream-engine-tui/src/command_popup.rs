use dream_engine_agent::commands::CommandSpec;

#[derive(Debug, Default)]
pub(super) struct CommandPopup {
    commands: Vec<CommandSpec>,
    filter: String,
    selected: usize,
}

impl CommandPopup {
    pub(super) fn set_commands(&mut self, commands: Vec<CommandSpec>) {
        self.commands = commands;
        self.selected = 0;
    }

    pub(super) fn update(&mut self, composer_text: &str) {
        let filter = command_filter(composer_text).unwrap_or_default();
        if filter != self.filter {
            self.filter = filter;
            self.selected = 0;
        }
        self.clamp_selection();
    }

    pub(super) fn is_visible(&self, composer_text: &str) -> bool {
        command_filter(composer_text).is_some() && !self.matches().is_empty()
    }

    pub(super) fn matches(&self) -> Vec<&CommandSpec> {
        let filter = self.filter.to_ascii_lowercase();
        let mut prefix = Vec::new();
        let mut contains = Vec::new();
        for command in &self.commands {
            let name = command.name.to_ascii_lowercase();
            if name.starts_with(&filter) {
                prefix.push(command);
            } else if name.contains(&filter) || command.aliases.iter().any(|alias| alias.contains(&filter)) {
                contains.push(command);
            }
        }
        prefix.extend(contains);
        prefix
    }

    pub(super) fn selected(&self) -> usize {
        self.selected
    }

    pub(super) fn commands(&self) -> &[CommandSpec] {
        &self.commands
    }

    pub(super) fn move_next(&mut self) {
        let count = self.matches().len();
        if count > 0 {
            self.selected = (self.selected + 1) % count;
        }
    }

    pub(super) fn move_previous(&mut self) {
        let count = self.matches().len();
        if count > 0 {
            self.selected = self.selected.checked_sub(1).unwrap_or(count - 1);
        }
    }

    pub(super) fn selected_name(&self) -> Option<String> {
        self.matches().get(self.selected).map(|command| command.name.clone())
    }

    pub(super) fn recognizes(&self, input: &str) -> bool {
        let Some(name) = input
            .trim()
            .strip_prefix('/')
            .and_then(|value| value.split_whitespace().next())
        else {
            return false;
        };
        self.commands
            .iter()
            .any(|command| command.name == name || command.aliases.iter().any(|alias| alias == name))
    }

    fn clamp_selection(&mut self) {
        let count = self.matches().len();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }
}

fn command_filter(text: &str) -> Option<String> {
    let value = text.strip_prefix('/')?;
    if value.chars().any(char::is_whitespace) {
        return None;
    }
    Some(value.to_string())
}

#[cfg(test)]
#[path = "command_popup_test.rs"]
mod command_popup_test;

use dream_engine_agent::commands::CommandSpec;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ApplicationCommand {
    Help,
    Mcp,
    Model(String),
    New,
    Permissions(String),
    Resume(String),
    Skills,
    Status,
}

impl ApplicationCommand {
    pub(super) fn parse(input: &str) -> Option<Self> {
        let command = input.trim().strip_prefix('/')?;
        let (name, args) = command.split_once(char::is_whitespace).unwrap_or((command, ""));
        let args = args.trim().to_string();
        match name {
            "help" => Some(Self::Help),
            "mcp" => Some(Self::Mcp),
            "model" => Some(Self::Model(args)),
            "new" => Some(Self::New),
            "permissions" => Some(Self::Permissions(args)),
            "resume" => Some(Self::Resume(args)),
            "skills" => Some(Self::Skills),
            "status" => Some(Self::Status),
            _ => None,
        }
    }
}

pub(super) fn application_command_specs() -> Vec<CommandSpec> {
    [
        ("mcp", "Show connected MCP servers"),
        ("model", "Show or change the active model"),
        ("new", "Start a new session"),
        ("permissions", "Show or change the approval mode"),
        ("resume", "Resume a saved session"),
        ("skills", "Show loaded skills"),
        ("status", "Show session and context status"),
    ]
    .into_iter()
    .map(|(name, description)| CommandSpec {
        name: name.to_string(),
        aliases: Vec::new(),
        description: description.to_string(),
    })
    .collect()
}

#[cfg(test)]
#[path = "app_command_test.rs"]
mod app_command_test;

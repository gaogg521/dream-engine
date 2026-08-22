use dream_engine_agent::commands::CommandSpec;

use super::CommandPopup;

fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            name: "compact".to_string(),
            aliases: Vec::new(),
            description: "Compress context".to_string(),
        },
        CommandSpec {
            name: "context".to_string(),
            aliases: Vec::new(),
            description: "Show context".to_string(),
        },
        CommandSpec {
            name: "quit".to_string(),
            aliases: vec!["exit".to_string()],
            description: "Exit".to_string(),
        },
    ]
}

#[test]
fn slash_opens_all_commands_and_text_filters_them() {
    let mut popup = CommandPopup::default();
    popup.set_commands(commands());
    popup.update("/");
    assert_eq!(popup.matches().len(), 3);
    popup.update("/cont");
    assert_eq!(popup.matches().len(), 1);
    assert_eq!(popup.selected_name().as_deref(), Some("context"));
}

#[test]
fn changing_filter_resets_stale_selection() {
    let mut popup = CommandPopup::default();
    popup.set_commands(commands());
    popup.update("/");
    popup.move_next();
    popup.move_next();
    popup.update("/co");
    assert_eq!(popup.selected(), 0);
    assert_eq!(popup.selected_name().as_deref(), Some("compact"));
}

#[test]
fn alias_is_recognized_but_not_duplicated_in_catalog() {
    let mut popup = CommandPopup::default();
    popup.set_commands(commands());
    assert!(popup.recognizes("/exit"));
    assert_eq!(popup.matches().len(), 3);
}

#[test]
fn arguments_close_command_popup() {
    let mut popup = CommandPopup::default();
    popup.set_commands(commands());
    popup.update("/context all");
    assert!(!popup.is_visible("/context all"));
}

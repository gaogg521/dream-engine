use super::{ApplicationCommand, application_command_specs};

#[test]
fn parses_application_commands_and_preserves_arguments() {
    assert_eq!(
        ApplicationCommand::parse("/model gpt-5.5"),
        Some(ApplicationCommand::Model("gpt-5.5".to_string()))
    );
    assert_eq!(
        ApplicationCommand::parse(" /resume   latest "),
        Some(ApplicationCommand::Resume("latest".to_string()))
    );
    assert_eq!(ApplicationCommand::parse("/status"), Some(ApplicationCommand::Status));
}

#[test]
fn ignores_agent_and_plain_messages() {
    assert_eq!(ApplicationCommand::parse("/compact"), None);
    assert_eq!(ApplicationCommand::parse("hello"), None);
}

#[test]
fn catalog_contains_only_tui_owned_commands() {
    let specs = application_command_specs();
    assert!(specs.iter().any(|spec| spec.name == "status"));
    assert!(specs.iter().any(|spec| spec.name == "resume"));
    assert!(!specs.iter().any(|spec| spec.name == "compact"));
    assert!(!specs.iter().any(|spec| spec.name == "help"));
}

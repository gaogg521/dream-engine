use dream_engine_agent::commands::CommandSpec;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};

use super::{
    dream_mark_lines, entry_style, pending_history_lines, render, streaming_history_commit, welcome_history_lines,
};
use crate::event::AgentEvent;
use crate::session_picker::TuiSession;
use crate::state::{AppState, ApprovalChoice};
use crate::transcript::{EntryKind, ToolStepStatus, TranscriptEntry};

#[test]
fn slash_command_popup_renders_an_unframed_selected_command() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    state.set_commands(vec![CommandSpec {
        name: "help".to_string(),
        aliases: Vec::new(),
        description: "List commands".to_string(),
    }]);
    state.composer.insert_text("/");
    state.popup.update("/");

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");
    let rendered = terminal.backend().to_string();
    assert!(rendered.contains("/help"));
    assert!(!rendered.contains("Commands"));
    assert!(
        !['│', '┌', '┐', '└', '┘']
            .iter()
            .any(|character| rendered.contains(*character))
    );

    let command_row = rendered
        .lines()
        .position(|line| line.contains("/help"))
        .expect("selected command should be visible") as u16;
    let final_content_cell = terminal
        .backend()
        .buffer()
        .cell((78, command_row))
        .expect("selected command should fill the content width");
    assert!(final_content_cell.modifier.contains(Modifier::REVERSED));
}

#[test]
fn slash_command_space_is_released_when_the_popup_closes() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    state.show_welcome = false;
    state.set_commands(
        (0..7)
            .map(|index| CommandSpec {
                name: format!("command{index}"),
                aliases: Vec::new(),
                description: "Description".to_string(),
            })
            .collect(),
    );
    state.composer.insert_text("/");
    state.popup.update("/");

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");
    let open = terminal.backend().to_string();
    let open_divider = open
        .lines()
        .position(|line| line.contains("────"))
        .expect("composer divider should be visible");
    assert_eq!(open_divider, 7);

    state.composer.clear();
    state.popup.update("");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");
    let closed = terminal.backend().to_string();
    let closed_divider = closed
        .lines()
        .position(|line| line.contains("────"))
        .expect("composer divider should be visible");
    assert_eq!(closed_divider, 0);
    assert!(!closed.contains("/command"));
}

#[test]
fn conversation_is_not_wrapped_in_an_outer_border() {
    let state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");

    let rendered = terminal.backend().to_string();
    assert!(
        !['│', '┌', '┐', '└', '┘']
            .iter()
            .any(|character| rendered.contains(*character))
    );
    assert!(rendered.contains("────"));
}

#[test]
fn runtime_metadata_is_rendered_in_the_footer_not_the_top_row() {
    let mut state = AppState::new(
        "gpt-5.5".to_string(),
        "openai".to_string(),
        "/workspace/project".to_string(),
        true,
    );
    state.session_id = Some("session-id".to_string());
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");

    let lines = terminal.backend().to_string();
    let lines = lines.lines().collect::<Vec<_>>();
    assert!(!lines[0].contains("openai"));
    let composer_row = lines
        .iter()
        .position(|line| line.contains("Type a message"))
        .expect("composer should be visible");
    let metadata_row = lines
        .iter()
        .position(|line| line.contains("openai"))
        .expect("metadata should be visible");
    let session_row = lines
        .iter()
        .position(|line| line.contains("session session-id"))
        .expect("session should be visible");
    assert!(metadata_row > composer_row);
    assert_eq!(session_row, metadata_row + 1);
}

#[test]
fn welcome_renders_ascii_dream_mark() {
    let state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");

    let rendered = terminal.backend().to_string();
    assert!(rendered.contains("::::::::"));
    assert!(rendered.contains("DreamCLI"));
}

#[test]
fn approval_modal_keeps_actions_visible_and_marks_the_selected_choice() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    state.handle_agent_event(AgentEvent::ApprovalRequested {
        call_id: "call-1".to_string(),
        name: "shell".to_string(),
        description: "Run a command".to_string(),
        input: "very long input ".repeat(30),
    });
    state.approval.as_mut().expect("approval should exist").choice = ApprovalChoice::Always;

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");

    let rendered = terminal.backend().to_string();
    assert!(rendered.contains("Allow once"));
    assert!(rendered.contains("Always allow"));
    assert!(rendered.contains("Enter confirm"));
    let action_row = rendered
        .lines()
        .position(|line| line.contains("Always allow"))
        .expect("approval actions should be visible") as u16;
    let action_line = rendered
        .lines()
        .nth(usize::from(action_row))
        .expect("action row should exist");
    let action_column = action_line
        .find("Always allow")
        .expect("selected action should be present") as u16;
    let selected = terminal
        .backend()
        .buffer()
        .cell((action_column, action_row))
        .expect("selected action cell should exist");
    assert!(selected.modifier.contains(Modifier::REVERSED));
}

#[test]
fn welcome_header_stays_close_to_the_composer() {
    let state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");

    let rendered = terminal.backend().to_string();
    let lines = rendered.lines().collect::<Vec<_>>();
    let subtitle_row = lines
        .iter()
        .position(|line| line.contains("Ask about this project"))
        .expect("welcome subtitle should be visible");
    let composer_divider_row = lines
        .iter()
        .position(|line| line.contains("────"))
        .expect("composer divider should be visible");
    assert_eq!(composer_divider_row.saturating_sub(subtitle_row), 2);
}

#[test]
fn welcome_ascii_uses_truecolor_grayscale_without_a_background() {
    let state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        false,
    );
    let mark = dream_mark_lines(&state);
    let colored = mark
        .iter()
        .flat_map(|line| &line.spans)
        .find(|span| span.style.fg == Some(Color::Rgb(28, 28, 28)))
        .expect("mark should contain a dense colored cell");
    assert_eq!(colored.style.fg, Some(Color::Rgb(28, 28, 28)));
    assert_eq!(colored.style.bg, None);
    assert!(
        mark.iter()
            .flat_map(|line| &line.spans)
            .flat_map(|span| span.content.chars())
            .all(|character| matches!(character, ':' | ' '))
    );
    for line in mark {
        let rendered = line.to_string();
        assert_eq!(rendered.chars().count(), 24);
        assert_eq!(rendered, rendered.chars().rev().collect::<String>());
    }
}

#[test]
fn welcome_mark_is_committed_separately_from_the_first_message() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    let welcome = welcome_history_lines(&state, 78)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(welcome.contains("::::::::"));

    state.show_welcome = false;
    state.begin_turn("first message");

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");

    let rendered = terminal.backend().to_string();
    assert!(!rendered.contains("::::::::"));
    assert!(rendered.contains("first message"));
}

#[test]
fn resumed_history_replay_contains_the_entire_session() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    state.show_welcome = false;
    state
        .transcript
        .push(TranscriptEntry::new(EntryKind::User, "", "old question"));
    state.transcript.push(TranscriptEntry::new(
        EntryKind::Assistant,
        "",
        (0..30).map(|index| format!("old answer {index}\n")).collect::<String>(),
    ));
    state
        .transcript
        .push(TranscriptEntry::new(EntryKind::User, "", "latest question"));
    state.transcript.push(TranscriptEntry::new(
        EntryKind::Assistant,
        "",
        (0..30)
            .map(|index| format!("latest answer {index}\n"))
            .collect::<String>(),
    ));
    let rendered = pending_history_lines(&state, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("old question"));
    assert!(rendered.contains("old answer 0"));
    assert!(rendered.contains("latest question"));
    assert!(rendered.contains("latest answer 29"));
}

#[test]
fn committed_history_leaves_only_new_transcript_pending() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    state.show_welcome = false;
    state
        .transcript
        .push(TranscriptEntry::new(EntryKind::User, "", "first question"));
    state.transcript.push(TranscriptEntry::new(
        EntryKind::Assistant,
        "",
        (0..80)
            .map(|index| format!("answer line {index}\n"))
            .collect::<String>(),
    ));
    let history = pending_history_lines(&state, 80);
    assert!(history.iter().any(|line| line.to_string().contains("first question")));
    assert!(history.iter().any(|line| line.to_string().contains("answer line 79")));

    state.mark_transcript_committed();
    assert!(pending_history_lines(&state, 80).is_empty());
    state
        .transcript
        .push(TranscriptEntry::new(EntryKind::User, "", "next question"));
    let pending = pending_history_lines(&state, 80);
    assert_eq!(pending.len(), 1);
    assert!(pending[0].to_string().contains("next question"));
}

#[test]
fn streaming_markdown_commits_complete_paragraphs_and_keeps_the_active_tail() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    state.show_welcome = false;
    state.begin_turn("question");
    state.mark_transcript_committed();
    state.handle_agent_event(crate::event::AgentEvent::TextDelta(
        "first paragraph\n\nsecond paragraph is still streaming".to_string(),
    ));

    let commit = streaming_history_commit(&state, 32).expect("the completed paragraph should be committed");
    let committed = commit
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(committed.contains("first paragraph"));
    assert!(!committed.contains("second paragraph"));

    state.commit_streaming_prefix(commit.complete_entries, commit.active_byte_count);
    for width in [24, 48] {
        let pending = pending_history_lines(&state, width)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!pending.contains("first paragraph"));
        assert!(pending.contains("second paragraph"));
    }
}

#[test]
fn streaming_markdown_does_not_commit_an_unclosed_code_fence() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    state.show_welcome = false;
    state.begin_turn("question");
    state.mark_transcript_committed();
    state.handle_agent_event(crate::event::AgentEvent::TextDelta(
        "```rust\nfn main() {\n\n    println!(\"hello\");\n".to_string(),
    ));

    assert!(streaming_history_commit(&state, 32).is_none());
}

#[test]
fn streaming_history_moves_completed_tool_steps_before_the_active_answer() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    state.show_welcome = false;
    state.begin_turn("question");
    state.mark_transcript_committed();
    state
        .transcript
        .push(TranscriptEntry::tool("Read", "file contents", ToolStepStatus::Success));
    state
        .transcript
        .push(TranscriptEntry::new(EntryKind::Assistant, "", "answer in progress"));

    let commit = streaming_history_commit(&state, 48).expect("the completed tool should be committed");
    assert_eq!(commit.complete_entries, 1);
    assert_eq!(commit.active_byte_count, 0);
    assert!(commit.lines.iter().any(|line| line.to_string().contains("Read")));
    assert!(
        !commit
            .lines
            .iter()
            .any(|line| line.to_string().contains("answer in progress"))
    );
}

#[test]
fn streaming_history_keeps_a_running_tool_mutable() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    state.show_welcome = false;
    state.begin_turn("question");
    state.mark_transcript_committed();
    state
        .transcript
        .push(TranscriptEntry::tool("Read", "request", ToolStepStatus::Running));
    state
        .transcript
        .push(TranscriptEntry::new(EntryKind::Assistant, "", "answer in progress"));

    assert!(streaming_history_commit(&state, 48).is_none());
}

#[test]
fn committed_history_does_not_leave_a_blank_transcript_viewport() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    state.show_welcome = false;
    state
        .transcript
        .push(TranscriptEntry::new(EntryKind::Assistant, "", "completed answer"));
    state.mark_transcript_committed();

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");
    let rendered = terminal.backend().to_string();
    let lines = rendered.lines().collect::<Vec<_>>();
    let composer_divider_row = lines
        .iter()
        .position(|line| line.contains("────"))
        .expect("composer divider should be visible");
    assert_eq!(composer_divider_row, 0);
}

#[test]
fn thinking_and_tools_render_countable_step_markers() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        false,
    );
    state.show_welcome = false;
    state
        .transcript
        .push(TranscriptEntry::new(EntryKind::Thinking, "Thinking", "checking"));
    state
        .transcript
        .push(TranscriptEntry::tool("Read", "file contents", ToolStepStatus::Success));

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");

    let rendered = terminal.backend().to_string();
    assert!(rendered.contains("• Thinking"));
    assert!(rendered.contains("• Read  done"));
}

#[test]
fn consecutive_tools_render_as_one_appended_group() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        false,
    );
    state.show_welcome = false;
    state
        .transcript
        .push(TranscriptEntry::tool("Read", "src/main.rs", ToolStepStatus::Success));
    state.transcript.push(TranscriptEntry::tool(
        "Grep",
        "matched two files",
        ToolStepStatus::Success,
    ));

    let backend = TestBackend::new(80, 18);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");

    let rendered = terminal.backend().to_string();
    assert!(rendered.contains("• Tools"));
    assert!(rendered.contains("├─ Read  done"));
    assert!(rendered.contains("└─ Grep  done"));
    assert!(rendered.contains("src/main.rs"));
    assert!(rendered.contains("matched two files"));
    assert!(!rendered.contains("• Read"));
    assert!(!rendered.contains("• Grep"));
    assert!(
        rendered.find("├─ Read").expect("Read should be visible")
            < rendered.find("└─ Grep").expect("Grep should be visible")
    );
}

#[test]
fn tool_group_keeps_only_the_three_most_recent_calls() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    state.show_welcome = false;
    for index in 1..=5 {
        state.transcript.push(TranscriptEntry::tool(
            format!("Tool{index}"),
            format!("output {index}"),
            ToolStepStatus::Success,
        ));
    }

    let rendered = pending_history_lines(&state, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("… 2 earlier tools"));
    assert!(!rendered.contains("Tool1"));
    assert!(!rendered.contains("output 2"));
    assert!(rendered.contains("Tool3"));
    assert!(rendered.contains("Tool4"));
    assert!(rendered.contains("Tool5"));
}

#[test]
fn assistant_markdown_is_rendered_but_user_markdown_stays_literal() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        false,
    );
    state.show_welcome = false;
    state
        .transcript
        .push(TranscriptEntry::new(EntryKind::User, "", "**literal** `input`"));
    state.transcript.push(TranscriptEntry::new(
        EntryKind::Assistant,
        "",
        "Use `cargo check` and **review**.\n\n```rust\nfn main() {}\n```",
    ));

    let lines = pending_history_lines(&state, 48);
    let rendered = lines.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
    assert!(rendered.contains("**literal** `input`"));
    assert!(rendered.contains("Use cargo check and review."));
    assert!(!rendered.contains("```"));
    assert!(!rendered.contains("rust"));

    let code = lines
        .iter()
        .flat_map(|line| &line.spans)
        .find(|span| span.content == "cargo check")
        .expect("inline code should be rendered as a styled span");
    assert_eq!(code.style.bg, Some(Color::Rgb(232, 232, 232)));
    let strong = lines
        .iter()
        .flat_map(|line| &line.spans)
        .find(|span| span.content == "review")
        .expect("strong text should be rendered as a styled span");
    assert!(strong.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn assistant_code_block_refills_after_resize() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        false,
    );
    state.show_welcome = false;
    state.transcript.push(TranscriptEntry::new(
        EntryKind::Assistant,
        "",
        "```text\nresponsive code\n```",
    ));

    for width in [40, 64] {
        let lines = pending_history_lines(&state, width);
        let code_line = lines
            .iter()
            .find(|line| line.to_string().contains("responsive code"))
            .expect("code line should be present");
        assert_eq!(
            unicode_width::UnicodeWidthStr::width(code_line.to_string().as_str()),
            width as usize
        );
    }
}

#[test]
fn tool_states_use_distinct_semantic_colors_and_text() {
    let status_rows = [
        ("Queued", ToolStepStatus::Queued),
        ("Approval", ToolStepStatus::Approval),
        ("Running", ToolStepStatus::Running),
        ("Success", ToolStepStatus::Success),
        ("Error", ToolStepStatus::Error),
        ("Cancelled", ToolStepStatus::Cancelled),
    ];
    let expected_colors = [
        Color::Rgb(92, 92, 92),
        Color::Rgb(146, 89, 0),
        Color::Rgb(29, 78, 216),
        Color::Rgb(21, 128, 61),
        Color::Rgb(185, 28, 28),
        Color::Rgb(109, 40, 217),
    ];
    for ((name, status), color) in status_rows.into_iter().zip(expected_colors) {
        let mut state = AppState::new(
            "model".to_string(),
            "provider".to_string(),
            "/workspace".to_string(),
            false,
        );
        state.show_welcome = false;
        state.transcript.push(TranscriptEntry::tool(name, "preview", status));
        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
        terminal
            .draw(|frame| render(frame, &state))
            .expect("render should succeed");
        let rendered = terminal.backend().to_string();
        let row = rendered
            .lines()
            .position(|line| line.contains(name))
            .expect("tool status should be visible") as u16;
        let marker = terminal
            .backend()
            .buffer()
            .cell((1, row))
            .expect("tool marker should exist");
        assert_eq!(marker.fg, color);
    }
}

#[test]
fn tool_output_is_a_responsive_single_line_preview() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    state.show_welcome = false;
    state.transcript.push(TranscriptEntry::tool(
        "Read",
        "first line\nsecond line with a very long result that should not take over the terminal window",
        ToolStepStatus::Success,
    ));

    let backend = TestBackend::new(44, 12);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");

    let rendered = terminal.backend().to_string();
    assert!(rendered.contains("first line second line"));
    assert!(rendered.contains('…'));
    assert!(!rendered.contains("terminal window"));
}

#[test]
fn user_and_assistant_messages_have_distinct_backgrounds() {
    let state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        false,
    );
    let user = entry_style(EntryKind::User, &state);
    let assistant = entry_style(EntryKind::Assistant, &state);
    assert_eq!(user.bg, Some(Color::Black));
    assert!(!user.add_modifier.contains(Modifier::BOLD));
    assert_eq!(assistant.fg, Some(Color::Black));
    assert_eq!(assistant.bg, None);
}

#[test]
fn message_background_fills_the_current_render_width() {
    for width in [40, 64] {
        let mut state = AppState::new(
            "model".to_string(),
            "provider".to_string(),
            "/workspace".to_string(),
            false,
        );
        state
            .transcript
            .push(TranscriptEntry::new(EntryKind::User, "", "short message"));
        let backend = TestBackend::new(width, 16);
        let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
        terminal
            .draw(|frame| render(frame, &state))
            .expect("render should succeed");

        let rendered = terminal.backend().to_string();
        let message_row = rendered
            .lines()
            .position(|line| line.contains("short message"))
            .expect("message should be visible") as u16;
        let final_content_column = width - 2;
        let cell = terminal
            .backend()
            .buffer()
            .cell((final_content_column, message_row))
            .expect("last message cell should exist");
        assert_eq!(cell.bg, Color::Black);
    }
}

#[test]
fn user_message_background_has_half_row_vertical_padding() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        false,
    );
    state.show_welcome = false;
    state
        .transcript
        .push(TranscriptEntry::new(EntryKind::User, "", "message"));

    let lines = pending_history_lines(&state, 24);
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].to_string(), "▄".repeat(24));
    assert_eq!(lines[2].to_string(), "▀".repeat(24));
    assert_eq!(lines[0].spans[0].style.fg, Some(Color::Black));
    assert_eq!(lines[2].spans[0].style.fg, Some(Color::Black));
}

#[test]
fn short_transcript_stays_close_to_the_composer_after_resize() {
    for height in [16, 30] {
        let mut state = AppState::new(
            "model".to_string(),
            "provider".to_string(),
            "/workspace".to_string(),
            true,
        );
        state.show_welcome = false;
        state
            .transcript
            .push(TranscriptEntry::new(EntryKind::Assistant, "", "short answer"));
        let backend = TestBackend::new(80, height);
        let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
        terminal
            .draw(|frame| render(frame, &state))
            .expect("render should succeed");

        let rendered = terminal.backend().to_string();
        let lines = rendered.lines().collect::<Vec<_>>();
        let answer_row = lines
            .iter()
            .position(|line| line.contains("short answer"))
            .expect("answer should be visible");
        let composer_divider_row = lines
            .iter()
            .position(|line| line.contains("────"))
            .expect("composer divider should be visible");
        assert_eq!(composer_divider_row.saturating_sub(answer_row), 2);
    }
}

#[test]
fn footer_contains_only_runtime_metadata() {
    let state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");

    let rendered = terminal.backend().to_string();
    let lines: Vec<_> = rendered.lines().collect();
    assert!(lines.iter().any(|line| line.contains("DreamCLI")));
    assert!(lines.iter().any(|line| line.contains("session new")));
    assert!(!rendered.contains("Enter send"));
    assert!(!rendered.contains("Shift+Enter"));
    assert!(!rendered.contains("mouse wheel"));
    assert!(!rendered.contains("drag to select"));
}

#[test]
fn undersized_terminal_shows_resize_message() {
    let state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    let backend = TestBackend::new(21, 7);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");
    assert!(terminal.backend().to_string().contains("Terminal too small"));
}

#[test]
fn initialization_state_replaces_the_terminal_before_bootstrap() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        true,
    );
    state.begin_initialization();
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");

    let rendered = terminal.backend().to_string();
    assert!(rendered.contains("Starting DreamCLI"));
    assert!(rendered.contains("starting"));
    assert!(!rendered.contains("Type a message"));
}

#[test]
fn resume_picker_renders_a_full_screen_divider_window() {
    let mut state = AppState::new(
        "model".to_string(),
        "provider".to_string(),
        "/workspace".to_string(),
        false,
    );
    state.session_picker.set_sessions(vec![
        TuiSession::new(
            "session-one".to_string(),
            "gpt-5.5".to_string(),
            "First task".to_string(),
            "2026-08-13 12:00 UTC".to_string(),
            4,
        ),
        TuiSession::new(
            "session-two".to_string(),
            "gpt-5.5".to_string(),
            "Second task".to_string(),
            "2026-08-13 13:00 UTC".to_string(),
            8,
        ),
    ]);
    state.session_picker.open();
    state.session_picker.move_next();

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
    terminal
        .draw(|frame| render(frame, &state))
        .expect("render should succeed");

    let rendered = terminal.backend().to_string();
    assert!(rendered.contains("Resume session · 2"));
    assert!(rendered.contains("First task"));
    assert!(rendered.contains("Second task"));
    assert!(!rendered.contains("Type a message"));
    assert!(!rendered.contains("DreamCLI ·"));
    assert!(!rendered.contains('│'));

    let selected_row = rendered
        .lines()
        .position(|line| line.contains("Second task"))
        .expect("selected session should be visible") as u16;
    let selected_width = (0..80)
        .filter(|column| {
            terminal
                .backend()
                .buffer()
                .cell((*column, selected_row))
                .is_some_and(|cell| cell.modifier.contains(Modifier::REVERSED))
        })
        .count();
    assert!(selected_width >= 70, "selected row should fill the picker width");
}

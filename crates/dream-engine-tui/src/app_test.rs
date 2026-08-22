use dream_engine_protocol::ToolApprovalResult;
use dream_engine_protocol::commands::SessionMode;
use dream_engine_protocol::events::ToolCategory;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::oneshot;

use super::{IdleAction, TuiMetadata, TuiOutcome, TuiRuntime, application_help, permission_mode, starts_conversation};
use crate::event::AgentEvent;
use crate::session_picker::TuiSession;

#[test]
fn permission_modes_accept_documented_spellings() {
    assert_eq!(permission_mode("default"), Some(SessionMode::Default));
    assert_eq!(permission_mode("auto_edit"), Some(SessionMode::AutoEdit));
    assert_eq!(permission_mode("auto-edit"), Some(SessionMode::AutoEdit));
    assert_eq!(permission_mode("yolo"), Some(SessionMode::Yolo));
    assert_eq!(permission_mode("unsafe"), None);
}

fn runtime() -> TuiRuntime {
    TuiRuntime::new(TuiMetadata {
        model: "model".to_string(),
        provider: "provider".to_string(),
        cwd: "/workspace".to_string(),
        no_color: true,
    })
}

fn session(id: &str) -> TuiSession {
    TuiSession::new(
        id.to_string(),
        "model".to_string(),
        "summary".to_string(),
        "2026-08-13 12:00 UTC".to_string(),
        2,
    )
}

fn request_approval(runtime: &mut TuiRuntime) -> oneshot::Receiver<ToolApprovalResult> {
    let receiver = runtime.approval_manager.request_approval("call-1", &ToolCategory::Exec);
    runtime.state.handle_agent_event(AgentEvent::ApprovalRequested {
        call_id: "call-1".to_string(),
        name: "shell".to_string(),
        description: "Run a command".to_string(),
        input: "cargo test".to_string(),
    });
    receiver
}

#[test]
fn resume_picker_uses_arrow_keys_and_enter() {
    let mut runtime = runtime();
    runtime
        .state
        .session_picker
        .set_sessions(vec![session("first"), session("second")]);
    assert!(runtime.state.session_picker.open());

    assert!(matches!(
        runtime.handle_idle_key(KeyEvent::from(KeyCode::Down)),
        IdleAction::Continue
    ));
    let outcome = runtime.handle_idle_key(KeyEvent::from(KeyCode::Enter));
    assert!(matches!(
        outcome,
        IdleAction::Outcome(TuiOutcome::ResumeSession(session_id)) if session_id == "second"
    ));
    assert!(!runtime.state.session_picker.is_visible());
}

#[test]
fn resetting_a_session_discards_queued_events_from_the_previous_session() {
    let mut runtime = runtime();
    runtime.reset_session(
        "first-model".to_string(),
        "first-provider".to_string(),
        Some("first-session".to_string()),
        &[],
    );
    assert!(!runtime.needs_full_clear);

    runtime
        .tx
        .send(AgentEvent::Info("stale session output".to_string()))
        .expect("event channel should be open");

    runtime.reset_session(
        "next-model".to_string(),
        "next-provider".to_string(),
        Some("next-session".to_string()),
        &[],
    );

    assert!(runtime.state.transcript.is_empty());
    assert!(runtime.needs_full_clear);
}

#[test]
fn only_real_messages_start_a_lazy_session() {
    assert!(starts_conversation("hello"));
    assert!(starts_conversation("  hello"));
    assert!(!starts_conversation("/status"));
    assert!(!starts_conversation("  /resume latest"));
}

#[test]
fn a_fresh_runtime_can_defer_a_requested_session_id() {
    let mut runtime = runtime();
    runtime.defer_session_initialization(Some("custom-session".to_string()));

    assert!(runtime.session_initialization_pending);
    assert_eq!(runtime.requested_session_id.as_deref(), Some("custom-session"));
    assert!(runtime.state.session_id.is_none());
}

#[test]
fn help_contains_input_and_navigation_guidance() {
    let output = application_help(&[dream_engine_agent::commands::CommandSpec {
        name: "status".to_string(),
        aliases: Vec::new(),
        description: "Show status".to_string(),
    }]);

    assert!(output.contains("/status"));
    assert!(output.contains("Shift+Enter   Insert newline"));
    assert!(output.contains("Mouse wheel   Scroll conversation"));
    assert!(output.contains("Drag          Select terminal text"));
}

#[test]
fn approval_navigation_selects_always_and_confirms_with_enter() {
    let mut runtime = runtime();
    let mut receiver = request_approval(&mut runtime);

    assert!(!runtime.handle_running_key(KeyEvent::from(KeyCode::Right)));
    assert_eq!(
        runtime.state.approval.as_ref().map(|request| request.choice),
        Some(crate::state::ApprovalChoice::Always)
    );
    assert!(!runtime.handle_running_key(KeyEvent::from(KeyCode::Enter)));

    assert!(matches!(receiver.try_recv(), Ok(ToolApprovalResult::Approved)));
    assert!(runtime.state.approval.is_none());
    assert!(runtime.approval_manager.is_auto_approved("exec"));
}

#[test]
fn approval_shortcuts_accept_uppercase_characters() {
    let mut runtime = runtime();
    let mut receiver = request_approval(&mut runtime);

    let key = KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::SHIFT);
    assert!(!runtime.handle_running_key(key));

    assert!(matches!(receiver.try_recv(), Ok(ToolApprovalResult::Approved)));
}

#[test]
fn control_c_during_approval_denies_the_request_and_cancels_the_turn() {
    let mut runtime = runtime();
    let mut receiver = request_approval(&mut runtime);

    let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(runtime.handle_running_key(key));

    assert!(matches!(
        receiver.try_recv(),
        Ok(ToolApprovalResult::Denied { reason }) if reason == "Turn cancelled by user"
    ));
    assert!(runtime.state.approval.is_none());
}

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use dream_engine_agent::commands::CommandSpec;
use dream_engine_agent::engine::AgentEngine;
use dream_engine_agent::error::AgentError;
use dream_engine_agent::output::OutputSink;
use dream_engine_protocol::commands::{ApprovalScope, SessionMode};
use dream_engine_protocol::writer::ProtocolEmitter;
use dream_engine_protocol::{ToolApprovalManager, ToolApprovalResult};
use dream_engine_types::message::Message;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time;

use crate::app_command::ApplicationCommand;
use crate::event::{AgentEvent, TuiProtocolEmitter, TuiSink};
use crate::session_picker::TuiSession;
use crate::state::{AppState, ApprovalChoice};
use crate::terminal::{
    AppTerminal, TerminalSession, clear_synchronized, draw_synchronized, insert_history_lines,
    reset_inline_synchronized,
};
use crate::terminal_event_reader::TerminalEventReader;
use crate::ui;

static MESSAGE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub struct TuiMetadata {
    pub model: String,
    pub provider: String,
    pub cwd: String,
    pub no_color: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiOutcome {
    Exit,
    NewSession,
    ResumeSession(String),
}

pub struct TuiRuntime {
    state: AppState,
    tx: UnboundedSender<AgentEvent>,
    rx: UnboundedReceiver<AgentEvent>,
    approval_manager: Arc<ToolApprovalManager>,
    protocol_emitter: Arc<dyn ProtocolEmitter>,
    terminal_session: Option<TerminalSession>,
    mcp_servers: Vec<String>,
    skills: Vec<String>,
    session_catalog_error: Option<String>,
    needs_full_clear: bool,
    session_configured: bool,
    history_replay_pending: bool,
    session_initialization_pending: bool,
    requested_session_id: Option<String>,
}

impl TuiRuntime {
    pub fn new(metadata: TuiMetadata) -> Self {
        let (tx, rx) = unbounded_channel();
        let protocol_emitter = TuiProtocolEmitter::shared(tx.clone());
        Self {
            state: AppState::new(metadata.model, metadata.provider, metadata.cwd, metadata.no_color),
            tx,
            rx,
            approval_manager: Arc::new(ToolApprovalManager::new()),
            protocol_emitter,
            terminal_session: None,
            mcp_servers: Vec::new(),
            skills: Vec::new(),
            session_catalog_error: None,
            needs_full_clear: false,
            session_configured: false,
            history_replay_pending: false,
            session_initialization_pending: false,
            requested_session_id: None,
        }
    }

    pub fn output_sink(&self) -> Arc<dyn OutputSink> {
        TuiSink::shared(self.tx.clone())
    }

    pub fn prepare_terminal(&mut self) -> anyhow::Result<()> {
        if self.terminal_session.is_some() {
            return Ok(());
        }
        self.state.begin_initialization();
        let mut terminal_session = TerminalSession::enter()?;
        draw_synchronized(terminal_session.terminal(), |frame| ui::render(frame, &self.state))?;
        self.terminal_session = Some(terminal_session);
        Ok(())
    }

    pub fn set_commands(&mut self, commands: Vec<CommandSpec>) {
        self.state.set_commands(commands);
    }

    pub fn reset_session(&mut self, model: String, provider: String, session_id: Option<String>, messages: &[Message]) {
        self.drain_agent_events();
        self.state.reset_session(model, provider, session_id, messages);
        self.needs_full_clear = self.session_configured;
        self.session_configured = true;
        self.history_replay_pending = true;
        self.session_initialization_pending = false;
        self.requested_session_id = None;
    }

    pub fn defer_session_initialization(&mut self, requested_session_id: Option<String>) {
        self.session_initialization_pending = true;
        self.requested_session_id = requested_session_id;
    }

    pub fn set_runtime_catalog(
        &mut self,
        mcp_servers: Vec<String>,
        skills: Vec<String>,
        sessions: Result<Vec<TuiSession>, String>,
    ) {
        self.mcp_servers = mcp_servers;
        self.skills = skills;
        match sessions {
            Ok(sessions) => {
                self.state.session_picker.set_sessions(sessions);
                self.session_catalog_error = None;
            }
            Err(error) => {
                self.state.session_picker.set_sessions(Vec::new());
                self.session_catalog_error = Some(error);
            }
        }
    }

    pub fn show_error(&mut self, error: impl Into<String>) {
        self.state.push_error(error);
    }

    pub async fn run(&mut self, engine: &mut AgentEngine) -> anyhow::Result<TuiOutcome> {
        engine.set_approval_manager(self.approval_manager.clone());
        engine.set_protocol_writer(self.protocol_emitter.clone());

        let mut terminal_session = match self.terminal_session.take() {
            Some(session) => session,
            None => TerminalSession::enter()?,
        };
        if self.needs_full_clear {
            terminal_session.reset_inline()?;
            self.needs_full_clear = false;
        }
        if self.history_replay_pending {
            self.commit_pending_history(terminal_session.terminal())?;
            self.history_replay_pending = false;
        }
        let mut terminal_events = TerminalEventReader::new();
        let mut ticker = time::interval(Duration::from_millis(120));
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

        let outcome = loop {
            terminal_session.set_picker_mode(self.state.session_picker.is_visible())?;
            self.draw(terminal_session.terminal())?;
            tokio::select! {
                event = self.rx.recv() => {
                    if let Some(event) = event {
                        self.state.handle_agent_event(event);
                    }
                }
                event = terminal_events.next() => {
                    let Some(event) = event else {
                        break TuiOutcome::Exit;
                    };
                    match event? {
                        Event::Key(key) if is_key_press(key) => {
                            match self.handle_idle_key(key) {
                                IdleAction::Continue => {}
                                IdleAction::Exit => break TuiOutcome::Exit,
                                IdleAction::Outcome(outcome) => break outcome,
                                IdleAction::Submit(input) => {
                                    match self.handle_application_command(engine, &input) {
                                        CommandAction::Handled => {
                                            self.commit_pending_history(terminal_session.terminal())?;
                                        }
                                        CommandAction::Outcome(outcome) => break outcome,
                                        CommandAction::PassThrough => {
                                            if self.run_turn(engine, input, terminal_session.terminal(), &mut terminal_events, &mut ticker).await? {
                                                break TuiOutcome::Exit;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Event::Paste(text) => {
                            if !self.state.session_picker.is_visible() {
                                self.state.composer.insert_text(&text);
                                self.update_popup();
                            }
                        }
                        Event::Resize(_, _) => {
                            if self.state.session_picker.is_visible() {
                                resize_terminal(terminal_session.terminal(), &mut terminal_events)?;
                            } else {
                                self.reflow_history(terminal_session.terminal(), &mut terminal_events)?;
                            }
                        }
                        _ => {}
                    }
                }
            }
        };

        terminal_session.set_picker_mode(false)?;
        if !matches!(&outcome, TuiOutcome::Exit) {
            self.terminal_session = Some(terminal_session);
        }
        Ok(outcome)
    }

    fn handle_application_command(&mut self, engine: &mut AgentEngine, input: &str) -> CommandAction {
        let Some(command) = ApplicationCommand::parse(input) else {
            return CommandAction::PassThrough;
        };

        match command {
            ApplicationCommand::Help => {
                self.state
                    .push_info("Commands", application_help(self.state.popup.commands()));
            }
            ApplicationCommand::Status => {
                let context = engine.context_status();
                let percent = if context.context_window == 0 {
                    0.0
                } else {
                    context.context_usage as f64 * 100.0 / context.context_window as f64
                };
                let text = format!(
                    "Provider: {}\nModel: {}\nSession: {}\nPermissions: {}\nContext: {}/{} tokens ({percent:.1}%)\nContext source: {:?}\nCompactions: {} full, {} micro\nWorking directory: {}",
                    self.state.provider,
                    context.model,
                    self.state.session_id.as_deref().unwrap_or("new"),
                    self.approval_manager.current_mode(),
                    context.context_usage,
                    context.context_window,
                    context.source,
                    context.compact_count,
                    context.microcompact_count,
                    self.state.cwd,
                );
                self.state.push_info("Status", text);
            }
            ApplicationCommand::Permissions(args) => {
                if args.is_empty() {
                    self.state.push_info(
                        "Permissions",
                        format!(
                            "Current mode: {}\nModes: default, auto_edit, yolo\nUse /permissions <mode> to change it.",
                            self.approval_manager.current_mode()
                        ),
                    );
                } else if let Some(mode) = permission_mode(&args) {
                    self.approval_manager.set_mode(mode);
                    self.state.push_info(
                        "Permissions",
                        format!("Approval mode changed to {}", self.approval_manager.current_mode()),
                    );
                } else {
                    self.state.push_error(format!(
                        "Unknown permission mode: {args}. Use default, auto_edit, or yolo."
                    ));
                }
            }
            ApplicationCommand::Model(model) => {
                if model.is_empty() {
                    self.state.push_info(
                        "Model",
                        format!("Current model: {}\nUse /model <name> to change it.", self.state.model),
                    );
                } else if model.chars().any(char::is_whitespace) {
                    self.state.push_error("Model name cannot contain whitespace");
                } else {
                    let changes = engine.apply_config_update(Some(model), None, None, None, None, None);
                    self.state.model = engine.context_status().model;
                    self.state.push_info("Model", changes.join("\n"));
                }
            }
            ApplicationCommand::New => {
                self.state.push_info("Session", "Starting a new session…");
                return CommandAction::Outcome(TuiOutcome::NewSession);
            }
            ApplicationCommand::Resume(session_id) => {
                if session_id.is_empty() {
                    if let Some(error) = &self.session_catalog_error {
                        self.state.push_error(format!("Could not list saved sessions: {error}"));
                    } else if self.state.session_picker.is_empty() {
                        self.state
                            .push_info("Resume", "No saved sessions found. Use /resume <id> or /resume latest.");
                    } else {
                        self.state.session_picker.open();
                    }
                } else if session_id.chars().any(char::is_whitespace) {
                    self.state.push_error("Session ID cannot contain whitespace");
                } else {
                    self.state
                        .push_info("Session", format!("Resuming session {session_id}…"));
                    return CommandAction::Outcome(TuiOutcome::ResumeSession(session_id));
                }
            }
            ApplicationCommand::Mcp => {
                let text = if self.mcp_servers.is_empty() {
                    "No MCP servers connected".to_string()
                } else {
                    format!(
                        "Connected servers · {}\n  {}",
                        self.mcp_servers.len(),
                        self.mcp_servers.join("\n  ")
                    )
                };
                self.state.push_info("MCP", text);
            }
            ApplicationCommand::Skills => {
                let text = if self.skills.is_empty() {
                    "No model-visible skills loaded".to_string()
                } else {
                    format!("Loaded skills · {}\n  {}", self.skills.len(), self.skills.join("\n  "))
                };
                self.state.push_info("Skills", text);
            }
        }
        CommandAction::Handled
    }

    async fn run_turn(
        &mut self,
        engine: &mut AgentEngine,
        input: String,
        terminal: &mut AppTerminal,
        terminal_events: &mut TerminalEventReader,
        ticker: &mut time::Interval,
    ) -> anyhow::Result<bool> {
        if self.session_initialization_pending && starts_conversation(&input) {
            engine.init_session(
                &self.state.provider,
                &self.state.cwd,
                self.requested_session_id.as_deref(),
            )?;
            self.session_initialization_pending = false;
            self.requested_session_id = None;
            self.state.session_id = engine.current_session_id();
        }
        if self.state.pending_transcript().is_empty() && self.state.show_welcome {
            self.commit_pending_history(terminal)?;
        }
        self.state.begin_turn(&input);
        self.commit_pending_history(terminal)?;
        let message_id = format!("tui-{}", MESSAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed));
        let mut cancelled = false;
        let result = {
            let engine_future = engine.run(&input, &message_id);
            tokio::pin!(engine_future);

            loop {
                self.commit_streaming_history(terminal)?;
                self.draw(terminal)?;
                tokio::select! {
                    result = &mut engine_future => break Some(result),
                    event = self.rx.recv() => {
                        if let Some(event) = event {
                            self.state.handle_agent_event(event);
                        }
                    }
                    event = terminal_events.next() => {
                        let Some(event) = event else {
                            cancelled = true;
                            break None;
                        };
                        match event? {
                            Event::Key(key)
                                if is_key_press(key) && self.handle_running_key(key) =>
                            {
                                cancelled = true;
                                break None;
                            }
                            Event::Resize(_, _) => self.reflow_history(terminal, terminal_events)?,
                            _ => {}
                        }
                    }
                    _ = ticker.tick() => self.state.tick(),
                }
            }
        };

        if cancelled {
            self.deny_pending_approval("Turn cancelled");
            engine.abort_current_turn("Turn cancelled by user");
            self.state.cancel_turn();
            self.drain_agent_events();
            self.commit_pending_history(terminal)?;
            return Ok(false);
        }

        self.drain_agent_events();
        match result {
            Some(Ok(result)) => {
                self.state.finish_turn(result.turns, result.usage);
                self.commit_pending_history(terminal)?;
                Ok(false)
            }
            Some(Err(AgentError::UserAborted)) => {
                self.state.busy = false;
                Ok(true)
            }
            Some(Err(error)) => {
                self.state.busy = false;
                self.state.handle_agent_event(AgentEvent::Error(error.to_string()));
                self.commit_pending_history(terminal)?;
                Ok(false)
            }
            None => Ok(false),
        }
    }

    fn handle_idle_key(&mut self, key: KeyEvent) -> IdleAction {
        if self.state.session_picker.is_visible() {
            return self.handle_session_picker_key(key);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.state.composer.is_empty() {
                return IdleAction::Exit;
            }
            self.state.composer.clear();
            self.update_popup();
            return IdleAction::Continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('d')
            && self.state.composer.is_empty()
        {
            return IdleAction::Exit;
        }

        let popup_visible = self.state.popup.is_visible(&self.state.composer.text());
        match key.code {
            KeyCode::Up if popup_visible => self.state.popup.move_previous(),
            KeyCode::Down if popup_visible => self.state.popup.move_next(),
            KeyCode::Tab if popup_visible => {
                if let Some(name) = self.state.popup.selected_name() {
                    self.state.composer.replace_command(&name);
                    self.update_popup();
                }
            }
            KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                if popup_visible && let Some(name) = self.state.popup.selected_name() {
                    self.state.composer.replace_command(&name);
                }
                let input = self.state.composer.take();
                self.update_popup();
                if !input.trim().is_empty() {
                    return IdleAction::Submit(input);
                }
            }
            KeyCode::Esc if popup_visible => {
                self.state.composer.clear();
                self.update_popup();
            }
            _ => {
                if self.state.composer.input(key) {
                    self.update_popup();
                }
            }
        }
        IdleAction::Continue
    }

    fn handle_session_picker_key(&mut self, key: KeyEvent) -> IdleAction {
        match key.code {
            KeyCode::Up => self.state.session_picker.move_previous(),
            KeyCode::Down => self.state.session_picker.move_next(),
            KeyCode::Enter => {
                let Some(session_id) = self.state.session_picker.selected_id() else {
                    self.state.session_picker.close();
                    return IdleAction::Continue;
                };
                self.state.session_picker.close();
                self.state
                    .push_info("Session", format!("Resuming session {session_id}…"));
                return IdleAction::Outcome(TuiOutcome::ResumeSession(session_id));
            }
            KeyCode::Esc | KeyCode::Char('q') => self.state.session_picker.close(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.state.session_picker.close();
            }
            _ => {}
        }
        IdleAction::Continue
    }

    /// Returns true when the active turn should be cancelled.
    fn handle_running_key(&mut self, key: KeyEvent) -> bool {
        if self.state.approval.is_some() {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                self.deny_pending_approval("Turn cancelled by user");
                return true;
            }

            let shortcut = match key.code {
                KeyCode::Char(character) => Some(character.to_ascii_lowercase()),
                _ => None,
            };
            let choice = match (key.code, shortcut) {
                (KeyCode::Left | KeyCode::Up | KeyCode::BackTab, _) => {
                    if let Some(request) = self.state.approval.as_mut() {
                        request.choice = request.choice.previous();
                    }
                    None
                }
                (KeyCode::Right | KeyCode::Down | KeyCode::Tab, _) => {
                    if let Some(request) = self.state.approval.as_mut() {
                        request.choice = request.choice.next();
                    }
                    None
                }
                (KeyCode::Enter, _) => self.state.approval.as_ref().map(|request| request.choice),
                (_, Some('y')) => Some(ApprovalChoice::Once),
                (_, Some('a')) => Some(ApprovalChoice::Always),
                (_, Some('n')) | (KeyCode::Esc, _) => Some(ApprovalChoice::Deny),
                _ => None,
            };
            if let Some(choice) = choice {
                self.resolve_pending_approval(choice);
            }
            return false;
        }

        key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
    }

    fn resolve_pending_approval(&mut self, choice: ApprovalChoice) {
        let Some(request) = self.state.approval.take() else {
            return;
        };
        match choice {
            ApprovalChoice::Once => self.approval_manager.approve(&request.call_id, ApprovalScope::Once),
            ApprovalChoice::Always => self.approval_manager.approve(&request.call_id, ApprovalScope::Always),
            ApprovalChoice::Deny => self.approval_manager.resolve(
                &request.call_id,
                ToolApprovalResult::Denied {
                    reason: "Denied by user".to_string(),
                },
            ),
        }
    }

    fn deny_pending_approval(&mut self, reason: &str) {
        let Some(request) = self.state.approval.take() else {
            return;
        };
        self.approval_manager.resolve(
            &request.call_id,
            ToolApprovalResult::Denied {
                reason: reason.to_string(),
            },
        );
    }

    fn update_popup(&mut self) {
        let text = self.state.composer.text();
        self.state.popup.update(&text);
    }

    fn drain_agent_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            self.state.handle_agent_event(event);
        }
    }

    fn draw(&self, terminal: &mut AppTerminal) -> anyhow::Result<()> {
        draw_synchronized(terminal, |frame| ui::render(frame, &self.state))
    }

    fn reflow_history(
        &mut self,
        terminal: &mut AppTerminal,
        terminal_events: &mut TerminalEventReader,
    ) -> anyhow::Result<()> {
        with_terminal_events_paused(terminal_events, || {
            let replayable_count = self.state.prepare_history_replay();
            reset_inline_synchronized(terminal)?;
            let width = terminal.size()?.width.saturating_sub(2).max(1);
            let lines = if self.state.transcript.is_empty() && self.state.show_welcome {
                self.state.show_welcome = false;
                ui::welcome_history_lines(&self.state, width)
            } else {
                ui::history_prefix_lines(&self.state, replayable_count, width)
            };
            if !lines.is_empty() {
                insert_history_lines(terminal, lines)?;
            }
            self.state.mark_transcript_prefix_committed(replayable_count);
            clear_synchronized(terminal)
        })
    }

    fn commit_pending_history(&mut self, terminal: &mut AppTerminal) -> anyhow::Result<()> {
        let width = terminal.size()?.width.saturating_sub(2).max(1);
        let lines = if self.state.pending_transcript().is_empty() && self.state.show_welcome {
            self.state.show_welcome = false;
            ui::welcome_history_lines(&self.state, width)
        } else {
            ui::pending_history_lines(&self.state, width)
        };
        if lines.is_empty() {
            return Ok(());
        }
        insert_history_lines(terminal, lines)?;
        self.state.mark_transcript_committed();
        clear_synchronized(terminal)
    }

    fn commit_streaming_history(&mut self, terminal: &mut AppTerminal) -> anyhow::Result<()> {
        if !self.state.can_commit_streaming_lines() {
            return Ok(());
        }

        let width = terminal.size()?.width.saturating_sub(2).max(1);
        let Some(commit) = ui::streaming_history_commit(&self.state, width) else {
            return Ok(());
        };

        insert_history_lines(terminal, commit.lines)?;
        self.state
            .commit_streaming_prefix(commit.complete_entries, commit.active_byte_count);
        Ok(())
    }
}

fn resize_terminal(terminal: &mut AppTerminal, terminal_events: &mut TerminalEventReader) -> anyhow::Result<()> {
    with_terminal_events_paused(terminal_events, || clear_synchronized(terminal))
}

fn with_terminal_events_paused<T>(
    terminal_events: &mut TerminalEventReader,
    operation: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    terminal_events.stop();
    let result = operation();
    terminal_events.restart();
    result
}

fn application_help(commands: &[CommandSpec]) -> String {
    let mut output = String::from("Available commands:\n");
    for command in commands {
        output.push_str(&format!("  /{:<12} {}\n", command.name, command.description));
    }
    output.push_str(
        "\nInput and navigation:\n\
         Enter         Send message\n\
         Shift+Enter   Insert newline\n\
         Ctrl+C        Stop current turn, or quit when idle\n\
         Mouse wheel   Scroll conversation\n\
         Drag          Select terminal text\n\
         /             Show command menu",
    );
    output
}

fn is_key_press(key: KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

fn starts_conversation(input: &str) -> bool {
    !input.trim_start().starts_with('/')
}

enum IdleAction {
    Continue,
    Submit(String),
    Outcome(TuiOutcome),
    Exit,
}

enum CommandAction {
    PassThrough,
    Handled,
    Outcome(TuiOutcome),
}

fn permission_mode(value: &str) -> Option<SessionMode> {
    match value {
        "default" => Some(SessionMode::Default),
        "auto_edit" | "auto-edit" => Some(SessionMode::AutoEdit),
        "yolo" => Some(SessionMode::Yolo),
        _ => None,
    }
}

#[cfg(test)]
#[path = "app_test.rs"]
mod app_test;

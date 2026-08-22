use std::cmp::Reverse;
use std::env;
use std::io::{self, IsTerminal};
use std::sync::Arc;

use dream_engine_agent::engine::AgentEngine;
use dream_engine_agent::error::AgentError;
use dream_engine_agent::output::OutputSink;
use dream_engine_agent::output::terminal::TerminalSink;
use dream_engine_agent::session::{Session, SessionManager};
use dream_engine_config::config::{Config, SessionConfig};
use dream_engine_mcp::manager::McpManager;
use dream_engine_tui::{TuiMetadata, TuiOutcome, TuiRuntime, TuiSession};
use dream_engine_types::message::Message;

use crate::bootstrap::{build_engine, init_logging, resolve_config};
use crate::cli::Cli;
use crate::json_stream;

/// Entry point for the default (non-subcommand) invocation: validates
/// flags, resolves config/logging, then either dispatches to JSON stream
/// mode or bootstraps a terminal engine and runs a single prompt / REPL.
pub(crate) async fn run_main_flow(cli: Cli) -> anyhow::Result<()> {
    if cli.resume.is_some() && cli.session_id.is_some() {
        anyhow::bail!("Cannot use --resume and --session-id together");
    }

    let config = resolve_config(&cli)?;
    let _log_guard = init_logging(&config, cli.log_dir.as_deref(), cli.log_level.as_deref());

    let cwd = env::current_dir()?.to_string_lossy().to_string();

    // Branch to JSON stream mode
    if cli.json_stream {
        return json_stream::run(config, &cwd, cli.resume, cli.session_id, cli.fork_session).await;
    }

    let prompt = cli.prompt.join(" ");
    if prompt.is_empty() && io::stdin().is_terminal() && io::stdout().is_terminal() {
        return run_tui_flow(config, &cwd, &cli).await;
    }

    run_plain_flow(config, &cwd, &cli, &prompt).await
}

async fn run_tui_flow(config: Config, cwd: &str, cli: &Cli) -> anyhow::Result<()> {
    let mut active_config = config;
    let mut tui = TuiRuntime::new(TuiMetadata {
        model: active_config.model.clone(),
        provider: active_config.provider_label.clone(),
        cwd: cwd.to_string(),
        no_color: cli.no_color,
    });
    let output = tui.output_sink();
    tui.prepare_terminal()?;
    let mut runtime = build_tui_engine(
        active_config.clone(),
        cwd,
        output.clone(),
        cli.resume.as_deref(),
        cli.session_id.as_deref(),
        cli.fork_session,
    )
    .await?;
    configure_tui(&mut tui, &runtime, &active_config);

    loop {
        let outcome = tui.run(&mut runtime.engine).await?;
        let (resume_id, session_id) = match outcome {
            TuiOutcome::Exit => {
                shutdown(&runtime.engine, &runtime.mcp_managers).await;
                return Ok(());
            }
            TuiOutcome::NewSession => (None, None),
            TuiOutcome::ResumeSession(session_id) => (Some(session_id), None),
        };

        active_config.model = runtime.engine.context_status().model;
        let next = build_tui_engine(
            active_config.clone(),
            cwd,
            output.clone(),
            resume_id.as_deref(),
            session_id,
            false,
        )
        .await;
        match next {
            Ok(next) => {
                shutdown(&runtime.engine, &runtime.mcp_managers).await;
                runtime = next;
                configure_tui(&mut tui, &runtime, &active_config);
            }
            Err(error) => {
                tui.show_error(format!("Could not switch session: {error}"));
            }
        }
    }
}

struct TuiEngineRuntime {
    engine: AgentEngine,
    mcp_managers: Vec<Arc<McpManager>>,
    history: Vec<Message>,
    session_initialization_pending: bool,
    requested_session_id: Option<String>,
}

async fn build_tui_engine(
    config: Config,
    cwd: &str,
    output: Arc<dyn OutputSink>,
    resume_id: Option<&str>,
    session_id: Option<&str>,
    fork_session: bool,
) -> anyhow::Result<TuiEngineRuntime> {
    let mut history = None;
    let result = build_engine(config, cwd, output.clone(), resume_id, fork_session, |session| {
        history = Some(session.messages.clone());
    })
    .await?;
    Ok(TuiEngineRuntime {
        engine: result.engine,
        mcp_managers: result.mcp_managers,
        history: history.unwrap_or_default(),
        session_initialization_pending: resume_id.is_none(),
        requested_session_id: session_id.map(str::to_string),
    })
}

fn configure_tui(tui: &mut TuiRuntime, runtime: &TuiEngineRuntime, config: &Config) {
    let session_id = runtime.engine.current_session_id();
    tui.set_commands(runtime.engine.slash_commands());
    tui.reset_session(
        runtime.engine.context_status().model,
        config.provider_label.clone(),
        session_id.clone(),
        &runtime.history,
    );
    if runtime.session_initialization_pending {
        tui.defer_session_initialization(runtime.requested_session_id.clone());
    }
    tui.set_runtime_catalog(
        mcp_catalog(&runtime.mcp_managers),
        runtime.engine.skill_names().to_vec(),
        session_catalog(&config.session, session_id.as_deref()),
    );
}

fn mcp_catalog(managers: &[Arc<McpManager>]) -> Vec<String> {
    let mut servers = Vec::new();
    for manager in managers {
        let tools = manager.all_tools();
        for name in manager.server_names() {
            let tool_count = tools.iter().filter(|(server, _)| *server == name).count();
            servers.push(format!("{name} · {tool_count} tools"));
        }
    }
    servers.sort();
    servers
}

fn session_catalog(config: &SessionConfig, current_session_id: Option<&str>) -> Result<Vec<TuiSession>, String> {
    if !config.enabled {
        return Ok(Vec::new());
    }
    let manager = SessionManager::new(config.directory.clone().into(), config.max_sessions);
    manager.list().map_err(|error| error.to_string()).map(|mut sessions| {
        sessions.sort_by_key(|session| Reverse(session.updated_at));
        sessions
            .into_iter()
            .filter(|session| Some(session.id.as_str()) != current_session_id)
            .map(|session| {
                let summary = if session.summary.is_empty() {
                    "(empty session)".to_string()
                } else {
                    session.summary
                };
                TuiSession::new(
                    session.id,
                    session.model,
                    summary,
                    session.updated_at.format("%Y-%m-%d %H:%M UTC").to_string(),
                    session.message_count,
                )
            })
            .collect()
    })
}

async fn run_plain_flow(config: Config, cwd: &str, cli: &Cli, prompt: &str) -> anyhow::Result<()> {
    let terminal = Arc::new(TerminalSink::new(cli.no_color));
    let output: Arc<dyn OutputSink> = terminal.clone();

    let provider_name = config.provider_label.clone();
    let terminal_for_resume = terminal.clone();
    let fork_session = cli.fork_session;

    let result = build_engine(
        config,
        cwd,
        output.clone(),
        cli.resume.as_deref(),
        fork_session,
        |session| {
            let banner = resume_banner(session, fork_session);
            terminal_for_resume.formatter().session_info(&banner);
        },
    )
    .await?;
    let mut engine = result.engine;
    let mcp_managers = result.mcp_managers;

    if cli.resume.is_none() {
        engine.init_session(&provider_name, cwd, cli.session_id.as_deref())?;
    }

    if prompt.is_empty() {
        repl_loop(&mut engine, &terminal, &output).await?;
    } else {
        let run_result = engine.run(prompt, "").await?;
        output.emit_stream_end(
            "",
            run_result.turns,
            run_result.usage.input_tokens,
            run_result.usage.output_tokens,
            run_result.usage.cache_creation_tokens,
            run_result.usage.cache_read_tokens,
        );
    }

    shutdown(&engine, &mcp_managers).await;
    Ok(())
}

fn resume_banner(session: &Session, fork_session: bool) -> String {
    if fork_session {
        format!(
            "Forked session {} from {} ({} messages, {} model)",
            session.id,
            session.forked_from.as_deref().unwrap_or("?"),
            session.messages.len(),
            session.model
        )
    } else {
        format!(
            "Resumed session {} ({} messages, {} model)",
            session.id,
            session.messages.len(),
            session.model
        )
    }
}

async fn shutdown(engine: &AgentEngine, managers: &[Arc<McpManager>]) {
    engine.run_stop_hooks().await;
    for manager in managers {
        manager.shutdown().await;
    }
}

async fn repl_loop(
    engine: &mut AgentEngine,
    terminal: &Arc<TerminalSink>,
    output: &Arc<dyn OutputSink>,
) -> anyhow::Result<()> {
    use std::io::{self, BufRead};

    loop {
        terminal.formatter().repl_prompt();

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            break;
        }

        match engine.run(input, "").await {
            Ok(result) => {
                if result.turns > 0 {
                    output.emit_stream_end(
                        "",
                        result.turns,
                        result.usage.input_tokens,
                        result.usage.output_tokens,
                        result.usage.cache_creation_tokens,
                        result.usage.cache_read_tokens,
                    );
                }
            }
            Err(AgentError::UserAborted) => break,
            Err(e) => {
                output.emit_error(&e.to_string());
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "run_test.rs"]
mod run_test;

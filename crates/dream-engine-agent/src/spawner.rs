use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use futures::FutureExt;
use futures::future::join_all;

use dream_engine_config::config::Config;
use dream_engine_providers::LlmProvider;
use dream_engine_tools::edit::EditTool;
use dream_engine_tools::exec_command::ExecCommandTool;
use dream_engine_tools::glob::GlobTool;
use dream_engine_tools::grep::GrepTool;
use dream_engine_tools::read::ReadTool;
use dream_engine_tools::registry::ToolRegistry;
use dream_engine_tools::write::WriteTool;
use dream_engine_types::message::TokenUsage;

use crate::engine::AgentEngine;
use crate::output::OutputSink;
use crate::output::null_sink::NullSink;
use crate::tool_policy::ToolPolicy;

// Re-export from dream-types — single source of truth
pub use dream_engine_types::spawner::{ForkOverrides, Spawner, SubAgentConfig, SubAgentResult};

/// Spawns independent child agents that share the parent's LLM provider.
///
/// Sub-agents use a [`NullSink`] so their streaming output is silently
/// discarded.  Results are collected via `engine.run()` and returned to the
/// parent which emits them as a single `tool_result` event — matching the
/// Claude Code pattern where only the parent writes to stdout.
///
/// Children inherit the parent's runtime tool policy. Fork overrides can
/// further narrow that policy, but cannot restore tools denied to the parent.
pub struct AgentSpawner {
    provider: Arc<dyn LlmProvider>,
    base_config: Config,
    cwd: PathBuf,
    runtime_env: Vec<(String, String)>,
    tool_policy: ToolPolicy,
}

impl AgentSpawner {
    pub fn new(provider: Arc<dyn LlmProvider>, config: Config, cwd: PathBuf, tool_policy: ToolPolicy) -> Self {
        Self::new_with_env(provider, config, cwd, Vec::new(), tool_policy)
    }

    pub fn new_with_env(
        provider: Arc<dyn LlmProvider>,
        config: Config,
        cwd: PathBuf,
        runtime_env: Vec<(String, String)>,
        tool_policy: ToolPolicy,
    ) -> Self {
        Self {
            provider,
            base_config: config,
            cwd,
            runtime_env,
            tool_policy,
        }
    }

    /// Spawn a single sub-agent and wait for result.
    pub async fn spawn_one(&self, sub_config: SubAgentConfig) -> SubAgentResult {
        let mut config = self.base_config.clone();
        config.max_turns = Some(sub_config.max_turns);
        config.max_tokens = Some(sub_config.max_tokens);
        if let Some(sp) = sub_config.system_prompt.clone() {
            config.system_prompt = Some(sp);
        }
        config.session.enabled = false;
        config.tools.auto_approve = true;

        tracing::info!(target: "dream_engine_agent", cwd = %self.cwd.display(), "sub-agent spawned with workspace cwd");

        let child_policy = effective_child_tool_policy(&self.tool_policy, &[]);
        let tools = build_tool_registry(&child_policy, &self.cwd, &self.runtime_env);
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let mut engine = AgentEngine::new_with_provider_and_env(
            self.provider.clone(),
            config,
            tools,
            output,
            self.cwd.clone(),
            self.runtime_env.clone(),
        );
        engine.set_tool_policy(child_policy);

        match engine.run(&sub_config.prompt, "").await {
            Ok(result) => SubAgentResult {
                name: sub_config.name,
                text: result.text,
                usage: result.usage,
                turns: result.turns,
                is_error: false,
            },
            Err(e) => SubAgentResult {
                name: sub_config.name,
                text: format!("Sub-agent error: {}", e),
                usage: TokenUsage::default(),
                turns: 0,
                is_error: true,
            },
        }
    }

    /// Run children concurrently within the caller's future. Dropping or
    /// aborting that future drops every child, rather than detaching tasks.
    pub async fn spawn_parallel(&self, sub_configs: Vec<SubAgentConfig>) -> Vec<SubAgentResult> {
        join_all(sub_configs.into_iter().map(|config| async move {
            let name = config.name.clone();
            // Keep the previous per-child panic boundary without independent
            // Tokio tasks. Results retain input order, even on partial failure.
            match AssertUnwindSafe(self.spawn_one(config)).catch_unwind().await {
                Ok(result) => result,
                Err(_) => {
                    tracing::warn!(target: "dream_engine_agent", "sub-agent task panicked");
                    SubAgentResult {
                        name,
                        text: "Sub-agent task panicked".to_owned(),
                        usage: TokenUsage::default(),
                        turns: 0,
                        is_error: true,
                    }
                }
            }
        }))
        .await
    }
}

#[async_trait]
impl Spawner for AgentSpawner {
    async fn spawn_fork(&self, sub_config: SubAgentConfig, overrides: ForkOverrides) -> SubAgentResult {
        let mut config = self.base_config.clone();
        config.max_turns = Some(sub_config.max_turns);
        config.max_tokens = Some(sub_config.max_tokens);
        if let Some(sp) = sub_config.system_prompt.clone() {
            config.system_prompt = Some(sp);
        }
        config.session.enabled = false;
        config.tools.auto_approve = true;
        if let Some(model) = overrides.model.clone() {
            config.model = model;
        }

        let child_policy = effective_child_tool_policy(&self.tool_policy, &overrides.allowed_tools);
        let tools = build_tool_registry(&child_policy, &self.cwd, &self.runtime_env);
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let mut engine = AgentEngine::new_with_provider_and_env(
            self.provider.clone(),
            config,
            tools,
            output,
            self.cwd.clone(),
            self.runtime_env.clone(),
        );
        engine.set_initial_reasoning_effort(overrides.effort.clone());
        engine.set_tool_policy(child_policy);

        match engine.run(&sub_config.prompt, "").await {
            Ok(result) => SubAgentResult {
                name: sub_config.name,
                text: result.text,
                usage: result.usage,
                turns: result.turns,
                is_error: false,
            },
            Err(e) => SubAgentResult {
                name: sub_config.name,
                text: format!("Sub-agent error: {}", e),
                usage: TokenUsage::default(),
                turns: 0,
                is_error: true,
            },
        }
    }
}

fn effective_child_tool_policy(parent: &ToolPolicy, allowed_tools: &[String]) -> ToolPolicy {
    if allowed_tools.is_empty() {
        return parent.clone();
    }

    ToolPolicy::allow_only(
        allowed_tools
            .iter()
            .filter(|tool_name| parent.allows(tool_name))
            .cloned(),
    )
}

fn build_tool_registry(policy: &ToolPolicy, cwd: &Path, runtime_env: &[(String, String)]) -> ToolRegistry {
    let all_tools: Vec<(&str, Box<dyn dream_engine_tools::Tool>)> = vec![
        ("Read", Box::new(ReadTool::new(None))),
        ("Write", Box::new(WriteTool::new(None))),
        ("Edit", Box::new(EditTool::new(None))),
        (
            "ExecCommand",
            Box::new(ExecCommandTool::new_with_env(cwd.to_path_buf(), runtime_env.to_vec())),
        ),
        ("Grep", Box::new(GrepTool::new(cwd.to_path_buf()))),
        ("Glob", Box::new(GlobTool::new(cwd.to_path_buf()))),
    ];

    let mut registry = ToolRegistry::new();
    for (name, tool) in all_tools {
        if policy.allows(name) {
            registry.register(tool);
        }
    }
    registry
}

#[cfg(test)]
#[path = "spawner_test.rs"]
mod spawner_test;

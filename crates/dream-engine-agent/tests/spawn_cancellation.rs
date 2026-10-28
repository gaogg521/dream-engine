//! Parent cancellation owns all parallel sub-agent work.
mod common;

use std::future::pending;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use dream_engine_agent::spawner::{AgentSpawner, SubAgentConfig};
use dream_engine_agent::tool_policy::ToolPolicy;
use dream_engine_providers::{LlmProvider, ProviderError};
use dream_engine_types::llm::{LlmEvent, LlmRequest};
use dream_engine_types::message::{ContentBlock, StopReason, TokenUsage};
use tokio::sync::{Semaphore, mpsc};
use tokio::time::timeout;

struct ActiveRequest(Arc<AtomicUsize>);

impl Drop for ActiveRequest {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct WaitingProvider {
    active: Arc<AtomicUsize>,
    entered: Arc<Semaphore>,
}

#[async_trait]
impl LlmProvider for WaitingProvider {
    async fn stream(&self, _request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.active.fetch_add(1, Ordering::SeqCst);
        let _request = ActiveRequest(Arc::clone(&self.active));
        self.entered.add_permits(1);
        pending().await
    }
}

fn tasks() -> Vec<SubAgentConfig> {
    (0..5)
        .map(|i| SubAgentConfig {
            name: format!("child-{i}"),
            prompt: format!("wait-{i}"),
            max_turns: 2,
            max_tokens: 64,
            system_prompt: None,
        })
        .collect()
}

fn waiting_spawner() -> (Arc<AgentSpawner>, Arc<AtomicUsize>, Arc<Semaphore>) {
    let active = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Semaphore::new(0));
    let provider = Arc::new(WaitingProvider {
        active: Arc::clone(&active),
        entered: Arc::clone(&entered),
    });
    let spawner = AgentSpawner::new(
        provider,
        common::test_config(),
        std::env::temp_dir(),
        ToolPolicy::Unrestricted,
    );
    (Arc::new(spawner), active, entered)
}

#[tokio::test]
async fn dropping_parallel_spawn_cancels_all_five_inflight_requests() {
    let (spawner, active, entered) = waiting_spawner();
    let mut parent = Box::pin(spawner.spawn_parallel(tasks()));
    timeout(Duration::from_secs(5), async {
        tokio::select! {
            permits = entered.acquire_many(5) => permits.unwrap().forget(),
            _ = &mut parent => panic!("waiting children must not complete"),
        }
    })
    .await
    .expect("all five children must run concurrently");
    assert_eq!(active.load(Ordering::SeqCst), 5);
    drop(parent);
    assert_eq!(
        active.load(Ordering::SeqCst),
        0,
        "no detached child may survive its parent future"
    );
}

#[tokio::test]
async fn aborting_the_parent_task_finishes_all_child_requests_before_join_returns() {
    let (spawner, active, entered) = waiting_spawner();
    let parent = tokio::spawn(async move { spawner.spawn_parallel(tasks()).await });
    timeout(Duration::from_secs(5), entered.acquire_many(5))
        .await
        .expect("all children started")
        .unwrap()
        .forget();
    parent.abort();
    assert!(parent.await.unwrap_err().is_cancelled());
    assert_eq!(
        active.load(Ordering::SeqCst),
        0,
        "joining a cancelled parent must not leave child requests alive"
    );
}

struct CompletingProvider;

#[async_trait]
impl LlmProvider for CompletingProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let contains = |needle: &str| {
            request
                .messages
                .iter()
                .flat_map(|message| &message.content)
                .any(|block| matches!(block, ContentBlock::Text { text } if text.contains(needle)))
        };
        assert!(!contains("panic-child"), "test provider panic");
        if contains("slow-child") {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let (sender, receiver) = mpsc::channel(2);
        sender.send(LlmEvent::TextDelta("completed".into())).await.unwrap();
        sender
            .send(LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage::default(),
            })
            .await
            .unwrap();
        Ok(receiver)
    }
}

#[tokio::test]
async fn parallel_results_keep_input_order_and_isolate_a_panicking_child() {
    let spawner = AgentSpawner::new(
        Arc::new(CompletingProvider),
        common::test_config(),
        std::env::temp_dir(),
        ToolPolicy::Unrestricted,
    );
    let configs = ["slow-child", "panic-child", "fast-child"].map(|name| SubAgentConfig {
        name: name.into(),
        prompt: name.into(),
        max_turns: 2,
        max_tokens: 64,
        system_prompt: None,
    });
    let results = spawner.spawn_parallel(configs.into()).await;
    assert_eq!(
        results.iter().map(|result| result.name.as_str()).collect::<Vec<_>>(),
        ["slow-child", "panic-child", "fast-child"]
    );
    assert!(!results[0].is_error && !results[2].is_error);
    assert!(results[1].is_error);
}

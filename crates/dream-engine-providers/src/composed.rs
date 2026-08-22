use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use async_trait::async_trait;
#[cfg(test)]
use serde_json::Value;
use tokio::sync::mpsc;

use dream_engine_config::compat::ProviderCompat;
use dream_engine_types::llm::{LlmEvent, LlmRequest};

use crate::error::ProviderError;
use crate::provider::LlmProvider;
use crate::stream_runner::run_stream;
use crate::transport::ProviderTransport;

#[derive(Clone)]
pub(crate) struct ComposedProvider {
    transport: ProviderTransport,
    compat: ProviderCompat,
    /// Sticky replay-escalation level learned from gateway rejections.
    /// Shared across clones so later turns of the same session skip the
    /// levels that already failed instead of re-probing every turn.
    /// 0 = as configured, 1 = content[].thinking blocks, 2 = omit thinking,
    /// 3 = textualize tool replay.
    replay_level: Arc<AtomicU8>,
}

const MAX_REPLAY_LEVEL: u8 = 3;

fn compat_for_replay_level(base: &ProviderCompat, level: u8) -> ProviderCompat {
    match level {
        0 => base.clone(),
        1 => base.with_thinking_replay_as_content_block(),
        2 => base.with_thinking_replay_omitted(),
        _ => base.with_textualized_tool_replay(),
    }
}

impl ComposedProvider {
    pub(crate) fn new(transport: ProviderTransport, compat: ProviderCompat) -> Self {
        Self {
            transport,
            compat,
            replay_level: Arc::new(AtomicU8::new(0)),
        }
    }

    #[cfg(test)]
    pub(crate) fn build_request_body(&self, request: &LlmRequest) -> Result<Value, ProviderError> {
        let (body, _) = self.transport.project_body(request, &self.compat)?;
        Ok(body)
    }
}

/// Some OpenAI-protocol gateways front a thinking-capable model through an
/// Anthropic-shaped conversion layer that cannot round-trip thinking/tool
/// history: they 400 with "content[].thinking ... must be passed back"
/// regardless of what the client actually sent. Detect that rejection so
/// `stream()` can escalate through alternative replay shapes.
fn is_thinking_replay_format_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("content[].thinking") && lower.contains("passed back")
}

impl ComposedProvider {
    async fn stream_with_compat(
        &self,
        request: &LlmRequest,
        compat: &ProviderCompat,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let (body, tool_wire_shape) = self.transport.project_body(request, compat)?;

        tracing::debug!(
            target: "dream_engine_providers",
            model = %request.model,
            message_count = request.messages.len(),
            tool_count = request.tools.len(),
            max_tokens = ?request.max_tokens,
            thinking_configured = request.thinking.is_some(),
            "provider request projected"
        );

        let transport = self.transport.clone();
        let compat_owned = compat.clone();
        let model = request.model.clone();
        let send = move || {
            let transport = transport.clone();
            let compat = compat_owned.clone();
            let body = body.clone();
            let model = model.clone();
            async move {
                let projected_request = transport.build_projected_request(&model, body, &compat, tool_wire_shape)?;
                transport.send(projected_request).await
            }
        };

        let decoder = self.transport.decoder(compat);
        let process = move |response, tx| async move { decoder.process(response, &tx).await };
        let retry_policy = self.transport.retry_policy();

        run_stream(send, process, retry_policy).await
    }
}

#[async_trait]
impl LlmProvider for ComposedProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let mut level = self.replay_level.load(Ordering::Relaxed);
        loop {
            let compat = compat_for_replay_level(&self.compat, level);
            match self.stream_with_compat(request, &compat).await {
                Err(ProviderError::Api { message, .. })
                    if level < MAX_REPLAY_LEVEL && is_thinking_replay_format_error(&message) =>
                {
                    level += 1;
                    tracing::warn!(
                        target: "dream_engine_providers",
                        replay_level = level,
                        "gateway rejected thinking/tool replay shape; escalating (1=content-block thinking, 2=omit thinking, 3=textualize tool history)"
                    );
                }
                Ok(rx) => {
                    // Remember what worked so subsequent turns in this
                    // session skip the shapes the gateway already rejected.
                    if level != self.replay_level.load(Ordering::Relaxed) {
                        self.replay_level.store(level, Ordering::Relaxed);
                    }
                    return Ok(rx);
                }
                Err(e) => return Err(e),
            }
        }
    }
}

#[cfg(test)]
#[path = "composed_test.rs"]
mod composed_test;

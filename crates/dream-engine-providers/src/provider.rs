use std::env;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use dream_engine_config::config::{Config, ProviderType};
use dream_engine_types::llm::{LlmEvent, LlmRequest};

use crate::anthropic;
use crate::bedrock;
use crate::error::ProviderError;
use crate::ollama;
use crate::openai;
use crate::vertex;

/// Unified interface for LLM API providers
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError>;
}

/// Create a provider from resolved config
pub fn create_provider(config: &Config) -> Arc<dyn LlmProvider> {
    let compat = config.compat.clone();

    match config.provider {
        ProviderType::Anthropic => Arc::new(
            anthropic::AnthropicProvider::new(&config.api_key, &config.base_url, compat)
                .with_cache(config.prompt_caching),
        ),
        ProviderType::OpenAI => Arc::new(openai::OpenAIProvider::new(&config.api_key, &config.base_url, compat)),
        ProviderType::Ollama => Arc::new(ollama::OllamaProvider::new(&config.api_key, &config.base_url, compat)),
        ProviderType::Bedrock => {
            let bc = config.bedrock.clone().unwrap_or_default();
            let region = bc
                .region
                .clone()
                .or_else(|| env::var("AWS_REGION").ok())
                .or_else(|| env::var("AWS_DEFAULT_REGION").ok())
                .unwrap_or_else(|| "us-east-1".to_string());
            let credentials = bedrock::credentials_from_config(&bc);
            let (base_url, bearer_token) = endpoint_override(&config.base_url, &config.api_key);
            Arc::new(bedrock::BedrockProvider::new(
                &region,
                credentials,
                config.prompt_caching,
                compat,
                base_url,
                bearer_token,
            ))
        }
        ProviderType::Vertex => {
            let vc = config.vertex.clone().unwrap_or_default();
            let project_id = vc.project_id.clone().unwrap_or_default();
            let region = vc.region.clone().unwrap_or_else(|| "us-central1".to_string());
            let auth = vertex::auth_from_config(&vc);
            let (base_url, bearer_token) = endpoint_override(&config.base_url, &config.api_key);
            Arc::new(vertex::VertexProvider::new(
                &project_id,
                &region,
                auth,
                config.prompt_caching,
                compat,
                base_url,
                bearer_token,
            ))
        }
    }
}

/// Endpoint override for Bedrock/Vertex, mirroring what `config.base_url`
/// means for the other providers (where the enterprise model proxy already
/// works).
///
/// `default_base_url` yields the empty string for these two providers — their
/// URLs are built from region/project — so empty means "no override". The
/// bearer token activates only when BOTH the base URL and the api key are
/// present: that combination is the enterprise model proxy (channel token as
/// `Authorization: Bearer`, no SigV4/OAuth on the caller side), while an api
/// key alone must never flip a direct-connection deployment off its native
/// auth (e.g. a stray `API_KEY` env var reaching `resolve_api_key`).
fn endpoint_override(base_url: &str, api_key: &str) -> (Option<String>, Option<String>) {
    let base_url = (!base_url.is_empty()).then(|| base_url.to_owned());
    let bearer_token = base_url
        .as_ref()
        .and_then(|_| (!api_key.is_empty()).then(|| api_key.to_owned()));
    (base_url, bearer_token)
}

#[cfg(test)]
#[path = "provider_test.rs"]
mod provider_test;

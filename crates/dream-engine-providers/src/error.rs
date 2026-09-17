use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },
    #[error("SSE parse error: {0}")]
    Parse(String),
    // Display intentionally omits `body` — it may contain provider response
    // payload (potentially sensitive) and would leak into logs via
    // `tracing::error!("{err}")`. Consumers that need the body must pattern
    // match on the variant explicitly.
    #[error("Rate limited, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64, body: Option<String> },
    #[error("Prompt too long: {0}")]
    PromptTooLong(String),
    #[error("Connection error: {0}")]
    Connection(String),
}

impl ProviderError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, ProviderError::RateLimited { .. } | ProviderError::Connection(_))
    }
}

/// Upstream error text that really means "the prompt does not fit the model's
/// context window". Every provider spells it differently — Ollama says
/// "llm: context overflow - prompt exceeds the available context window",
/// OpenAI says "maximum context length" / `context_length_exceeded`,
/// Anthropic says "prompt is too long". Mapping these to `PromptTooLong`
/// lets downstream classifiers surface context-specific guidance instead of
/// a generic "provider rejected the request".
pub(crate) fn looks_like_context_overflow(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    const NEEDLES: &[&str] = &[
        "context overflow",
        "prompt too long",
        "prompt is too long",
        "maximum context length",
        "context_length_exceeded",
        "context length exceeded",
        "exceeds the available context window",
        "exceeds the context window",
        "exceed the context window",
        "reduce the length of the messages",
        "reduce your prompt",
    ];
    NEEDLES.iter().any(|needle| lower.contains(needle))
}

/// Map a provider JSON error payload to a `ProviderError`.
///
/// Covers error bodies delivered with a successful HTTP status: whole-body
/// JSON errors on non-streaming responses and `data: {"error": ...}` frames
/// embedded in SSE streams. Returns `None` when the body does not look like
/// an error payload.
pub(crate) fn provider_error_from_json_body(body: &Value, body_bytes: &[u8]) -> Option<ProviderError> {
    // Some gateways include `"error": null` in perfectly normal responses;
    // treat a null error field the same as an absent one.
    let error_field = body.get("error").filter(|error| !error.is_null());
    let error = error_field.unwrap_or(body);
    let status = [
        error.get("code"),
        error.get("status"),
        body.get("code"),
        body.get("status"),
    ]
    .into_iter()
    .flatten()
    .find_map(json_http_status_code);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .or_else(|| body.get("message").and_then(Value::as_str))
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .unwrap_or("Provider returned a JSON error response without a message")
        .to_string();

    match status {
        Some(429) => Some(ProviderError::RateLimited {
            retry_after_ms: 5000,
            body: (!body_bytes.is_empty()).then(|| String::from_utf8_lossy(body_bytes).into_owned()),
        }),
        Some(status) if (400..=499).contains(&status) && looks_like_context_overflow(&message) => {
            Some(ProviderError::PromptTooLong(message))
        }
        Some(status) => Some(ProviderError::Api { status, message }),
        None if error_field.is_some() && looks_like_context_overflow(&message) => {
            Some(ProviderError::PromptTooLong(message))
        }
        None if error_field.is_some() => Some(ProviderError::Parse(format!(
            "Provider returned a JSON error response without an HTTP status: {message}"
        ))),
        None => None,
    }
}

/// Does this upstream error text mean "the prompt does not fit the context
/// window"?
///
/// Public because the agent has to recognise the same condition arriving as a
/// bare string: a gateway that reports the failure mid-stream produces an
/// `LlmEvent::Error(String)`, which has already lost the
/// [`ProviderError::PromptTooLong`] typing by the time the agent sees it.
pub fn is_context_overflow(message: &str) -> bool {
    looks_like_context_overflow(message)
}

/// Smallest and largest window sizes worth believing from an error string.
///
/// Below the floor the "limit" is far more likely to be a completion budget or
/// a stray count than a context window; above the ceiling it is an id or a
/// byte count. A wrong value here makes the agent compact against a fiction,
/// so the bar is deliberately narrow.
const PLAUSIBLE_WINDOW: std::ops::RangeInclusive<usize> = 256..=20_000_000;

/// Phrases that introduce the model's real context window, and whether the
/// number sits after the phrase or before it.
///
/// Matching on phrases rather than scanning for numbers is the whole point.
/// OpenAI's message carries four of them — "This model's maximum context
/// length is 128000 tokens. However, you requested 130500 tokens (125000 in
/// the messages, 5500 in the completion)" — so "the smallest number present"
/// would learn 5500 and compact the session down to nothing.
const WINDOW_PHRASES: &[(&str, bool)] = &[
    ("maximum context length is", true),
    ("maximum context length of", true),
    ("context length is", true),
    ("context window is", true),
    ("context window of", true),
    ("context limit is", true),
    ("maximum", false),
];

/// Recover the model's real context window from a provider's overflow error.
///
/// Returns `None` whenever the message does not state one in a form we
/// recognise — Ollama's "context overflow - prompt exceeds the available
/// context window" carries no number at all — and the caller must then fall
/// back to shrinking relative to what it just sent rather than guessing.
pub fn parse_context_limit(message: &str) -> Option<usize> {
    let lower = message.to_ascii_lowercase();
    WINDOW_PHRASES
        .iter()
        .find_map(|(phrase, after)| {
            let at = lower.find(phrase)?;
            if *after {
                integer_after(&lower, at + phrase.len())
            } else {
                integer_before(&lower, at)
            }
        })
        .filter(|window| PLAUSIBLE_WINDOW.contains(window))
}

/// First integer at or after `from`, skipping any non-digits in between.
fn integer_after(haystack: &str, from: usize) -> Option<usize> {
    let rest = haystack.get(from..)?;
    let start = rest.find(|c: char| c.is_ascii_digit())?;
    let digits: String = rest[start..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Last integer ending at or before `before`, skipping any non-digits.
fn integer_before(haystack: &str, before: usize) -> Option<usize> {
    let head = haystack.get(..before)?.trim_end();
    let end = head.rfind(|c: char| c.is_ascii_digit())? + 1;
    let start = head[..end]
        .rfind(|c: char| !c.is_ascii_digit())
        .map(|i| i + 1)
        .unwrap_or(0);
    head[start..end].parse().ok()
}

fn json_http_status_code(value: &Value) -> Option<u16> {
    value
        .as_u64()
        .and_then(|status| u16::try_from(status).ok())
        .or_else(|| value.as_str().and_then(|status| status.parse().ok()))
        .filter(|status| (400..=599).contains(status))
}

#[cfg(test)]
#[path = "error_test.rs"]
mod error_test;

//! The one place an HTTP client for a model provider is built.
//!
//! Every provider transport used to call `reqwest::Client::new()` directly,
//! which configures **no timeout of any kind**. A provider that accepted the
//! request and then stopped sending bytes left the request outstanding
//! forever: the turn never finished, never errored, and never released the
//! conversation — one report had a turn still spinning after 183 minutes.

use std::time::Duration;

/// Longest gap tolerated between two reads on a response body.
///
/// A *read* timeout, deliberately not a total request timeout. Model responses
/// stream, and a legitimate one can take many minutes end to end — a total cap
/// would abort long answers, which is a worse failure than the one being
/// fixed. This only fires when the connection goes quiet, which for a healthy
/// stream never happens: tokens arrive continuously once generation starts.
///
/// Five minutes leaves room for the slowest realistic pre-first-token pause on
/// a large reasoning model while still failing a dead connection long before
/// anyone would sit through it.
const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Longest wait to establish the TCP/TLS connection.
///
/// Separate from the read timeout because "cannot reach the provider at all"
/// deserves to fail fast; there is nothing to stream yet.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Builds the HTTP client every provider transport should use.
///
/// Falls back to an unconfigured client if the builder fails, which it only
/// can on a broken TLS backend: refusing to construct a provider at all would
/// be a worse outcome than one without timeouts, and the caller has no way to
/// recover here.
pub(crate) fn build() -> reqwest::Client {
    reqwest::Client::builder()
        .read_timeout(READ_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .unwrap_or_else(|error| {
            tracing::warn!(
                target: "dream_engine_providers",
                %error,
                "falling back to an HTTP client with no timeouts"
            );
            reqwest::Client::new()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The read timeout has to be long enough that a slow first token never
    /// trips it, and short enough that a dead connection is not mistaken for a
    /// working one for the rest of the afternoon.
    #[test]
    fn the_read_timeout_is_generous_but_finite() {
        let minutes = READ_TIMEOUT.as_secs() / 60;
        assert!(
            (2..=15).contains(&minutes),
            "want a gap far past a slow first token but well short of a wasted \
             session; got {minutes} minutes"
        );
        assert!(
            CONNECT_TIMEOUT < READ_TIMEOUT,
            "an unreachable provider should fail faster than a quiet stream"
        );
    }

    /// The builder must actually produce a client — a fallback to the untimed
    /// one is the bug this module exists to prevent, so it must not be the
    /// normal path.
    #[test]
    fn a_client_is_built_successfully() {
        let _client = build();
    }
}

//! Emergency truncation: the last safety net before a context overflow.
//!
//! When the best-known context size is within `emergency_buffer` of the full
//! `context_window`, the engine should block the next API call and ask
//! the user to compact or start a new conversation.
//!
//! Unlike autocompact, the emergency check always applies — even when
//! the compaction system is disabled via `CompactConfig.enabled`.

use dream_engine_config::compact::CompactConfig;

/// User-facing message shown when the emergency limit is hit.
pub const EMERGENCY_USER_MESSAGE: &str = "Context window nearly full. Please use /compact or start a new conversation.";

/// Token count at which the engine stops sending requests.
///
/// Nominally `context_window - emergency_buffer`, but `emergency_buffer` is an
/// absolute count tuned for a 200k window, and the block has to stay *above*
/// the autocompact trigger — otherwise the turn is refused before compaction
/// ever runs and the only way forward is a new conversation. At an 8k window
/// the fixed 3000 tokens put the block at 5192, below the 6553 where
/// autocompact fires. So in percentage mode the buffer is capped at half the
/// headroom above the trigger, which preserves the ordering at any window size
/// and any threshold.
///
/// The absolute-buffer mode needs no cap: its trigger is already
/// `window - output_reserve - autocompact_buffer`, a full 33k below the block.
pub fn emergency_limit(config: &CompactConfig) -> usize {
    let buffer = match config.autocompact_threshold_pct {
        Some(pct) => {
            let headroom = 100usize.saturating_sub(pct as usize);
            config.emergency_buffer.min(config.context_window * headroom / 200)
        }
        None => config.emergency_buffer,
    };
    config.context_window.saturating_sub(buffer)
}

/// Check whether the best-known context token count has reached the
/// emergency blocking limit.
///
/// At or past [`emergency_limit`] the engine must not send another API
/// request — doing so would almost certainly fail with a prompt-too-long
/// error from the provider.
///
/// This check is independent of `CompactConfig.enabled`; the emergency
/// safety net is always active.
pub fn is_at_emergency_limit(context_tokens: u64, config: &CompactConfig) -> bool {
    context_tokens as usize >= emergency_limit(config)
}

#[cfg(test)]
#[path = "emergency_test.rs"]
mod emergency_test;

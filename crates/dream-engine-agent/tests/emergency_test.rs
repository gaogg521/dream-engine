//! Black-box integration tests for emergency truncation (TC-2.5-01 .. TC-2.5-04).
//!
//! These tests treat `is_at_emergency_limit` as a public API and verify
//! functional requirements from test-plan.md without relying on internal details.

use dream_engine_agent::compact::emergency::{EMERGENCY_USER_MESSAGE, emergency_limit, is_at_emergency_limit};
use dream_engine_config::compact::CompactConfig;

// ── TC-2.5-01: Below emergency threshold ───────────────────────────────────

#[test]
fn tc_2_5_01_below_emergency_threshold() {
    // context_window=1_000_000, emergency_buffer=3_000
    // emergency_limit = 1M - 3k = 997k
    // 990k < 997k → false
    let config = CompactConfig::default();
    assert!(
        !is_at_emergency_limit(990_000, &config),
        "990k tokens should be below the 997k emergency limit"
    );
}

// ── TC-2.5-02: Above emergency threshold ───────────────────────────────────

#[test]
fn tc_2_5_02_above_emergency_threshold() {
    // 998k >= 997k → true
    let config = CompactConfig::default();
    assert!(
        is_at_emergency_limit(998_000, &config),
        "998k tokens should exceed the 997k emergency limit"
    );
}

// ── TC-2.5-03: Exactly at emergency threshold ──────────────────────────────

#[test]
fn tc_2_5_03_at_exact_emergency_threshold() {
    // 997k >= 997k → true
    let config = CompactConfig::default();
    assert!(
        is_at_emergency_limit(997_000, &config),
        "997k tokens should trigger at exactly the emergency limit"
    );
}

// ── TC-2.5-04: Small context window ────────────────────────────────────────

#[test]
fn tc_2_5_04_small_context_window() {
    // context_window=8_000, emergency_buffer=3_000 capped to 800 (half the
    // headroom above the 6.4k autocompact trigger), so emergency_limit = 7.2k.
    // The uncapped 5k limit sat BELOW the trigger: the turn was refused before
    // compaction could run and the session was unrecoverable.
    let config = CompactConfig {
        context_window: 8_000,
        emergency_buffer: 3_000,
        ..CompactConfig::default()
    };
    assert_eq!(emergency_limit(&config), 7_200);
    assert!(
        !is_at_emergency_limit(6_400, &config),
        "the autocompact trigger must not itself be a hard block"
    );
    assert!(
        is_at_emergency_limit(7_200, &config),
        "7.2k tokens should hit the emergency limit on an 8k context window"
    );
}

// ── Additional integration-level checks ────────────────────────────────────

#[test]
fn emergency_check_ignores_enabled_flag() {
    // Emergency is the safety net — it fires even when compact is disabled
    let config = CompactConfig {
        enabled: false,
        ..CompactConfig::default()
    };
    assert!(
        is_at_emergency_limit(998_000, &config),
        "emergency check must fire regardless of the enabled flag"
    );
}

#[test]
fn user_message_is_actionable() {
    // The message should tell the user what to do
    assert!(
        EMERGENCY_USER_MESSAGE.contains("/compact"),
        "emergency message should mention /compact"
    );
    assert!(
        EMERGENCY_USER_MESSAGE.contains("new conversation"),
        "emergency message should mention starting a new conversation"
    );
}

#[test]
fn autocompact_fires_before_emergency() {
    // Verify that the autocompact threshold is lower than the emergency limit
    // so autocompact gets a chance to run before the safety net kicks in.
    use dream_engine_agent::compact::auto::should_autocompact;

    let config = CompactConfig::default();

    // Pick a token count that triggers autocompact but not emergency
    let token_count: u64 = 810_000;
    let autocompact_triggers = should_autocompact(token_count, &config);
    let emergency_triggers = is_at_emergency_limit(token_count, &config);

    assert!(
        autocompact_triggers && !emergency_triggers,
        "at 810k tokens, autocompact should trigger (threshold 800k) \
         but emergency should not (limit 997k)"
    );
}

#[test]
fn both_trigger_near_limit() {
    // When very close to the limit, both autocompact and emergency should fire
    use dream_engine_agent::compact::auto::should_autocompact;

    let config = CompactConfig::default();
    let token_count: u64 = 998_000;

    assert!(should_autocompact(token_count, &config));
    assert!(is_at_emergency_limit(token_count, &config));
}

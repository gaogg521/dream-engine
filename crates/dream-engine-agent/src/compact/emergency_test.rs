use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> CompactConfig {
        CompactConfig::default()
    }

    // ── is_at_emergency_limit ──────────────────────────────────────────

    #[test]
    fn below_limit_returns_false() {
        // limit = 1M - 3k = 997k; 990k < 997k
        let config = default_config();
        assert!(!is_at_emergency_limit(990_000, &config));
    }

    #[test]
    fn above_limit_returns_true() {
        // 998k >= 997k
        let config = default_config();
        assert!(is_at_emergency_limit(998_000, &config));
    }

    #[test]
    fn at_exact_limit_returns_true() {
        // 997k >= 997k
        let config = default_config();
        assert!(is_at_emergency_limit(997_000, &config));
    }

    #[test]
    fn small_context_window_still_leaves_room_to_compact() {
        // The regression: a fixed 3000-token buffer is 37% of an 8k window, so
        // the block landed at 5000 — BELOW the 6400 where autocompact fires.
        // The turn was refused before compaction ever ran and the only way
        // forward was a new conversation.
        let config = CompactConfig {
            context_window: 8_000,
            emergency_buffer: 3_000,
            ..default_config()
        };
        assert_eq!(emergency_limit(&config), 7_200);
        assert!(
            !is_at_emergency_limit(6_400, &config),
            "the autocompact trigger must not be a hard block"
        );
        assert!(is_at_emergency_limit(7_200, &config));
    }

    #[test]
    fn the_block_stays_above_the_autocompact_trigger_at_every_window_size() {
        for window in [1_000usize, 4_096, 8_192, 60_000, 200_000, 1_000_000] {
            for pct in [10u8, 50, 80, 95] {
                let config = CompactConfig {
                    context_window: window,
                    autocompact_threshold_pct: Some(pct),
                    ..default_config()
                };
                let trigger = window * pct as usize / 100;
                assert!(
                    trigger < emergency_limit(&config),
                    "window {window} at {pct}%: trigger {trigger} must stay below block {}",
                    emergency_limit(&config)
                );
            }
        }
    }

    #[test]
    fn zero_tokens_below_limit() {
        let config = default_config();
        assert!(!is_at_emergency_limit(0, &config));
    }

    #[test]
    fn custom_emergency_buffer() {
        let config = CompactConfig {
            context_window: 100_000,
            emergency_buffer: 10_000,
            ..default_config()
        };
        // 10k is exactly half the headroom above the 80k trigger, so the cap
        // leaves it alone.
        // limit = 100k - 10k = 90k
        assert!(!is_at_emergency_limit(89_999, &config));
        assert!(is_at_emergency_limit(90_000, &config));
        assert!(is_at_emergency_limit(95_000, &config));
    }

    #[test]
    fn works_regardless_of_enabled_flag() {
        let config = CompactConfig {
            enabled: false,
            ..default_config()
        };
        // Emergency check ignores the enabled flag
        assert!(is_at_emergency_limit(998_000, &config));
    }

    #[test]
    fn emergency_buffer_larger_than_context_window_saturates() {
        let config = CompactConfig {
            context_window: 1_000,
            emergency_buffer: 5_000,
            // Absolute-buffer mode: nothing caps the buffer here, so the
            // oversized one has to saturate rather than underflow.
            autocompact_threshold_pct: None,
            ..default_config()
        };
        // saturating_sub: limit = 0; any positive token count triggers
        assert!(is_at_emergency_limit(1, &config));
        // 0 tokens = 0 >= 0 → true (degenerate but safe)
        assert!(is_at_emergency_limit(0, &config));
    }

    #[test]
    fn an_oversized_buffer_is_capped_rather_than_saturated_in_percentage_mode() {
        // Same config in the default mode: capping to half the headroom keeps
        // a 1k window usable instead of blocking every turn at zero tokens.
        let config = CompactConfig {
            context_window: 1_000,
            emergency_buffer: 5_000,
            ..default_config()
        };
        assert_eq!(emergency_limit(&config), 900);
        assert!(!is_at_emergency_limit(0, &config));
    }

    // ── EMERGENCY_USER_MESSAGE ─────────────────────────────────────────

    #[test]
    fn user_message_mentions_compact() {
        assert!(EMERGENCY_USER_MESSAGE.contains("/compact"));
    }

    #[test]
    fn user_message_mentions_new_conversation() {
        assert!(EMERGENCY_USER_MESSAGE.contains("new conversation"));
    }
}

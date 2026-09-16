use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use dream_engine_types::compact::CompactTrigger;

    fn default_config() -> CompactConfig {
        CompactConfig::default()
    }

    // ── should_autocompact (TC-2.4-01..03, TC-2.4-14) ──────────────────

    #[test]
    fn above_threshold_triggers() {
        // Default is percentage mode: threshold = 1M * 80% = 800k.
        let config = default_config();
        assert!(should_autocompact(810_000, &config));
    }

    #[test]
    fn below_threshold_does_not_trigger() {
        let config = default_config();
        assert!(!should_autocompact(790_000, &config));
    }

    #[test]
    fn at_exact_threshold_triggers() {
        let config = default_config();
        assert!(should_autocompact(800_000, &config));
    }

    #[test]
    fn default_threshold_is_a_share_of_whatever_window_is_configured() {
        // The point of the percentage default: the same config is correct at a
        // 4k local window and at 1M. The absolute-buffer formula is not — it
        // underflows to zero below 33k and compacts every single turn.
        for window in [4_096usize, 8_192, 60_000, 200_000, 1_000_000] {
            let config = CompactConfig {
                context_window: window,
                ..default_config()
            };
            let threshold = window * 80 / 100;
            assert!(!should_autocompact(threshold as u64 - 1, &config), "window {window}");
            assert!(should_autocompact(threshold as u64, &config), "window {window}");
        }
    }

    #[test]
    fn disabled_config_never_triggers() {
        let config = CompactConfig {
            enabled: false,
            ..default_config()
        };
        assert!(!should_autocompact(999_999, &config));
    }

    #[test]
    fn custom_config_threshold() {
        let config = CompactConfig {
            context_window: 100_000,
            output_reserve: 10_000,
            autocompact_buffer: 5_000,
            // Absolute-buffer mode is opt-in now; clearing the percentage is
            // what selects it.
            autocompact_threshold_pct: None,
            ..default_config()
        };
        // threshold = 100k - 10k - 5k = 85k
        assert!(!should_autocompact(80_000, &config));
        assert!(should_autocompact(85_000, &config));
        assert!(should_autocompact(90_000, &config));
    }

    #[test]
    fn zero_tokens_does_not_trigger() {
        let config = default_config();
        assert!(!should_autocompact(0, &config));
    }

    #[test]
    fn threshold_pct_overrides_default_calculation() {
        let config = CompactConfig {
            context_window: 200_000,
            autocompact_threshold_pct: Some(50),
            ..default_config()
        };
        // threshold = 200k * 50 / 100 = 100k
        assert!(!should_autocompact(99_999, &config));
        assert!(should_autocompact(100_000, &config));
        assert!(should_autocompact(150_000, &config));
    }

    #[test]
    fn threshold_pct_zero_triggers_immediately() {
        let config = CompactConfig {
            autocompact_threshold_pct: Some(0),
            ..default_config()
        };
        // threshold = 0, any non-negative triggers
        assert!(should_autocompact(0, &config));
        assert!(should_autocompact(1, &config));
    }

    #[test]
    fn threshold_pct_100_never_triggers() {
        let config = CompactConfig {
            context_window: 200_000,
            autocompact_threshold_pct: Some(100),
            ..default_config()
        };
        // threshold = 200k, provider never reports 200k input_tokens
        assert!(!should_autocompact(199_999, &config));
        assert!(should_autocompact(200_000, &config));
    }

    #[test]
    fn threshold_pct_none_uses_default_logic() {
        let config = CompactConfig {
            autocompact_threshold_pct: None,
            ..default_config()
        };
        // Absolute-buffer formula against the 1M default window:
        // threshold = 1M - 20k - 13k = 967k
        assert!(!should_autocompact(966_999, &config));
        assert!(should_autocompact(967_000, &config));
    }

    // ── truncate_for_retry ──────────────────────────────────────────────

    #[test]
    fn truncate_drops_20_percent() {
        let msgs: Vec<Message> = (0..10)
            .map(|i| {
                let role = if i % 2 == 0 { Role::User } else { Role::Assistant };
                Message::new(
                    role,
                    vec![ContentBlock::Text {
                        text: format!("msg-{i}"),
                    }],
                )
            })
            .collect();

        let result = truncate_for_retry(&msgs).unwrap();
        // Drop 20% of 10 = 2 messages, remaining 8
        assert_eq!(result.len(), 8);
    }

    #[test]
    fn truncate_ensures_user_first() {
        let msgs: Vec<Message> = (0..5)
            .map(|i| {
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::Text {
                        text: format!("msg-{i}"),
                    }],
                )
            })
            .collect();

        let result = truncate_for_retry(&msgs).unwrap();
        assert_eq!(result[0].role, Role::User);
    }

    #[test]
    fn truncate_too_few_returns_none() {
        let msgs = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "only one".to_string(),
            }],
        )];
        assert!(truncate_for_retry(&msgs).is_none());
    }

    #[test]
    fn truncate_empty_returns_none() {
        assert!(truncate_for_retry(&[]).is_none());
    }

    #[test]
    fn truncate_preserves_user_first_without_placeholder() {
        // First remaining message is already User — no placeholder needed
        let msgs: Vec<Message> = (0..10)
            .map(|i| {
                let role = if i % 2 == 0 { Role::User } else { Role::Assistant };
                Message::new(
                    role,
                    vec![ContentBlock::Text {
                        text: format!("msg-{i}"),
                    }],
                )
            })
            .collect();

        let result = truncate_for_retry(&msgs).unwrap();
        // msgs[2] (User) should be first; no placeholder prepended
        assert_eq!(result.len(), 8);
        match &result[0].content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "msg-2"),
            _ => panic!("expected Text"),
        }
    }

    // ── boundary detection / extraction ─────────────────────────────────

    #[test]
    fn detect_boundary_message() {
        let metadata = CompactMetadata {
            trigger: CompactTrigger::Auto,
            pre_compact_tokens: 150_000,
            messages_summarized: 42,
        };
        let text = format!("{BOUNDARY_PREFIX}\n{}", serde_json::to_string(&metadata).unwrap());
        let msg = Message::new(Role::User, vec![ContentBlock::Text { text }]);
        assert!(is_compact_boundary(&msg));
    }

    #[test]
    fn non_boundary_message() {
        let msg = Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
        );
        assert!(!is_compact_boundary(&msg));
    }

    #[test]
    fn extract_metadata_from_boundary() {
        let metadata = CompactMetadata {
            trigger: CompactTrigger::Auto,
            pre_compact_tokens: 150_000,
            messages_summarized: 42,
        };
        let text = format!("{BOUNDARY_PREFIX}\n{}", serde_json::to_string(&metadata).unwrap());
        let msg = Message::new(Role::User, vec![ContentBlock::Text { text }]);
        let extracted = extract_compact_metadata(&msg).unwrap();
        assert_eq!(extracted, metadata);
    }

    #[test]
    fn extract_metadata_from_non_boundary_returns_none() {
        let msg = Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "not a boundary".to_string(),
            }],
        );
        assert!(extract_compact_metadata(&msg).is_none());
    }
}

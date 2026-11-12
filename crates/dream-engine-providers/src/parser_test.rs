use super::*;

#[cfg(test)]
mod tests {
    use super::{AnthropicParser, OpenAiParser, ResponseParser};
    use crate::framing::{Frame, FrameKind};
    use dream_engine_types::llm::LlmEvent;
    use dream_engine_types::message::StopReason;

    #[test]
    fn openai_done_frame_flushes_empty_state_to_no_events() {
        let parser = OpenAiParser { auto_tool_id: false };
        let mut state = parser.new_state();
        let frame = Frame {
            event: None,
            data: "[DONE]".to_string(),
            kind: FrameKind::Done,
        };

        let events = parser.parse_frame(&frame, &mut state);

        assert!(events.is_empty());
    }

    #[test]
    fn openai_done_frame_flushes_pending_done_with_usage() {
        let parser = OpenAiParser { auto_tool_id: false };
        let mut state = parser.new_state();

        let finish_frame = Frame {
            event: None,
            data: r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}]}"#.to_string(),
            kind: FrameKind::Data,
        };
        let finish_events = parser.parse_frame(&finish_frame, &mut state);
        assert_eq!(finish_events.len(), 1);
        assert!(matches!(&finish_events[0], LlmEvent::TextDelta(text) if text == "hi"));

        let usage_frame = Frame {
            event: None,
            data: r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#.to_string(),
            kind: FrameKind::Data,
        };
        assert!(parser.parse_frame(&usage_frame, &mut state).is_empty());

        let done_frame = Frame {
            event: None,
            data: "[DONE]".to_string(),
            kind: FrameKind::Done,
        };
        let done_events = parser.parse_frame(&done_frame, &mut state);
        assert_eq!(done_events.len(), 1);
        match &done_events[0] {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 10);
                assert_eq!(usage.output_tokens, 5);
            }
            event => panic!("expected Done, got {event:?}"),
        }
    }

    #[test]
    fn anthropic_data_frame_routes_event_type_to_text_delta() {
        let parser = AnthropicParser;
        let mut state = parser.new_state();
        let frame = Frame {
            event: Some("content_block_delta".to_string()),
            data: r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#
                .to_string(),
            kind: FrameKind::Data,
        };

        let events = parser.parse_frame(&frame, &mut state);

        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], LlmEvent::TextDelta(text) if text == "Hello"));
    }

    #[test]
    fn anthropic_invalid_json_frame_returns_no_events() {
        let parser = AnthropicParser;
        let mut state = parser.new_state();
        let frame = Frame {
            event: Some("content_block_delta".to_string()),
            data: "not json".to_string(),
            kind: FrameKind::Data,
        };

        let events = parser.parse_frame(&frame, &mut state);

        assert!(events.is_empty());
    }

    fn data(json: &str) -> Frame {
        Frame {
            event: None,
            data: json.to_string(),
            kind: FrameKind::Data,
        }
    }

    fn done_sentinel() -> Frame {
        Frame {
            event: None,
            data: "[DONE]".to_string(),
            kind: FrameKind::Done,
        }
    }

    const TEAM_TOOL_DELTA: &str = r#"{"choices":[{"delta":{"content":"dispatching now","tool_calls":[{"index":0,"id":"call_1","function":{"name":"team_send_message","arguments":"{\"to\":\"backend\"}"}}]}}]}"#;

    fn tool_uses(events: &[LlmEvent]) -> Vec<(String, serde_json::Value)> {
        events
            .iter()
            .filter_map(|event| match event {
                LlmEvent::ToolUse { name, input, .. } => Some((name.clone(), input.clone())),
                _ => None,
            })
            .collect()
    }

    fn done_reason(events: &[LlmEvent]) -> Option<StopReason> {
        events.iter().find_map(|event| match event {
            LlmEvent::Done { stop_reason, .. } => Some(*stop_reason),
            _ => None,
        })
    }

    #[test]
    fn openai_tool_calls_survive_non_standard_finish_reason() {
        // Vendor gateways send legacy or custom finish reasons. The tool call
        // must still reach the agent, otherwise the turn reads as text-only:
        // the model says it is dispatching work and nothing happens.
        for finish_reason in ["function_call", "tool_use", "end_turn"] {
            let parser = OpenAiParser { auto_tool_id: false };
            let mut state = parser.new_state();
            let mut events = parser.parse_frame(&data(TEAM_TOOL_DELTA), &mut state);
            let finish = format!(r#"{{"choices":[{{"delta":{{}},"finish_reason":"{finish_reason}"}}]}}"#);
            events.extend(parser.parse_frame(&data(&finish), &mut state));
            events.extend(parser.parse_frame(&done_sentinel(), &mut state));

            let calls = tool_uses(&events);
            assert_eq!(calls.len(), 1, "finish_reason={finish_reason} dropped the tool call");
            assert_eq!(calls[0].0, "team_send_message");
            assert_eq!(calls[0].1["to"], "backend");
            assert_eq!(
                done_reason(&events),
                Some(StopReason::ToolUse),
                "finish_reason={finish_reason}"
            );
        }
    }

    #[test]
    fn openai_tool_calls_flush_on_done_sentinel_without_finish_reason() {
        let parser = OpenAiParser { auto_tool_id: true };
        let mut state = parser.new_state();
        let mut events = parser.parse_frame(&data(TEAM_TOOL_DELTA), &mut state);
        assert!(tool_uses(&events).is_empty(), "calls wait for a terminal signal");
        events.extend(parser.parse_frame(&done_sentinel(), &mut state));

        let calls = tool_uses(&events);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "team_send_message");
        assert_eq!(done_reason(&events), Some(StopReason::ToolUse));
    }

    #[test]
    fn openai_bare_eof_does_not_execute_accumulated_tool_calls() {
        // Without [DONE] or a finish_reason the stream may have been cut
        // mid-call; executing a possibly partial call is worse than failing
        // the stream so the runner retries.
        let parser = OpenAiParser { auto_tool_id: false };
        let mut state = parser.new_state();
        let _ = parser.parse_frame(&data(TEAM_TOOL_DELTA), &mut state);
        let events = parser.finish(&mut state);
        assert!(events.is_empty(), "bare EOF must stay a failed stream, got {events:?}");
    }

    #[test]
    fn openai_unknown_finish_reason_without_tool_calls_is_unchanged() {
        let parser = OpenAiParser { auto_tool_id: false };
        let mut state = parser.new_state();
        let mut events = parser.parse_frame(
            &data(r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":"content_filter"}]}"#),
            &mut state,
        );
        events.extend(parser.parse_frame(&done_sentinel(), &mut state));
        assert!(tool_uses(&events).is_empty());
        assert_eq!(done_reason(&events), None);
    }
}

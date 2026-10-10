use super::PendingInput;
use dream_engine_types::message::ContentBlock;

fn text(s: &str) -> Vec<ContentBlock> {
    vec![ContentBlock::Text { text: s.to_string() }]
}

/// `ContentBlock` has no `PartialEq`; compare the text each message carries.
fn texts(messages: &[Vec<ContentBlock>]) -> Vec<String> {
    messages
        .iter()
        .flatten()
        .map(|block| match block {
            ContentBlock::Text { text } => text.clone(),
            other => panic!("unexpected block {other:?}"),
        })
        .collect()
}

#[test]
fn a_closed_inbox_refuses_input() {
    let inbox = PendingInput::default();
    assert!(!inbox.push(text("too early")));
    assert!(inbox.is_empty());
}

#[test]
fn an_open_inbox_keeps_input_in_order() {
    let inbox = PendingInput::default();
    inbox.open();
    assert!(inbox.push(text("first")));
    assert!(inbox.push(text("second")));
    let drained = inbox.drain();
    assert_eq!(texts(&drained), ["first", "second"]);
    assert!(inbox.is_empty());
}

#[test]
fn close_if_empty_refuses_to_close_over_waiting_input() {
    let inbox = PendingInput::default();
    inbox.open();
    inbox.push(text("late"));
    assert!(!inbox.close_if_empty(), "waiting input must keep the inbox open");
    // Still open: a further push is accepted, not lost.
    assert!(inbox.push(text("later")));
    assert_eq!(inbox.drain().len(), 2);
    assert!(inbox.close_if_empty());
    assert!(!inbox.push(text("after close")));
}

#[test]
fn close_and_drain_returns_leftovers_and_closes() {
    let inbox = PendingInput::default();
    inbox.open();
    inbox.push(text("left over"));
    assert_eq!(texts(&inbox.close_and_drain()), ["left over"]);
    assert!(!inbox.push(text("after")));
}

#[test]
fn clones_share_one_inbox() {
    let host_side = PendingInput::default();
    let engine_side = host_side.clone();
    host_side.open();
    assert!(host_side.push(text("hi")));
    assert_eq!(texts(&engine_side.drain()), ["hi"]);
}

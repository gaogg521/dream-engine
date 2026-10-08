use super::*;

fn answer(question: &str, label: &str) -> AskAnswer {
    AskAnswer {
        question: question.to_string(),
        labels: vec![label.to_string()],
    }
}

#[tokio::test]
async fn resolve_delivers_answers_to_the_waiting_request() {
    let mgr = AskUserManager::new();
    let rx = mgr.request("q1");
    assert!(mgr.is_pending("q1"));

    assert!(mgr.resolve("q1", AskUserOutcome::Answered(vec![answer("Who?", "A")])));

    assert_eq!(rx.await.unwrap(), AskUserOutcome::Answered(vec![answer("Who?", "A")]));
    assert!(!mgr.is_pending("q1"));
}

#[test]
fn resolve_unknown_or_settled_request_reports_false() {
    let mgr = AskUserManager::new();
    assert!(!mgr.resolve("missing", AskUserOutcome::Declined));

    let _rx = mgr.request("q1");
    assert!(mgr.resolve("q1", AskUserOutcome::Declined));
    // A second answer (double click, second client) must not be reported as delivered.
    assert!(!mgr.resolve("q1", AskUserOutcome::Declined));
}

#[test]
fn abandoned_request_is_neither_pending_nor_resolvable() {
    let mgr = AskUserManager::new();
    let rx = mgr.request("q1");
    // The asking turn was stopped: its receiver is gone.
    drop(rx);

    assert!(!mgr.is_pending("q1"));
    assert!(!mgr.resolve("q1", AskUserOutcome::Declined));
}

#[test]
fn question_accepts_both_wire_spellings_of_multi_select() {
    let camel: AskQuestion =
        serde_json::from_str(r#"{"question":"Q","options":[{"label":"A"}],"multiSelect":true}"#).unwrap();
    let snake: AskQuestion =
        serde_json::from_str(r#"{"question":"Q","options":[{"label":"A"}],"multi_select":true}"#).unwrap();
    assert!(camel.multi_select);
    assert!(snake.multi_select);

    let json = serde_json::to_value(&camel).unwrap();
    assert_eq!(json["multiSelect"], true);
    assert!(
        json.get("header").is_none(),
        "absent header must be omitted on the wire"
    );
}

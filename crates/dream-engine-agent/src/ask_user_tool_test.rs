use std::io;
use std::sync::Mutex;

use super::*;

/// Records every emitted event as JSON (ProtocolEvent is not Clone).
#[derive(Default)]
struct RecordingEmitter {
    events: Mutex<Vec<Value>>,
    fail: bool,
}

impl ProtocolEmitter for RecordingEmitter {
    fn emit(&self, event: &ProtocolEvent) -> io::Result<()> {
        if self.fail {
            return Err(io::Error::other("host gone"));
        }
        self.events.lock().unwrap().push(serde_json::to_value(event).unwrap());
        Ok(())
    }
}

fn two_questions() -> Value {
    json!({
        "questions": [
            {
                "question": "Who is the audience?",
                "header": "Audience",
                "options": [{"label": "Singles (Recommended)"}, {"label": "Couples"}]
            },
            {
                "question": "Which platforms?",
                "options": [{"label": "Web"}, {"label": "Mini program"}, {"label": "App"}],
                "multiSelect": true
            }
        ]
    })
}

fn setup(fail: bool) -> (Arc<AskUserManager>, Arc<RecordingEmitter>, AskUserTool) {
    let manager = Arc::new(AskUserManager::new());
    let emitter = Arc::new(RecordingEmitter {
        fail,
        ..Default::default()
    });
    let tool = AskUserTool::new(manager.clone(), emitter.clone());
    (manager, emitter, tool)
}

/// Wait until the tool has emitted its question, then return the request id.
async fn raised_request_id(emitter: &RecordingEmitter) -> String {
    for _ in 0..200 {
        if let Some(event) = emitter.events.lock().unwrap().first() {
            return event["request_id"].as_str().unwrap().to_string();
        }
        tokio::task::yield_now().await;
    }
    panic!("ask_user event was never emitted");
}

#[tokio::test]
async fn answered_questions_are_returned_to_the_model_per_question() {
    let (manager, emitter, tool) = setup(false);
    let task = tokio::spawn(async move { tool.execute(two_questions()).await });

    let request_id = raised_request_id(&emitter).await;
    let event = emitter.events.lock().unwrap()[0].clone();
    assert_eq!(event["type"], "ask_user");
    assert_eq!(event["questions"][1]["multiSelect"], true);
    assert!(manager.is_pending(&request_id));

    assert!(manager.resolve(
        &request_id,
        AskUserOutcome::Answered(vec![
            AskAnswer {
                question: "Who is the audience?".into(),
                labels: vec!["Singles (Recommended)".into()],
            },
            AskAnswer {
                question: "Which platforms?".into(),
                labels: vec!["Web".into(), "a desktop client".into()],
            },
        ]),
    ));

    let result = task.await.unwrap();
    assert!(!result.is_error);
    assert!(
        result
            .content
            .contains("Who is the audience?\n  Answer: Singles (Recommended)")
    );
    // Multi-select and free-text ("Other") answers both survive.
    assert!(result.content.contains("Answer: Web, a desktop client"));
}

#[tokio::test]
async fn unanswered_question_in_an_answer_set_is_flagged_not_dropped() {
    let (manager, emitter, tool) = setup(false);
    let task = tokio::spawn(async move { tool.execute(two_questions()).await });
    let request_id = raised_request_id(&emitter).await;

    manager.resolve(
        &request_id,
        AskUserOutcome::Answered(vec![AskAnswer {
            question: "Who is the audience?".into(),
            labels: vec!["Couples".into()],
        }]),
    );

    let result = task.await.unwrap();
    assert!(
        result
            .content
            .contains("Which platforms?\n  Answer: (no answer; use your best judgment)")
    );
}

#[tokio::test]
async fn declined_question_tells_the_model_to_continue_without_asking_again() {
    let (manager, emitter, tool) = setup(false);
    let task = tokio::spawn(async move { tool.execute(two_questions()).await });
    let request_id = raised_request_id(&emitter).await;

    manager.resolve(&request_id, AskUserOutcome::Declined);

    let result = task.await.unwrap();
    assert!(!result.is_error);
    assert!(result.content.contains("Do not ask them again"));
}

#[tokio::test]
async fn stopped_turn_withdraws_the_pending_question() {
    let (manager, emitter, tool) = setup(false);
    let task = tokio::spawn(async move { tool.execute(two_questions()).await });
    let request_id = raised_request_id(&emitter).await;

    task.abort();
    let _ = task.await;

    assert!(!manager.is_pending(&request_id));
    assert!(!manager.resolve(&request_id, AskUserOutcome::Declined));
}

#[tokio::test]
async fn host_that_cannot_show_the_question_gets_an_error_instead_of_hanging() {
    let (_manager, _emitter, tool) = setup(true);

    // Must return promptly: nobody can ever answer a question that was never shown.
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), tool.execute(two_questions()))
        .await
        .expect("execute must not wait for an answer after a failed emit");

    assert!(result.is_error);
    assert!(result.content.contains("could not be shown"));
}

#[tokio::test]
async fn invalid_inputs_are_rejected_before_anything_is_shown() {
    let (_manager, emitter, tool) = setup(false);
    let cases = [
        (json!({}), "Missing required field"),
        (json!({"questions": []}), "between 1 and 4"),
        (
            json!({"questions": [{"question": "Q?", "options": [{"label": "only one"}]}]}),
            "between 2 and 6 options",
        ),
        (
            json!({"questions": [{"question": " ", "options": [{"label": "A"}, {"label": "B"}]}]}),
            "empty `question`",
        ),
        (
            json!({"questions": [
                {"question": "Same?", "options": [{"label": "A"}, {"label": "B"}]},
                {"question": "Same?", "options": [{"label": "A"}, {"label": "B"}]}
            ]}),
            "duplicates",
        ),
        (
            json!({"questions": [{"question": "Q?", "options": [{"label": "A"}, {"label": ""}]}]}),
            "empty `label`",
        ),
    ];

    for (input, expected) in cases {
        let result = tool.execute(input.clone()).await;
        assert!(result.is_error, "{input} should be rejected");
        assert!(result.content.contains(expected), "{input}: {}", result.content);
    }
    assert!(emitter.events.lock().unwrap().is_empty());
}

#[test]
fn asking_skips_the_approval_prompt() {
    let (_manager, _emitter, tool) = setup(false);
    assert!(!tool.requires_approval());
    assert_eq!(tool.name(), ASK_USER_TOOL_NAME);
}

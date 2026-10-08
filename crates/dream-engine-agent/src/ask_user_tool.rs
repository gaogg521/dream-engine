use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use uuid::Uuid;

use dream_engine_protocol::events::{ProtocolEvent, ToolCategory};
use dream_engine_protocol::writer::ProtocolEmitter;
use dream_engine_protocol::{AskAnswer, AskQuestion, AskUserManager, AskUserOutcome};
use dream_engine_tools::Tool;
use dream_engine_types::tool::{JsonSchema, ToolResult};

/// Name the model calls. Matches the AskUserQuestion convention models are
/// already trained on, so they reach for it without extra prompting.
pub const ASK_USER_TOOL_NAME: &str = "AskUserQuestion";

const MAX_QUESTIONS: usize = 4;
const MIN_OPTIONS: usize = 2;
const MAX_OPTIONS: usize = 6;

const DESCRIPTION: &str = "\
Ask the user one to four structured multiple-choice questions and wait for the answers. \
The host shows them as a selection dialog; the user can always type a custom answer instead of an option.\n\
\n\
Use this ONLY when you are genuinely blocked: the answer materially changes the result, it cannot be \
inferred from the request, the conversation or the workspace, and a wrong guess would be costly to undo. \
For everything else pick a sensible default, state the assumption, and keep working.\n\
\n\
Rules:\n\
- Ask everything you need in ONE call (up to 4 questions) instead of asking one question per turn.\n\
- Never write numbered or lettered option lists in your reply and wait for the user to type a letter; call this tool instead.\n\
- Each question offers 2-4 concrete, mutually exclusive options (set multiSelect when several can apply). \
Put the option you recommend first and append \"(Recommended)\" to its label.\n\
- Do not add an \"Other\" option; the dialog provides free-text input automatically.";

/// Agent-level tool that raises a structured question to the host and blocks
/// until the user answers.
///
/// Only registered by hosts that can render the question (JSON stream /
/// embedded backends), so a terminal session never advertises it.
pub struct AskUserTool {
    manager: Arc<AskUserManager>,
    emitter: Arc<dyn ProtocolEmitter>,
}

impl AskUserTool {
    pub fn new(manager: Arc<AskUserManager>, emitter: Arc<dyn ProtocolEmitter>) -> Self {
        Self { manager, emitter }
    }
}

/// Removes the pending entry when the asking turn is dropped (user pressed
/// Stop), so a late answer is reported as stale instead of silently vanishing.
struct PendingGuard<'a> {
    manager: &'a AskUserManager,
    request_id: &'a str,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.manager.drop_pending(self.request_id);
    }
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        ASK_USER_TOOL_NAME
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_QUESTIONS,
                    "description": "Questions to ask the user (1-4), all shown in one dialog",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "The complete question, ending with a question mark"
                            },
                            "header": {
                                "type": "string",
                                "description": "Very short label shown as a chip, e.g. \"Audience\" (max ~12 chars)"
                            },
                            "options": {
                                "type": "array",
                                "minItems": MIN_OPTIONS,
                                "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {
                                            "type": "string",
                                            "description": "Concise choice text (1-5 words)"
                                        },
                                        "description": {
                                            "type": "string",
                                            "description": "What this option means or implies"
                                        }
                                    },
                                    "required": ["label"]
                                }
                            },
                            "multiSelect": {
                                "type": "boolean",
                                "description": "Allow selecting several options"
                            }
                        },
                        "required": ["question", "options"]
                    }
                }
            },
            "required": ["questions"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let questions = match parse_questions(&input) {
            Ok(questions) => questions,
            Err(message) => {
                return ToolResult {
                    content: message,
                    is_error: true,
                };
            }
        };

        let request_id = Uuid::now_v7().to_string();
        let rx = self.manager.request(&request_id);
        let _guard = PendingGuard {
            manager: &self.manager,
            request_id: &request_id,
        };

        if let Err(e) = self.emitter.emit(&ProtocolEvent::AskUser {
            request_id: request_id.clone(),
            questions: questions.clone(),
        }) {
            tracing::error!(target: "dream_engine_agent", %request_id, error = %e, "failed to deliver ask_user question to host");
            return ToolResult {
                content: "The question could not be shown to the user. Continue with sensible defaults and state your assumptions.".to_string(),
                is_error: true,
            };
        }
        tracing::info!(target: "dream_engine_agent", %request_id, questions = questions.len(), "ask_user question raised");

        match rx.await {
            Ok(AskUserOutcome::Answered(answers)) => {
                tracing::info!(target: "dream_engine_agent", %request_id, "ask_user answered");
                ToolResult {
                    content: format_answers(&questions, &answers),
                    is_error: false,
                }
            }
            Ok(AskUserOutcome::Declined) => {
                tracing::info!(target: "dream_engine_agent", %request_id, "ask_user declined");
                ToolResult {
                    content: "The user dismissed the questions without answering. Do not ask them again; \
                              continue with sensible defaults and state the assumptions you made."
                        .to_string(),
                    is_error: false,
                }
            }
            Err(_) => ToolResult {
                content: "The question was withdrawn before the user answered.".to_string(),
                is_error: true,
            },
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    /// Asking is the user interaction itself; gating it behind a second
    /// approval prompt would ask the user twice.
    fn requires_approval(&self) -> bool {
        false
    }

    fn describe(&self, input: &Value) -> String {
        let count = input["questions"].as_array().map_or(0, Vec::len);
        format!("{ASK_USER_TOOL_NAME}: {count} question(s)")
    }
}

fn parse_questions(input: &Value) -> Result<Vec<AskQuestion>, String> {
    let raw = input
        .get("questions")
        .cloned()
        .ok_or_else(|| "Missing required field `questions`.".to_string())?;
    let questions: Vec<AskQuestion> = serde_json::from_value(raw).map_err(|e| format!("Invalid `questions`: {e}"))?;

    if questions.is_empty() || questions.len() > MAX_QUESTIONS {
        return Err(format!(
            "Provide between 1 and {MAX_QUESTIONS} questions (got {}). Ask the most important ones only.",
            questions.len()
        ));
    }

    let mut seen = HashSet::new();
    for (index, question) in questions.iter().enumerate() {
        let number = index + 1;
        let text = question.question.trim();
        if text.is_empty() {
            return Err(format!("Question {number} has an empty `question` text."));
        }
        // Answers are keyed by the question text, so duplicates would collide.
        if !seen.insert(text.to_string()) {
            return Err(format!(
                "Question {number} duplicates an earlier question; each must be unique."
            ));
        }
        let options = question.options.len();
        if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&options) {
            return Err(format!(
                "Question {number} must offer between {MIN_OPTIONS} and {MAX_OPTIONS} options (got {options})."
            ));
        }
        if question.options.iter().any(|option| option.label.trim().is_empty()) {
            return Err(format!("Question {number} has an option with an empty `label`."));
        }
    }

    Ok(questions)
}

fn format_answers(questions: &[AskQuestion], answers: &[AskAnswer]) -> String {
    let mut lines = vec!["The user answered your questions:".to_string()];
    for question in questions {
        let labels = answers
            .iter()
            .find(|answer| answer.question.trim() == question.question.trim())
            .map(|answer| {
                answer
                    .labels
                    .iter()
                    .map(|label| label.trim())
                    .filter(|label| !label.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let answer = if labels.is_empty() {
            "(no answer; use your best judgment)".to_string()
        } else {
            labels.join(", ")
        };
        lines.push(format!("- {}\n  Answer: {answer}", question.question.trim()));
    }
    lines.push("Proceed with the task using these answers.".to_string());
    lines.join("\n")
}

#[cfg(test)]
#[path = "ask_user_tool_test.rs"]
mod ask_user_tool_test;

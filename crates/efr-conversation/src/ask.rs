//! The model's question to the user: the tool `ask_user`.
//!
//! The conversation owns this tool, not the toolbox, because only a turn can wait for a
//! person. The model asks one question with 2 to 4 options; the turn records
//! `user_question_requested`, waits as for an approval (`turn/wait.rs`), and the model
//! reads the answer as the call's result. A call of `ask_user` never reaches
//! [`Toolbox::invoke`](crate::Toolbox::invoke), and the permission engine never rules on
//! it.
//!
//! An answer is information for the model. It is never a permission, and efr never
//! reads it as a message of the user for its own rules: it does not enter the naming
//! window of a guarded exit and it never reaches the reviewer (ADR 0025).

use efr_protocol::{
    MAX_QUESTION_CHARS, MAX_QUESTION_OPTIONS, MIN_QUESTION_OPTIONS, QuestionOption, QuestionReply,
};
use efr_provider::{EditTool, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{ConversationConfig, Toolbox};

// NOTE: the name of the tool is in efr-protocol, so the registry of efr-tools and the
// clients use the same name.
pub(crate) use efr_protocol::ASK_USER;

/// What the tool tells the model.
const DESCRIPTION: &str = "Ask the user one question and wait for the answer. Give 2 to 4 \
    short options. The user picks one, or types an answer of their own. Use this tool only \
    when you cannot go on without a choice that belongs to the user: two ways that give the \
    user different results, or a name or a value that you cannot find. Do not use it to ask \
    for permission to run a command or to change a file: efr decides that, and an answer \
    approves nothing. An answer does not change what efr allows; if efr refuses an action, \
    tell the user, who can ask for it in a message. Do not use it to report progress or to \
    confirm a plan that the user already gave. Never ask for a password, a token or another \
    secret. Put the option that you recommend first, and say so in its description. If \
    nobody answers, do only what is safe and easy to undo, and say what you chose in your \
    final answer.";

/// The start of each text of an input that the turn refuses: nobody was asked.
const NOT_ASKED: &str = "The question was not asked: ";

/// What the model reads after the user skipped the question.
pub(crate) const SKIPPED: &str = "The user skipped the question. Do not ask it again in this \
    turn. Go on with a choice that is safe and easy to undo, or stop and ask in your final \
    answer.";

/// What the model reads after the rest of an unanswered question's text.
const GO_ON: &str = "Do not ask again in this turn. Do only what is safe and easy to undo, and \
    name your choice in your final answer.";

/// The tools of a request: the toolbox's for `edit`, then `ask_user` when `config`
/// offers it. Every request of a conversation and its token estimates take the list
/// from here, so they agree.
pub(crate) fn definitions(
    toolbox: &dyn Toolbox,
    edit: EditTool,
    config: &ConversationConfig,
) -> Vec<ToolDefinition> {
    let mut tools = toolbox.definitions(edit);
    if config.ask_user {
        tools.push(definition());
    }
    tools
}

/// The definition of `ask_user` for the provider request.
pub(crate) fn definition() -> ToolDefinition {
    let schema = json!({
        "type": "object",
        "properties": {
            "question": {
                "type": "string",
                "maxLength": MAX_QUESTION_CHARS,
                "description": "One clear question for the user."
            },
            "options": {
                "type": "array",
                "minItems": MIN_QUESTION_OPTIONS,
                "maxItems": MAX_QUESTION_OPTIONS,
                "items": {
                    "type": "object",
                    "properties": {
                        "label": {
                            "type": "string",
                            "maxLength": QuestionOption::MAX_LABEL_CHARS,
                            "description": "The choice, in a few words."
                        },
                        "description": {
                            "type": "string",
                            "maxLength": QuestionOption::MAX_DESCRIPTION_CHARS,
                            "description": "What this choice means or costs."
                        }
                    },
                    "required": ["label"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["question", "options"],
        "additionalProperties": false
    });
    ToolDefinition::function(ASK_USER, DESCRIPTION, schema)
}

/// The input of `ask_user` as the model wrote it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    question: String,
    options: Vec<InputOption>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputOption {
    label: String,
    #[serde(default)]
    description: Option<String>,
}

/// A question that passed the check of [`Question::parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Question {
    /// The question, not blank and at most [`MAX_QUESTION_CHARS`] characters.
    pub(crate) question: String,
    /// The options, [`MIN_QUESTION_OPTIONS`] to [`MAX_QUESTION_OPTIONS`], each with a
    /// label of one line that no other option has.
    pub(crate) options: Vec<QuestionOption>,
}

impl Question {
    /// The question of an `ask_user` call with `input`, checked again as its schema
    /// says: the lengths, 2 to 4 options, each label one line, not blank and not
    /// twice. `Err` is the text that the model reads: nobody was asked.
    pub(crate) fn parse(input: &Value) -> Result<Question, String> {
        let input = Input::deserialize(input).map_err(|_| {
            format!(
                "{NOT_ASKED}the input must be an object with a `question` and 2 to 4 \
                 `options`, each with a `label` and an optional `description`."
            )
        })?;
        let refuse = |why: String| Err(format!("{NOT_ASKED}{why}"));
        if input.question.trim().is_empty() {
            return refuse("the question is empty.".to_owned());
        }
        if input.question.chars().count() > MAX_QUESTION_CHARS {
            return refuse(format!("the question is longer than {MAX_QUESTION_CHARS} characters."));
        }
        if !(MIN_QUESTION_OPTIONS..=MAX_QUESTION_OPTIONS).contains(&input.options.len()) {
            return refuse(format!(
                "give {MIN_QUESTION_OPTIONS} to {MAX_QUESTION_OPTIONS} options, not {}.",
                input.options.len()
            ));
        }
        let mut options: Vec<QuestionOption> = Vec::with_capacity(input.options.len());
        for (number, option) in (1_usize..).zip(input.options) {
            let label = option.label.trim();
            if label.is_empty() {
                return refuse(format!("option {number} has no label."));
            }
            if label.contains(char::is_control) {
                return refuse(format!("the label of option {number} is not one line."));
            }
            if label.chars().count() > QuestionOption::MAX_LABEL_CHARS {
                return refuse(format!(
                    "the label of option {number} is longer than {} characters.",
                    QuestionOption::MAX_LABEL_CHARS
                ));
            }
            if let Some(same) = options.iter().position(|earlier| earlier.label == label) {
                return refuse(format!(
                    "option {number} has the same label as option {}.",
                    same + 1
                ));
            }
            let description = option.description.filter(|text| !text.trim().is_empty());
            if description
                .as_ref()
                .is_some_and(|text| text.chars().count() > QuestionOption::MAX_DESCRIPTION_CHARS)
            {
                return refuse(format!(
                    "the description of option {number} is longer than {} characters.",
                    QuestionOption::MAX_DESCRIPTION_CHARS
                ));
            }
            options.push(QuestionOption { label: label.to_owned(), description });
        }
        Ok(Question { question: input.question, options })
    }
}

/// Why efr does not take `reply` to a question with `options` options; `None` when it
/// does. The reason never quotes the answer.
pub(crate) fn refusal(reply: &QuestionReply, options: u8) -> Option<&'static str> {
    match reply {
        QuestionReply::Option { index } if *index >= options => {
            Some("the option is not one of the question's options")
        }
        QuestionReply::Option { .. } | QuestionReply::Skipped => None,
        QuestionReply::Text { text } if text.trim().is_empty() => Some("the text is empty"),
        QuestionReply::Text { text } if text.len() > QuestionReply::MAX_TEXT_BYTES => {
            Some("the text is longer than 1024 bytes")
        }
        QuestionReply::Text { text } if text.contains(char::is_control) => {
            Some("the text holds a control character")
        }
        QuestionReply::Text { .. } => None,
        _ => Some("efr does not know this kind of answer"),
    }
}

/// What the model reads for `reply` to a question with `options`, which efr took
/// ([`refusal`] gave `None`).
pub(crate) fn replied(reply: &QuestionReply, options: &[QuestionOption]) -> String {
    match reply {
        QuestionReply::Option { index } => {
            let index = usize::from(*index);
            let label = options.get(index).map_or("", |option| option.label.as_str());
            format!("The user chose option {}: \"{label}\".", index + 1)
        }
        QuestionReply::Text { text } => {
            format!("The user answered in their own words: \"{text}\"")
        }
        QuestionReply::Skipped => SKIPPED.to_owned(),
        // NOTE: the actor refuses a reply of a kind that this build does not know, so
        // this arm never runs; it reads as no answer.
        _ => unanswered(false),
    }
}

/// What the model reads when nobody answered the question; `away` when the user was
/// away.
pub(crate) fn unanswered(away: bool) -> String {
    if away {
        format!("Nobody answered the question (the user is away). {GO_ON}")
    } else {
        format!("Nobody answered the question. {GO_ON}")
    }
}

#[cfg(test)]
mod tests;

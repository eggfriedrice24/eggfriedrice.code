//! The model's question to the user (the tool `ask_user`): its options, the user's
//! reply and how the question ended.
//!
//! An answer is information for the model. It is never a permission, and efr never
//! reads it as a message of the user for its own rules.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::InputRespond;

/// The longest question, in characters.
pub const MAX_QUESTION_CHARS: usize = 300;

/// The fewest options of a question.
pub const MIN_QUESTION_OPTIONS: usize = 2;

/// The most options of a question.
pub const MAX_QUESTION_OPTIONS: usize = 4;

/// One choice of a question. The model wrote it, so a client shows it as text that a
/// program wrote, on one line.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct QuestionOption {
    /// The choice in a few words, at most [`MAX_LABEL_CHARS`](Self::MAX_LABEL_CHARS)
    /// characters.
    pub label: String,
    /// What the choice means or costs, at most
    /// [`MAX_DESCRIPTION_CHARS`](Self::MAX_DESCRIPTION_CHARS) characters. Absent when
    /// the model gave none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl QuestionOption {
    /// The longest label, in characters.
    pub const MAX_LABEL_CHARS: usize = 60;

    /// The longest description, in characters.
    pub const MAX_DESCRIPTION_CHARS: usize = 200;
}

/// What a person answered to the model's question.
///
/// On the wire an object with a `kind` member: `{"kind": "option", "index": 1}`,
/// `{"kind": "text", "text": "..."}` or `{"kind": "skipped"}`. A kind that this build
/// does not know reads as `unknown`; efrd refuses it as `invalid`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum QuestionReply {
    /// The person picked an option.
    Option {
        /// The place of the option in the question's `options`, from 0.
        index: u8,
    },
    /// The person typed an answer of their own.
    Text {
        /// The answer: not empty, at most [`MAX_TEXT_BYTES`](Self::MAX_TEXT_BYTES)
        /// bytes, with no control character. It goes to the event log and to the
        /// model, as a prompt does.
        text: String,
    },
    /// The person skipped the question.
    Skipped,
    /// A reply that this build does not know.
    #[serde(other)]
    Unknown,
}

impl QuestionReply {
    /// The longest typed answer, in bytes: the length of a line of `input.respond`.
    pub const MAX_TEXT_BYTES: usize = InputRespond::MAX_TEXT_BYTES;
}

/// How the model's question ended, as `user_question_answered` records it.
///
/// On the wire an object with a `kind` member: `{"kind": "replied", "reply": {...}}`,
/// `{"kind": "unanswered"}` (with `"away": true` when the user was away) or
/// `{"kind": "interrupted"}`. A kind that this build does not know reads as `unknown`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum UserAnswer {
    /// A person answered.
    Replied {
        /// The answer.
        reply: QuestionReply,
    },
    /// Nobody answered in time, and the model goes on without an answer.
    Unanswered {
        /// True when the user was away. False when absent.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        away: bool,
    },
    /// The turn was interrupted while the question waited.
    Interrupted,
    /// An end that this build does not know.
    #[serde(other)]
    Unknown,
}

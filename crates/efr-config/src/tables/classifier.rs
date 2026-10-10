//! `[classifier]`: the reviewer model that decides most exits of the `auto` sandbox
//! before the user does (efr's auto spec, section 9.5, phase A).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The default of `classifier.effort`.
pub const DEFAULT_CLASSIFIER_EFFORT: &str = "low";

/// `[classifier]`: who reviews an action that leaves the `auto` sandbox. Every key
/// applies from the next exit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ClassifierSettings {
    /// `true` lets a reviewer model decide most actions that leave the sandbox in
    /// `auto`, and hand the unsure ones to you. `false` asks you about every such
    /// action, as before phase A. Each review is one more request to your provider.
    pub enabled: bool,
    /// The model that reviews first. Unset: the small model of the turn's provider,
    /// such as `gpt-6-luna` for OpenAI or `claude-haiku-5-5` for Claude.
    pub model: Option<String>,
    /// `true` lets the turn's model review when the first model is missing, refuses,
    /// times out or gives an answer that does not fit. `false` then asks you.
    pub fallback_to_turn_model: bool,
    /// The reasoning effort of each review, one lowercase word such as `low`, for the
    /// models that take an effort.
    pub effort: String,
    /// How long each model of the chain may take, in seconds, from 1 to 300. A slower
    /// answer goes to the next step.
    pub timeout_secs: u32,
}

impl Default for ClassifierSettings {
    fn default() -> Self {
        ClassifierSettings {
            enabled: true,
            model: None,
            fallback_to_turn_model: true,
            effort: DEFAULT_CLASSIFIER_EFFORT.to_owned(),
            timeout_secs: 20,
        }
    }
}

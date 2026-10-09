//! The few facts about Claude models that efr knows without the API.
//!
//! There is no table of models here, unlike `efr-provider-openai`: the list, the
//! windows, the output limits and the effort levels come only from `GET /models` (see
//! `catalog`). A guessed window would be wrong for a new model, and a wrong window
//! breaks the compaction. What stays are the defaults that Claude Code uses.
//!
//! The prompt cache of the current Claude models holds no prefix shorter than 512
//! tokens. No code here needs that number: a marker on a shorter prefix costs nothing,
//! and the end-of-turn line of `efr` applies its own, higher minimum.

/// The model of a turn that names none, when the API lists it: Claude Code's default.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";

/// The effort of a request that names none, when the model lists it: Claude Code's
/// default. The catalog makes it the model's default effort, so the provider sends it,
/// because the API's own default differs from model to model. A model that does not
/// list it gets no effort from efr.
pub const DEFAULT_EFFORT: &str = "medium";

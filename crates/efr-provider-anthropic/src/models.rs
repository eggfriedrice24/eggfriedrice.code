//! The few facts about Claude models that efr knows without the API.
//!
//! There is no table of models here, unlike `efr-provider-openai`: the list, the
//! windows, the output limits and the effort levels come only from `GET /models` (see
//! `catalog`). A guessed window would be wrong for a new model, and a wrong window
//! breaks the compaction. What stays are the defaults that Claude Code uses and one
//! limit of the prompt cache.

/// The model of a turn that names none, when the API lists it: Claude Code's default.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";

/// The effort of a request that names none, when the model takes it: Claude Code's
/// default. The provider always sends an effort, because the API's own default differs
/// from model to model.
pub const DEFAULT_EFFORT: &str = "medium";

/// The fewest tokens that a prompt cache entry holds on the current Claude models. A
/// shorter prefix is never cached, whatever its markers say.
pub const CACHE_MIN_TOKENS: u64 = 512;

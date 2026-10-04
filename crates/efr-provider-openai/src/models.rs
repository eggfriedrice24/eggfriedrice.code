//! The models each backend is known to serve.
//!
//! The lists are hints, not gates: a request for a model that is not listed is still
//! sent, and the backend's answer decides (an unknown model comes back as
//! `ProviderError::UnknownModel`). Model ids change every few months and which ids the
//! subscription backend accepts for a client other than Codex is an open question, so
//! the daemon can replace the list through `OpenAiConfig::with_models`.

use efr_provider::ModelInfo;

/// The model the subscription is used with when the config names none, as in goose
/// (`crates/goose/src/providers/chatgpt_codex.rs`, `CHATGPT_CODEX_DEFAULT_MODEL`).
pub const DEFAULT_SUBSCRIPTION_MODEL: &str = "gpt-5.5";

/// The input budget Codex gives every listed model
/// (`codex-rs/models-manager/models.json`, `context_window`).
const CODEX_CONTEXT_WINDOW: u64 = 272_000;

/// The output limit opencode records for the 5.5 and 5.6 families on the subscription
/// (`packages/opencode/src/plugin/openai/codex.ts`).
const GPT_5_OUTPUT_LIMIT: u32 = 128_000;

/// The models the ChatGPT subscription backend lists for Codex, newest first, from the
/// `visibility: "list"` entries of `codex-rs/models-manager/models.json`.
const SUBSCRIPTION: &[(&str, Option<u32>)] = &[
    ("gpt-6-astra", None),
    ("gpt-6.1-sol", None),
    ("gpt-6-sol", None),
    ("gpt-6-luna", None),
    ("gpt-5.6-sol", Some(GPT_5_OUTPUT_LIMIT)),
    ("gpt-5.6-terra", Some(GPT_5_OUTPUT_LIMIT)),
    ("gpt-5.6-luna", Some(GPT_5_OUTPUT_LIMIT)),
    ("gpt-5.5", Some(GPT_5_OUTPUT_LIMIT)),
];

/// Model id prefixes of the reasoning families. Every model the subscription serves is
/// one of them; on the API, older chat models are not, and they refuse the `reasoning`
/// parameter and encrypted reasoning.
const REASONING_PREFIXES: &[&str] = &["gpt-5", "gpt-6", "o1", "o3", "o4", "codex-"];

/// The models the subscription backend is known to serve, with their limits.
pub fn subscription_models() -> Vec<ModelInfo> {
    SUBSCRIPTION
        .iter()
        .map(|&(id, max_output)| {
            let model = ModelInfo::new(id).with_context_window(CODEX_CONTEXT_WINDOW);
            match max_output {
                Some(tokens) => model.with_max_output_tokens(tokens),
                None => model,
            }
        })
        .collect()
}

/// The models the API is known to serve: none listed, because an API key reaches
/// every model of its organisation and the list changes too often to keep here. Any
/// model id is passed through.
pub fn api_models() -> Vec<ModelInfo> {
    Vec::new()
}

/// True when `model` belongs to a reasoning family, so a request for it carries the
/// `reasoning` parameter and asks for encrypted reasoning back. Chat variants of a
/// reasoning family (such as `gpt-5-chat-latest`) do not reason.
pub(crate) fn is_reasoning_model(model: &str) -> bool {
    REASONING_PREFIXES.iter().any(|prefix| model.starts_with(prefix)) && !model.contains("-chat")
}

#[cfg(test)]
mod tests;

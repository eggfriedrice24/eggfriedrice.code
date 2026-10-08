//! The models each backend is known to serve.
//!
//! The lists are hints for the request, not gates: the provider sends a request for a
//! model that is not listed, and the backend's answer decides (an unknown model comes
//! back as `ProviderError::UnknownModel`). The daemon checks a prompt's model and effort
//! against the effective list, which is the built-in one plus the ids of
//! `[openai] models`.
//!
//! NOTE: Codex does not trust a fixed list for a ChatGPT login. It fetches
//! `GET https://chatgpt.com/backend-api/codex/models?client_version=<version>` with the
//! login's token (`codex-rs/model-provider/src/models_endpoint.rs`, `MODELS_ENDPOINT`,
//! cached for 5 minutes in `models_cache.json`) and falls back to the bundled
//! `codex-rs/models-manager/models.json`. efr does not call that endpoint yet; the
//! built-in list below is the bundled catalog.

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

/// The efforts of the models that take every level up to `ultra`.
const UP_TO_ULTRA: &[&str] = &["low", "medium", "high", "xhigh", "max", "ultra"];

/// The efforts of the models that stop at `max`.
const UP_TO_MAX: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// The efforts of gpt-5.5.
const UP_TO_XHIGH: &[&str] = &["low", "medium", "high", "xhigh"];

/// One model of the built-in subscription list.
struct Builtin {
    id: &'static str,
    max_output: Option<u32>,
    efforts: &'static [&'static str],
    default_effort: &'static str,
    /// True when the model takes freeform (`custom`) tools, so a freeform tool such as
    /// `apply_patch` goes to it in its freeform form with its grammar. Codex's catalog
    /// says it per model as `apply_patch_tool_type`: `"freeform"` for this form; a model
    /// without the key gets no `apply_patch` tool in Codex. efr sends such a model the
    /// function form instead.
    freeform_tools: bool,
}

/// The models that the ChatGPT subscription backend lists for Codex, in the order of
/// Codex's model picker (its `priority`).
///
/// Verified on 2026-10-05 against two sources:
///
/// - openai/codex at 823ea830c0fd (2026-10-05), `codex-rs/models-manager/models.json`:
///   every entry with `visibility: "list"`, each with its `supported_reasoning_levels`
///   and `default_reasoning_level`. Every one is offered on the Plus and Pro plans
///   (`available_in_plans`). The hidden entries (`gpt-daybreak-*`,
///   `codex-auto-review`) are left out.
/// - sst/opencode at 907b3bc518fa (2026-10-02),
///   `packages/opencode/src/plugin/openai/codex.ts`: with a ChatGPT login it keeps
///   `gpt-5.5`, `gpt-5.3-codex-spark`, `gpt-5.4`, `gpt-5.4-mini`, `gpt-6-sol`,
///   `gpt-6-luna` and every newer `gpt-N.M` except the bare `gpt-5.6` and the pro
///   models. Every id below passes that filter; the older ids that opencode keeps are
///   not in Codex's list, so they are left out.
///
/// Read again at c0c230e (2026-10-08) for `apply_patch_tool_type`: every listed entry
/// has `"freeform"`, and `ApplyPatchToolType` (`codex-rs/protocol/src/openai_models.rs`)
/// has no other value left.
const SUBSCRIPTION: &[Builtin] = &[
    Builtin {
        id: "gpt-6.1-sol",
        max_output: None,
        efforts: UP_TO_ULTRA,
        default_effort: "low",
        freeform_tools: true,
    },
    Builtin {
        id: "gpt-6-astra",
        max_output: None,
        efforts: UP_TO_ULTRA,
        default_effort: "low",
        freeform_tools: true,
    },
    Builtin {
        id: "gpt-6-sol",
        max_output: None,
        efforts: UP_TO_ULTRA,
        default_effort: "medium",
        freeform_tools: true,
    },
    Builtin {
        id: "gpt-6-luna",
        max_output: None,
        efforts: UP_TO_MAX,
        default_effort: "medium",
        freeform_tools: true,
    },
    Builtin {
        id: "gpt-5.6-sol",
        max_output: Some(GPT_5_OUTPUT_LIMIT),
        efforts: UP_TO_ULTRA,
        default_effort: "low",
        freeform_tools: true,
    },
    Builtin {
        id: "gpt-5.6-terra",
        max_output: Some(GPT_5_OUTPUT_LIMIT),
        efforts: UP_TO_ULTRA,
        default_effort: "medium",
        freeform_tools: true,
    },
    Builtin {
        id: "gpt-5.6-luna",
        max_output: Some(GPT_5_OUTPUT_LIMIT),
        efforts: UP_TO_MAX,
        default_effort: "medium",
        freeform_tools: true,
    },
    Builtin {
        id: "gpt-5.5",
        max_output: Some(GPT_5_OUTPUT_LIMIT),
        efforts: UP_TO_XHIGH,
        default_effort: "medium",
        freeform_tools: true,
    },
];

/// Model id prefixes of the reasoning families. Every model the subscription serves is
/// one of them; on the API, older chat models are not, and they refuse the `reasoning`
/// parameter and encrypted reasoning.
const REASONING_PREFIXES: &[&str] = &["gpt-5", "gpt-6", "o1", "o3", "o4", "codex-"];

/// The models the subscription backend is known to serve, with their limits, their
/// reasoning efforts and the default effort of each.
pub fn subscription_models() -> Vec<ModelInfo> {
    SUBSCRIPTION
        .iter()
        .map(|builtin| {
            let model = ModelInfo::new(builtin.id)
                .with_context_window(CODEX_CONTEXT_WINDOW)
                .with_efforts(builtin.efforts.iter().copied(), Some(builtin.default_effort));
            match builtin.max_output {
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

/// True when `model` takes freeform (`custom`) tools, so a request sends a tool with a
/// grammar in its freeform form. Only the models of the built-in catalog do, on either
/// backend, because the catalog is the one source that says so per model. Every other
/// model gets each freeform tool in its function form, one string member that holds
/// the text, which every model with function calls takes.
pub(crate) fn takes_freeform_tools(model: &str) -> bool {
    SUBSCRIPTION.iter().any(|builtin| builtin.id == model && builtin.freeform_tools)
}

/// True when `model` belongs to a reasoning family, so a request for it carries the
/// `reasoning` parameter and asks for encrypted reasoning back. Chat variants of a
/// reasoning family (such as `gpt-5-chat-latest`) do not reason.
pub(crate) fn is_reasoning_model(model: &str) -> bool {
    REASONING_PREFIXES.iter().any(|prefix| model.starts_with(prefix)) && !model.contains("-chat")
}

#[cfg(test)]
mod tests;

//! The model table built into efr: the last fallback of the model catalog.
//!
//! efr reads the subscription's models from the backend (`catalog`): the list, each
//! model's window, its efforts and its tool forms. This table stands in only when no
//! catalog came from the backend and no cache of one is on disk, such as at the first
//! start while offline, and for the API key backend, whose `/v1/models` says nothing
//! about windows. It is a copy of Codex's bundled catalog
//! (`codex-rs/models-manager/models.json`), which Codex uses for the same purpose.

use crate::catalog::{CatalogEntry, Visibility};

/// The input budget Codex gives every listed model
/// (`codex-rs/models-manager/models.json`, `context_window`).
const CONTEXT_WINDOW: u64 = 272_000;

/// The largest window that a setting can raise the window of the newer models to
/// (`max_context_window`).
const MAX_CONTEXT_WINDOW: u64 = 872_000;

/// The efforts of the models that take every level up to `ultra`.
const UP_TO_ULTRA: &[&str] = &["low", "medium", "high", "xhigh", "max", "ultra"];

/// The efforts of the models that stop at `max`.
const UP_TO_MAX: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// The efforts of gpt-5.5.
const UP_TO_XHIGH: &[&str] = &["low", "medium", "high", "xhigh"];

/// One model of the built-in table.
struct Builtin {
    slug: &'static str,
    display_name: &'static str,
    description: &'static str,
    priority: i64,
    max_context_window: u64,
    efforts: &'static [&'static str],
    default_effort: &'static str,
}

/// The models of Codex's bundled catalog with `visibility: "list"`, in the order of
/// their `priority`.
///
/// Read at c0c230e of openai/codex (2026-10-08), `codex-rs/models-manager/models.json`.
/// Every listed entry has `apply_patch_tool_type: "freeform"`, `prefer_websockets:
/// true`, `supported_in_api: true` and a `context_window` of 272000. The hidden
/// entries (`gpt-daybreak-*`, `codex-auto-review`) are left out. The descriptions are
/// the catalog's own; gpt-5.5 is the legacy model now.
const TABLE: &[Builtin] = &[
    Builtin {
        slug: "gpt-6.1-sol",
        display_name: "GPT-6.1-Sol",
        description: "Latest workhorse model for coding and everyday work.",
        priority: 1,
        max_context_window: MAX_CONTEXT_WINDOW,
        efforts: UP_TO_ULTRA,
        default_effort: "low",
    },
    Builtin {
        slug: "gpt-6-astra",
        display_name: "GPT-6-Astra",
        description: "Frontier intelligence for the most demanding work.",
        priority: 2,
        max_context_window: MAX_CONTEXT_WINDOW,
        efforts: UP_TO_ULTRA,
        default_effort: "low",
    },
    Builtin {
        slug: "gpt-6-sol",
        display_name: "GPT-6-Sol",
        description: "Previous generation workhorse model.",
        priority: 3,
        max_context_window: MAX_CONTEXT_WINDOW,
        efforts: UP_TO_ULTRA,
        default_effort: "medium",
    },
    Builtin {
        slug: "gpt-6-luna",
        display_name: "GPT-6-Luna",
        description: "Fast and affordable model for easier tasks.",
        priority: 4,
        max_context_window: MAX_CONTEXT_WINDOW,
        efforts: UP_TO_MAX,
        default_effort: "medium",
    },
    Builtin {
        slug: "gpt-5.6-sol",
        display_name: "GPT-5.6-Sol",
        description: "Older generation workhorse model.",
        priority: 5,
        max_context_window: MAX_CONTEXT_WINDOW,
        efforts: UP_TO_ULTRA,
        default_effort: "low",
    },
    Builtin {
        slug: "gpt-5.6-terra",
        display_name: "GPT-5.6-Terra",
        description: "Older balanced model for straightforward work.",
        priority: 8,
        max_context_window: MAX_CONTEXT_WINDOW,
        efforts: UP_TO_ULTRA,
        default_effort: "medium",
    },
    Builtin {
        slug: "gpt-5.6-luna",
        display_name: "GPT-5.6-Luna",
        description: "Older fast and efficient model.",
        priority: 9,
        max_context_window: MAX_CONTEXT_WINDOW,
        efforts: UP_TO_MAX,
        default_effort: "medium",
    },
    Builtin {
        slug: "gpt-5.5",
        display_name: "GPT-5.5",
        description: "Legacy coding model.",
        priority: 13,
        max_context_window: CONTEXT_WINDOW,
        efforts: UP_TO_XHIGH,
        default_effort: "medium",
    },
];

/// Model id prefixes of the reasoning families. Every model the subscription serves is
/// one of them; on the API, older chat models are not, and they refuse the `reasoning`
/// parameter and encrypted reasoning.
const REASONING_PREFIXES: &[&str] = &["gpt-5", "gpt-6", "o1", "o3", "o4", "codex-"];

/// The entries of the built-in table, as a catalog from the backend would list them.
pub(crate) fn builtin_entries() -> Vec<CatalogEntry> {
    TABLE
        .iter()
        .map(|builtin| CatalogEntry {
            slug: builtin.slug.to_owned(),
            display_name: Some(builtin.display_name.to_owned()),
            description: Some(builtin.description.to_owned()),
            priority: builtin.priority,
            visibility: Visibility::List,
            context_window: Some(CONTEXT_WINDOW),
            max_context_window: Some(builtin.max_context_window),
            supported_reasoning_levels: builtin
                .efforts
                .iter()
                .map(|effort| (*effort).into())
                .collect(),
            default_reasoning_level: Some(builtin.default_effort.to_owned()),
            apply_patch_tool_type: Some(crate::catalog::FREEFORM.to_owned()),
            prefer_websockets: true,
            supported_in_api: true,
        })
        .collect()
}

/// True when `model` belongs to a reasoning family, so a request for it carries the
/// `reasoning` parameter and asks for encrypted reasoning back. Chat variants of a
/// reasoning family (such as `gpt-5-chat-latest`) do not reason.
pub(crate) fn is_reasoning_model(model: &str) -> bool {
    REASONING_PREFIXES.iter().any(|prefix| model.starts_with(prefix)) && !model.contains("-chat")
}

#[cfg(test)]
mod tests;

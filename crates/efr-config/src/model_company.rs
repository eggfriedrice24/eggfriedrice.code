//! [`ModelCompany`] and [`ForeignModel`]: a `[model] name` that names a model of
//! another company than the one of `[model] provider`.
//!
//! The company of a model comes from the form of its id: `claude-...` is an Anthropic
//! model, `gpt-...`, `chatgpt-...`, `codex-...` and `o1`, `o3`, `o4` (with or without a
//! `-...` tail) are OpenAI models. Any other id belongs to no known company, so it is
//! never foreign. An id in the config's own list of the provider's company
//! (`[openai] models` or `[anthropic] models`) is never foreign either: the user says
//! that the provider serves it, such as through a proxy at its base URL.

use std::fmt;

use crate::Settings;

/// The prefixes of OpenAI model ids, besides the `o<digit>` reasoning models.
const OPENAI_PREFIXES: &[&str] = &["gpt-", "chatgpt-", "codex-"];

/// The prefix of Anthropic model ids.
const ANTHROPIC_PREFIX: &str = "claude-";

/// A company whose models a provider serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ModelCompany {
    /// `openai-subscription` and `openai-api`.
    OpenAi,
    /// `anthropic-api`.
    Anthropic,
}

impl ModelCompany {
    /// The company of the model `id`, by the form of the id; `None` when the form
    /// names no company that efr knows.
    pub fn of_model(id: &str) -> Option<ModelCompany> {
        if id.starts_with(ANTHROPIC_PREFIX) {
            return Some(ModelCompany::Anthropic);
        }
        if OPENAI_PREFIXES.iter().any(|prefix| id.starts_with(prefix)) || is_o_series(id) {
            return Some(ModelCompany::OpenAi);
        }
        None
    }

    /// The company of the provider `provider`; `None` for a provider that efr does not
    /// know.
    pub fn of_provider(provider: &str) -> Option<ModelCompany> {
        match provider {
            "openai-subscription" | "openai-api" => Some(ModelCompany::OpenAi),
            "anthropic-api" => Some(ModelCompany::Anthropic),
            _ => None,
        }
    }

    /// The company's name, as a person reads it.
    pub fn name(self) -> &'static str {
        match self {
            ModelCompany::OpenAi => "OpenAI",
            ModelCompany::Anthropic => "Anthropic",
        }
    }
}

/// True for `o` and one digit, alone or before `-`, such as `o3` and `o4-mini`.
fn is_o_series(id: &str) -> bool {
    let bytes = id.as_bytes();
    matches!(bytes, [b'o', digit, rest @ ..] if digit.is_ascii_digit() && matches!(rest, [] | [b'-', ..]))
}

/// A `[model] name` that is a model of another company than `[model] provider`. Every
/// turn that does not name its own model then fails, so efrd warns at start and at
/// a reload, `efr config check` says so, and a prompt that uses it fails with
/// [`fix`](Self::fix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForeignModel<'a> {
    /// The value of `[model] name`.
    pub name: &'a str,
    /// The company of that model.
    pub company: ModelCompany,
    /// The value of `[model] provider`.
    pub provider: &'a str,
}

impl ForeignModel<'_> {
    /// What the user changes, in one sentence.
    pub fn fix(&self) -> String {
        format!("set [model] name to a model of {}, or remove it", self.provider)
    }
}

impl fmt::Display for ForeignModel<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[model] name {} is a model of {}, but [model] provider is {}",
            self.name,
            self.company.name(),
            self.provider
        )
    }
}

impl Settings {
    /// `[model] name` when it is a model of another company than `[model] provider`
    /// and the config's list of the provider's company does not name it.
    pub fn foreign_model(&self) -> Option<ForeignModel<'_>> {
        let name = self.model.name.as_deref()?;
        let provider = self.model.provider.as_str();
        let own = ModelCompany::of_provider(provider)?;
        let company = ModelCompany::of_model(name)?;
        if company == own {
            return None;
        }
        let listed = match own {
            ModelCompany::OpenAi => &self.openai.models,
            ModelCompany::Anthropic => &self.anthropic.models,
        };
        if listed.iter().flatten().any(|entry| entry.id() == name) {
            return None;
        }
        Some(ForeignModel { name, company, provider })
    }
}

#[cfg(test)]
mod tests;

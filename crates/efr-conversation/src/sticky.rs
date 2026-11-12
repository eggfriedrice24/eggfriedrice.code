//! The model of a conversation: the newest pick holds until a prompt picks another.
//!
//! A prompt that names a model picks it for the conversation, from a terminal or from
//! the phone. A later prompt that names none, and a turn that job notices start, run
//! on the model of the turn before, while that turn's prompt picked it (or took it from
//! a pick before) and the model list still has it; else on the config's default. The
//! turn records the model as its own, so the pick goes from turn to turn and never
//! depends on how far back the picking prompt is. The effort of a pick goes along with
//! its model, and so does its provider while the provider's list still has the model:
//! a pick of one of two accounts stays on that account. A model that the list no
//! longer has, as after a logout, gives way to the config's default.
//!
//! A turn on another model, or on another provider, than the turn before records
//! `model_switched` and tells the new model once, in its preamble ([`switched`]).

use efr_protocol::{EffectiveSettings, SwitchedModel, TurnSettings};

use crate::ConversationConfig;

/// The settings that a turn asks for: the prompt's own, `asked`, with the model and the
/// effort of `previous`, the settings of the turn before, where the prompt names no
/// model and `previous` holds a pick that `config`'s model list still has.
pub(crate) fn asked(
    asked: &TurnSettings,
    previous: Option<&EffectiveSettings>,
    config: &ConversationConfig,
) -> TurnSettings {
    let Some(previous) = previous.filter(|previous| previous.overridden.model) else {
        return asked.clone();
    };
    let listed =
        config.models.is_empty() || config.models.iter().any(|info| info.id == previous.model);
    if asked.model.is_some() || !listed {
        return asked.clone();
    }
    // NOTE: the provider of the pick goes along while its list still has the model, so
    // a pick of one of two accounts stays on that account.
    let by_provider = previous.provider.as_ref().filter(|provider| {
        config.models.iter().any(|info| {
            info.id == previous.model && info.provider.as_deref() == Some(provider.as_str())
        })
    });
    let effort = match &asked.effort {
        Some(effort) => Some(effort.clone()),
        None if previous.overridden.effort => previous.effort.clone(),
        None => None,
    };
    TurnSettings {
        model: Some(previous.model.clone()),
        provider: by_provider.cloned(),
        effort,
        ..asked.clone()
    }
}

/// The provider, the model and the effort of `previous` when `now` runs on another
/// model, or on another provider that both name; `None` when the turn stays on the
/// model.
pub(crate) fn switched(
    previous: &EffectiveSettings,
    now: &EffectiveSettings,
) -> Option<SwitchedModel> {
    let other_provider = matches!(
        (&previous.provider, &now.provider),
        (Some(before), Some(after)) if before != after
    );
    (previous.model != now.model || other_provider).then(|| SwitchedModel {
        provider: previous.provider.clone(),
        model: previous.model.clone(),
        effort: previous.effort.clone(),
    })
}

#[cfg(test)]
mod tests;

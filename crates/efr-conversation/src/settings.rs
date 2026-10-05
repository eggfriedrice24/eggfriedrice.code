//! Turn settings: the mode, the model and the effort that a prompt asks for, over the
//! defaults of the config.
//!
//! [`resolve`] runs twice for every prompt: when `prompt.send` arrives, so a value that
//! cannot work fails before anything is recorded, and again when its turn starts,
//! against the settings of that moment, because the config may have changed while the
//! prompt waited. Each value is checked against the effective model list
//! ([`ConversationConfig::models`]): the model must be in it, and the effort must be
//! one that the model takes. A value from the config is checked the same way as one
//! from the prompt, so a default that no longer fits fails the turn with the choices
//! instead of reaching the backend.

use efr_protocol::{
    EffectiveSettings, ErrorBody, ErrorCode, ModelInfo, Origin, OverriddenSettings, TurnSettings,
    is_effort_word,
};
use serde_json::{Value, json};

use crate::{ConversationConfig, ConversationError};

/// The settings a turn runs with: `asked`, the prompt's own, over the defaults of
/// `config`. A turn from a remote origin runs with at most `cautious`
/// ([`efr_permissions::effective_mode`]).
pub(crate) fn resolve(
    asked: &TurnSettings,
    config: &ConversationConfig,
    origin: Origin,
) -> Result<EffectiveSettings, ConversationError> {
    // NOTE: the engine's own cap, so the recorded mode is the one the engine decides by.
    let mode = efr_permissions::effective_mode(asked.mode.unwrap_or(config.mode), origin);
    let model = asked.model.clone().unwrap_or_else(|| config.model.clone());
    let info = check_model(&model, asked.model.is_none(), &config.models)?;
    let effort = asked.effort.clone().or_else(|| config.effort.clone());
    if let Some(effort) = &effort {
        check_effort(effort, asked.effort.is_none(), &model, info)?;
    }
    Ok(EffectiveSettings {
        mode,
        model,
        effort,
        overridden: OverriddenSettings {
            mode: asked.mode.is_some(),
            model: asked.model.is_some(),
            effort: asked.effort.is_some(),
        },
    })
}

/// The model `model` from `models`; any model when the list is empty, because then the
/// provider does not say which models it serves and the backend's answer decides.
fn check_model<'a>(
    model: &str,
    from_config: bool,
    models: &'a [ModelInfo],
) -> Result<Option<&'a ModelInfo>, ConversationError> {
    if let Some(info) = models.iter().find(|info| info.id == model) {
        return Ok(Some(info));
    }
    if models.is_empty() && !model.trim().is_empty() {
        return Ok(None);
    }
    Err(ConversationError::InvalidSetting {
        setting: "model",
        value: model.to_owned(),
        model: None,
        choices: models.iter().map(|info| info.id.clone()).collect(),
        from_config,
    })
}

/// Checks `effort` against the efforts of `info`, the turn's model
/// ([`ModelInfo::takes_effort`]). A model that is not in the list takes any effort
/// word.
fn check_effort(
    effort: &str,
    from_config: bool,
    model: &str,
    info: Option<&ModelInfo>,
) -> Result<(), ConversationError> {
    let fits = info.map_or_else(|| is_effort_word(effort), |info| info.takes_effort(effort));
    if fits {
        return Ok(());
    }
    let efforts = info.map_or(&[][..], |info| info.efforts.as_slice());
    Err(ConversationError::InvalidSetting {
        setting: "effort",
        value: effort.to_owned(),
        model: Some(model.to_owned()),
        choices: efforts.to_vec(),
        from_config,
    })
}

/// The `turn_failed` body of a turn whose settings no longer fit: code `invalid`, the
/// error's sentence, and the setting, its value and the choices as data, so a client
/// can offer them.
pub(crate) fn failure(error: &ConversationError) -> ErrorBody {
    let body = ErrorBody::new(ErrorCode::Invalid, error.to_string());
    match error {
        ConversationError::InvalidSetting { setting, value, model, choices, .. } => {
            let mut data = json!({ "setting": setting, "value": value, "choices": choices });
            if let (Some(model), Value::Object(members)) = (model, &mut data) {
                members.insert("model".to_owned(), Value::String(model.clone()));
            }
            body.with_data(data)
        }
        _ => body,
    }
}

#[cfg(test)]
mod tests;

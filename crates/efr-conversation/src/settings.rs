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
//!
//! `auto` needs the sandbox. Without an available sandbox, or in a registered project at
//! the home directory, which is never a write root, the turn runs as `cautious` and
//! records why ([`ModeFallback`]); there is no `auto` with less. `full` runs only when
//! the daemon allows it ([`ConversationConfig::full_mode`]); else the prompt or the turn
//! fails. The check comes after the cap of a remote origin, so a phone's `full` runs as
//! `cautious` with no error. A `full` that comes from the config default, not from the
//! prompt, does not fail when the daemon refuses full mode (it runs as root, undo or
//! snapshots are off, the shell is no zsh): the turn runs as `auto` and records why, as
//! the config's `permissions.mode = "full"` needs `permissions.allow_full` anyway.
//!
//! A turn that job notices start (a wake turn) runs with at most `auto`: nobody asked
//! for its steps now, so it never runs in full mode. The cap comes before the check of
//! the daemon's gate, so such a turn needs no gate and never fails for it, and it
//! records one fallback with [`ModeFallback::WAKE_REASON`], plus the reason of `auto`
//! when the turn falls further to `cautious`.

use std::path::Path;

use crate::{ConversationConfig, ConversationError};
use efr_protocol::{
    EffectiveSettings, ErrorBody, ErrorCode, FullRefusal, Mode, ModeFallback, ModelInfo, Origin,
    OverriddenSettings, SandboxStatus, TurnSettings, is_effort_word,
};

/// Where a turn runs, for the fallback of `auto`: the probe's latest result, the root of
/// the turn's registered project, if any, and the home directory; and whether job
/// notices started the turn, for the cap of a wake turn.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Place<'a> {
    pub(crate) sandbox: &'a SandboxStatus,
    pub(crate) project_root: Option<&'a Path>,
    pub(crate) home: &'a Path,
    /// True for a turn that job notices started: it runs with at most `auto`.
    pub(crate) wake: bool,
}

/// The settings a turn runs with: `asked`, the prompt's own, over the defaults of
/// `config`. A turn from a remote origin runs with at most `cautious`
/// ([`efr_permissions::effective_mode`]), and an `auto` turn that `place` cannot hold
/// runs as `cautious` with the reason in `fallback`. A wake turn runs with at most
/// `auto`, with [`ModeFallback::WAKE_REASON`]. A `full` turn after those caps fails
/// unless the daemon allows full mode ([`ConversationConfig::full_mode`]); it never
/// falls back, except a `full` of the config default that the daemon refuses: that turn
/// runs as `auto`, or as `cautious` when `auto` cannot run either, with both reasons.
pub(crate) fn resolve(
    asked: &TurnSettings,
    config: &ConversationConfig,
    origin: Origin,
    place: Place<'_>,
) -> Result<EffectiveSettings, ConversationError> {
    // NOTE: the engine's own cap, so the recorded mode is the one the engine decides by.
    let mode = efr_permissions::effective_mode(asked.mode.unwrap_or(config.mode), origin);
    // NOTE: before the gate, so a wake turn of a full conversation runs as `auto` also
    // after the user turned full mode off (DF15).
    let woke = place.wake && mode == Mode::Full;
    let mode = if woke { Mode::Auto } else { mode };
    // NOTE: a default of full that the daemon refuses falls back, so no prompt that
    // names no mode fails while efrd runs as root or undo is off (OQF3).
    let refused_default =
        (mode == Mode::Full && asked.mode.is_none()).then_some(config.full_mode.refused).flatten();
    let mode = match refused_default {
        Some(_) => Mode::Auto,
        None => {
            if mode == Mode::Full {
                check_full(config)?;
            }
            mode
        }
    };
    let auto = (mode == Mode::Auto).then(|| auto_fallback(place)).flatten();
    let mode = if auto.is_some() { Mode::Cautious } else { mode };
    let fallback = match refused_default {
        Some(refusal) => Some(full_fallback(refusal, auto)),
        None if woke => Some(wake_fallback(auto)),
        None => auto,
    };
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
        fallback,
    })
}

/// Fails unless the daemon allows full mode: `permissions.allow_full` is true, and no
/// refusal of the daemon holds, such as efrd that runs as root.
fn check_full(config: &ConversationConfig) -> Result<(), ConversationError> {
    if !config.full_mode.allowed {
        return Err(ConversationError::FullModeOff);
    }
    match config.full_mode.refused {
        Some(refusal) => Err(ConversationError::FullModeRefused { refusal }),
        None => Ok(()),
    }
}

/// The fallback of a config default of full that the daemon refuses for `refusal`: to
/// `auto`, and further to `cautious` with the reason of `auto` when `auto` cannot run.
fn full_fallback(refusal: FullRefusal, auto: Option<ModeFallback>) -> ModeFallback {
    let mut reason = format!("full mode cannot run: {}", refusal.describe());
    if let Some(auto) = auto {
        reason.push_str("; ");
        reason.push_str(&auto.reason);
    }
    ModeFallback { asked: Mode::Full, reason }
}

/// The fallback of a wake turn of full mode: to `auto`, and further to `cautious` with
/// the reason of `auto` when `auto` cannot run.
fn wake_fallback(auto: Option<ModeFallback>) -> ModeFallback {
    let mut reason = ModeFallback::WAKE_REASON.to_owned();
    if let Some(auto) = auto {
        reason.push_str("; ");
        reason.push_str(&auto.reason);
    }
    ModeFallback { asked: Mode::Full, reason }
}

/// Why `place` cannot run an `auto` turn, or `None` when it can.
fn auto_fallback(place: Place<'_>) -> Option<ModeFallback> {
    let reason = if !place.sandbox.available {
        place
            .sandbox
            .reason
            .clone()
            .filter(|reason| !reason.trim().is_empty())
            .unwrap_or_else(|| ModeFallback::UNAVAILABLE_REASON.to_owned())
    } else if place.project_root.is_some_and(|root| place.home.starts_with(root)) {
        // NOTE: a project at `~` or above it would make every config below `~` that runs
        // code writable, and every path below it a named project.
        ModeFallback::HOME_PROJECT_REASON.to_owned()
    } else {
        return None;
    };
    Some(ModeFallback { asked: Mode::Auto, reason })
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
    match error.data() {
        Some(data) => body.with_data(data),
        None => body,
    }
}

#[cfg(test)]
mod tests;

//! `efr settings`: the mode, model and effort that a prompt with these flags and
//! variables would use, each with where it comes from and the choices.
//!
//! The zsh plugin runs it to check a value before `,mode`, `,model` or `,effort` keeps
//! it, and to show a setting. A value that the daemon would refuse is an error with the
//! choices (exit 2), so a terminal never keeps it.
//!
//! The model list and the default model are the daemon's (`models.list`). An empty list
//! takes any model id, as the daemon does. The default
//! mode and effort come from `config.toml` as `efr` reads it; the daemon reads its own
//! copy, so they match when both use the same config root.

use std::fmt::Write as _;

use efr_protocol::{Method, Mode, ModelInfo, ModelsList, ModelsListResult, Origin};

use crate::cli::TurnSettingsArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format;
use crate::output::Output;
use crate::settings::TurnDefaults;
use crate::turn_settings::{Asked, SettingSource};

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    args: &TurnSettingsArgs,
) -> Result<(), CliError> {
    let asked = Asked::read(ctx, args)?;
    let client = ctx.connect(Origin::Cli, None).await?;
    let list: ModelsListResult = client.call(Method::ModelsList(ModelsList {})).await?;
    let lines = resolve(&asked, &ctx.settings.turn, &list.models)?;
    out.out(&show(&lines))
}

/// One setting as `efr settings` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line {
    /// `mode`, `model` or `effort`.
    pub(crate) key: &'static str,
    /// The value; `None` for an effort that is not sent.
    pub(crate) value: Option<String>,
    pub(crate) source: SettingSource,
    /// The values the setting may take; empty when they are not known.
    pub(crate) choices: Vec<String>,
}

/// The settings that a prompt asking for `asked` would use, with `defaults` from the
/// config file and the daemon's model list `models`. Fails, with the choices, on a
/// model the list does not hold or an effort the model does not take. An empty list
/// takes any model, as the daemon does; the model is then unknown (`None`) when neither
/// the prompt nor the file names one.
pub(crate) fn resolve(
    asked: &Asked,
    defaults: &TurnDefaults,
    models: &[ModelInfo],
) -> Result<Vec<Line>, CliError> {
    let file = || defaults.path.clone().map_or(SettingSource::Default, SettingSource::File);

    let (mode, mode_source) = match (&asked.mode, defaults.mode) {
        (Some(given), _) => (given.value, given.source.clone()),
        (None, Some(mode)) => (mode, file()),
        (None, None) => (Mode::default(), SettingSource::Default),
    };

    let ids = || models.iter().map(|model| model.id.clone()).collect::<Vec<_>>();
    // NOTE: an empty list means that the provider names no models (`openai-api` without
    // `[openai] models`); the daemon then takes any model id, and so does efr.
    let any_model = models.is_empty();
    let (model, info, model_source) = match &asked.model {
        Some(given) => match models.iter().find(|model| model.id == given.value) {
            Some(model) => (Some(model.id.clone()), Some(model), given.source.clone()),
            None if any_model && !given.value.trim().is_empty() => {
                (Some(given.value.clone()), None, given.source.clone())
            }
            None => {
                return Err(CliError::UnknownModel {
                    model: given.value.clone(),
                    from: given.source.clone(),
                    choices: ids(),
                });
            }
        },
        None => match models.iter().find(|model| model.default) {
            Some(model) => {
                let source = if defaults.model.as_deref() == Some(model.id.as_str()) {
                    file()
                } else {
                    SettingSource::DaemonDefault
                };
                (Some(model.id.clone()), Some(model), source)
            }
            None if any_model => match &defaults.model {
                Some(name) => (Some(name.clone()), None, file()),
                None => (None, None, SettingSource::DaemonDefault),
            },
            None => return Err(CliError::NoDefaultModel),
        },
    };
    let model_name = || model.clone().unwrap_or_else(|| "the daemon's default model".to_owned());
    let efforts = info.map_or(&[][..], |info| info.efforts.as_slice());

    let (effort, effort_source) = match (&asked.effort, &defaults.effort) {
        (Some(given), _) => (Some(given.value.clone()), given.source.clone()),
        (None, Some(effort)) => (Some(effort.clone()), file()),
        (None, None) => match info.and_then(|info| info.default_effort.as_ref()) {
            Some(effort) => {
                (Some(effort.clone()), SettingSource::ModelDefault { model: model_name() })
            }
            None => (None, SettingSource::Backend { model: model_name() }),
        },
    };
    // NOTE: a model whose efforts efr does not know takes any effort; the backend
    // decides then.
    if let Some(effort) = &effort
        && !efforts.is_empty()
        && !efforts.contains(effort)
    {
        return Err(CliError::UnknownEffort {
            model: model_name(),
            effort: effort.clone(),
            from: effort_source,
            choices: efforts.to_vec(),
        });
    }

    Ok(vec![
        Line {
            key: "mode",
            value: Some(mode.as_str().to_owned()),
            source: mode_source,
            choices: Mode::ALL.map(|mode| mode.as_str().to_owned()).to_vec(),
        },
        Line { key: "model", value: model, source: model_source, choices: ids() },
        Line { key: "effort", value: effort, source: effort_source, choices: efforts.to_vec() },
    ])
}

/// One `key = value  # source; choices: a, b` line per setting. The plugin picks a
/// line by its key, so each setting stays on one line.
pub(crate) fn show(lines: &[Line]) -> String {
    let mut out = String::new();
    for Line { key, value, source, choices } in lines {
        // NOTE: only a daemon without a model list leaves the model unknown here.
        let unset = if *key == "model" { "(the daemon's default)" } else { "(unset)" };
        let value = value.as_deref().unwrap_or(unset);
        let mut comment = source.to_string();
        if !choices.is_empty() {
            let _ = write!(comment, "; choices: {}", choices.join(", "));
        }
        let _ =
            writeln!(out, "{key} = {}  # {}", format::one_line(value), format::one_line(&comment));
    }
    out
}

#[cfg(test)]
mod tests;

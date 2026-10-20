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
//!
//! `--mode auto` also asks the daemon for its sandbox (`admin.status`): when the
//! sandbox is not available, a warning on stderr says that turns run as `cautious` and
//! why, at once, so `,mode auto` tells the truth before the first prompt.
//!
//! `--mode full` asks the daemon whether it allows full mode (`admin.status`). When the
//! config does not allow it, or the daemon cannot run it, the command fails with the
//! daemon's own text (exit 2), so the terminal keeps its old mode. Otherwise three lines
//! on stderr say what full mode does and what it does not read. A daemon that does not
//! say leaves the choice to `permissions.allow_full` as `efr` reads the file; the daemon
//! checks each prompt again.

use std::fmt::Write as _;

use efr_protocol::{
    AdminStatus, AdminStatusResult, FullModeStatus, Method, Mode, ModelInfo, ModelsList,
    ModelsListResult, Origin, is_effort_word,
};

use crate::cli::TurnSettingsArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format;
use crate::output::Output;
use crate::settings::TurnDefaults;
use crate::turn_settings::{Asked, SettingSource};

/// What `efr settings --mode full` writes on stderr when full mode may run: what it
/// does, what efr still reads, and what it does not read.
pub(crate) const FULL_WARNING: &str = "\
efr: full mode: commands run with your rights, with no sandbox and no questions.
efr: efr reads each line: it denies secrets and its own config, and asks for sudo, for a \
process that outlives the turn and for a delete of ~, / or a project.
efr: a script or a program that a line starts is not read; it can do what you can.
";

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    args: &TurnSettingsArgs,
) -> Result<(), CliError> {
    let asked = Asked::read(ctx, args)?;
    let client = ctx.connect(Origin::Cli, None).await?;
    let list: ModelsListResult = client.call(Method::ModelsList(ModelsList {})).await?;
    let lines = resolve(&asked, &ctx.settings.turn, &list.models)?;
    // `,mode auto` and `,mode full` check the value with `--mode`: say at once what the
    // turns will get, instead of at the next prompt.
    let chose = asked
        .mode
        .as_ref()
        .filter(|given| matches!(given.source, SettingSource::Flag(_)))
        .map(|given| given.value);
    let status = match chose {
        Some(Mode::Auto | Mode::Full) => daemon_status(&client).await,
        _ => None,
    };
    // NOTE: the gate comes before any output: the plugin keeps a value only when this
    // command succeeds, so a terminal never keeps a full mode that the daemon refuses.
    if chose == Some(Mode::Full) {
        let full = status.as_ref().and_then(|status| status.full_mode.as_ref());
        full_gate(full, ctx.settings.turn.allow_full)?;
    }
    out.out(&show(&lines))?;
    match chose {
        Some(Mode::Auto) => {
            if let Some(reason) = status.as_ref().and_then(sandbox_unavailable) {
                out.err(&format!("efr: {}\n", format::sandbox::fallback_warning(&reason)));
            }
        }
        Some(Mode::Full) => out.err(FULL_WARNING),
        _ => {}
    }
    Ok(())
}

/// The daemon's status; `None` when it cannot be asked.
async fn daemon_status(client: &efr_transport::client::Client) -> Option<AdminStatusResult> {
    match client.call::<AdminStatusResult>(Method::AdminStatus(AdminStatus {})).await {
        Ok(status) => Some(status),
        Err(error) => {
            tracing::debug!(error = %error, "the daemon's status could not be read");
            None
        }
    }
}

/// Why the daemon's sandbox is not available; `None` when it is, or when the daemon
/// does not report it.
fn sandbox_unavailable(status: &AdminStatusResult) -> Option<String> {
    let sandbox = status.sandbox.as_ref().filter(|sandbox| !sandbox.available)?;
    Some(sandbox.reason.clone().unwrap_or_else(|| "the probe gave no reason".to_owned()))
}

/// Whether a terminal may choose full mode: the daemon's `status` when it gives one,
/// else `allow_full`, the file's `permissions.allow_full` as `efr` reads it. Fails with
/// the text that the daemon gives a prompt in full mode.
pub(crate) fn full_gate(status: Option<&FullModeStatus>, allow_full: bool) -> Result<(), CliError> {
    match status {
        Some(FullModeStatus { allowed: false, .. }) => Err(CliError::FullModeOff),
        Some(FullModeStatus { refused: Some(refusal), .. }) => {
            Err(CliError::FullModeRefused { refusal: *refusal })
        }
        Some(_) => Ok(()),
        None if allow_full => Ok(()),
        None => Err(CliError::FullModeOff),
    }
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
    // NOTE: the daemon's own test, so a value that efr keeps is one the daemon takes.
    if let Some(effort) = &effort
        && !info.map_or_else(|| is_effort_word(effort), |info| info.takes_effort(effort))
    {
        if efforts.is_empty() {
            return Err(CliError::EffortNotAWord {
                model: model_name(),
                effort: effort.clone(),
                from: effort_source,
            });
        }
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

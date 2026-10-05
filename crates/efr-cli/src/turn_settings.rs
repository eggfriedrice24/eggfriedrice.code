//! The turn settings that a command asks for, and where each value comes from.
//!
//! A prompt may ask for a permission mode, a model and a reasoning effort. Each comes
//! from its flag (`--mode`, `--model`, `--effort`), else from its variable (`EFR_MODE`,
//! `EFR_MODEL`, `EFR_EFFORT`, which the zsh plugin hands over for the terminal), else
//! from the daemon's config when the turn starts. An empty variable counts as unset.
//!
//! The CLI checks only the mode's name. The model and the effort belong to the daemon's
//! model list, so the daemon checks them at `prompt.send`; `efr settings` checks them
//! against `models.list` ahead of a prompt.

use std::fmt;
use std::path::PathBuf;

use efr_protocol::{Mode, TurnSettings};
use efr_stdx::env::Var;

use crate::cli::TurnSettingsArgs;
use crate::context::Context;
use crate::error::CliError;

/// One value that a command asks for, and the flag or variable it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Given<T> {
    pub(crate) value: T,
    pub(crate) source: SettingSource,
}

/// The turn settings that a command asks for. A value left out comes from the config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Asked {
    pub(crate) mode: Option<Given<Mode>>,
    pub(crate) model: Option<Given<String>>,
    pub(crate) effort: Option<Given<String>>,
}

impl Asked {
    /// The settings that `args` and, for each one without a flag, its variable ask for.
    pub(crate) fn read(ctx: &Context, args: &TurnSettingsArgs) -> Result<Asked, CliError> {
        let var = |var| ctx.env.var(var).map_err(|source| CliError::Environment { source });
        let mode = match args.mode {
            Some(mode) => Some(Given { value: mode, source: SettingSource::Flag("--mode") }),
            None => match var(Var::Mode)? {
                Some(value) => match value.parse::<Mode>() {
                    Ok(mode) => Some(Given { value: mode, source: SettingSource::Var(Var::Mode) }),
                    Err(_) => return Err(CliError::UnknownMode { input: Var::Mode, value }),
                },
                None => None,
            },
        };
        let text = |flag: &Option<String>, name: &'static str, variable: Var| {
            Ok::<_, CliError>(match flag {
                Some(value) => {
                    Some(Given { value: value.clone(), source: SettingSource::Flag(name) })
                }
                None => var(variable)?
                    .map(|value| Given { value, source: SettingSource::Var(variable) }),
            })
        };
        Ok(Asked {
            mode,
            model: text(&args.model, "--model", Var::Model)?,
            effort: text(&args.effort, "--effort", Var::Effort)?,
        })
    }

    /// The settings as `prompt.send` carries them.
    pub(crate) fn to_wire(&self) -> TurnSettings {
        TurnSettings {
            mode: self.mode.as_ref().map(|given| given.value),
            model: self.model.as_ref().map(|given| given.value.clone()),
            effort: self.effort.as_ref().map(|given| given.value.clone()),
        }
    }
}

/// Where the value of a turn setting comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SettingSource {
    /// A flag of the command line, such as `--mode`.
    Flag(&'static str),
    /// A variable, such as `EFR_MODE`, which the zsh plugin hands over for the terminal.
    Var(Var),
    /// `config.toml` at this path.
    File(PathBuf),
    /// The built-in default.
    Default,
    /// The model that the daemon marks as its default in `models.list`.
    DaemonDefault,
    /// The model's own default effort.
    ModelDefault { model: String },
    /// No effort is sent, so the backend chooses one for the model.
    Backend { model: String },
}

impl fmt::Display for SettingSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingSource::Flag(flag) => f.write_str(flag),
            SettingSource::Var(var) => f.write_str(var.name()),
            SettingSource::File(path) => write!(f, "{}", path.display()),
            SettingSource::Default => f.write_str("default"),
            SettingSource::DaemonDefault => f.write_str("the daemon's default"),
            SettingSource::ModelDefault { model } => write!(f, "the default of {model}"),
            SettingSource::Backend { model } => {
                write!(f, "none is sent; the backend chooses for {model}")
            }
        }
    }
}

#[cfg(test)]
mod tests;

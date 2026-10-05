//! The daemon's settings: `config.toml`, read and checked by `efr-config`, then the
//! `EFR_*` variables, then the flags of `efrd`.
//!
//! Every value comes from the first of these that sets it, read from the last to the
//! first: the flags, the variables, the file, the built-in defaults.
//! `Settings::effective` prints every value with the place it came from. The keys, their
//! defaults and their checks belong to `efr-config` (see its `examples/config.toml`);
//! this layer only knows which variables and flags override which key.

use std::path::Path;

use efr_config::{ConfigError, Settings, Source};
use efr_stdx::env::{Env, Var};

use crate::DaemonError;

/// The variables that override a key: `EFR_LOG` and `EFR_SCREEN`.
const VARIABLES: &[(Var, &str)] = &[(Var::Log, "log"), (Var::Screen, "screen")];

/// The flags of `efrd` that override a config value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Flags {
    /// `--log`.
    pub log: Option<String>,
    /// `--screen`.
    pub screen: Option<String>,
}

impl Flags {
    /// Flags with the given `--log` and `--screen` values.
    pub fn new(log: Option<String>, screen: Option<String>) -> Self {
        Flags { log, screen }
    }
}

/// Reads `config.toml` from `config_dir` (a missing file is an empty one) and applies
/// `env` and `flags` over it.
pub fn load_settings(config_dir: &Path, env: &Env, flags: &Flags) -> Result<Settings, DaemonError> {
    let mut settings = Settings::load(config_dir).map_err(config)?;
    apply_overrides(&mut settings, env, flags)?;
    Ok(settings)
}

/// The settings from the file `path` with contents `text` (`None` when it does not
/// exist), then `env`, then `flags`.
pub fn resolve_settings(
    path: &Path,
    text: Option<&str>,
    env: &Env,
    flags: &Flags,
) -> Result<Settings, DaemonError> {
    let mut settings = Settings::parse(path, text).map_err(config)?;
    apply_overrides(&mut settings, env, flags)?;
    Ok(settings)
}

/// Lays `env`, then `flags`, over `settings`.
fn apply_overrides(settings: &mut Settings, env: &Env, flags: &Flags) -> Result<(), DaemonError> {
    for (var, key) in VARIABLES {
        if let Some(value) = env.var(*var).map_err(|source| DaemonError::Env { source })? {
            settings.apply_override(key, &value, Source::Env(*var)).map_err(config)?;
        }
    }
    let Flags { log, screen } = flags;
    for (value, key, flag) in [(log, "log", "--log"), (screen, "screen", "--screen")] {
        if let Some(value) = value {
            settings.apply_override(key, value, Source::Flag(flag)).map_err(config)?;
        }
    }
    Ok(())
}

fn config(source: ConfigError) -> DaemonError {
    DaemonError::Config { source }
}

#[cfg(test)]
mod tests;

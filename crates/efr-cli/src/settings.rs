//! The client's settings from `config.toml`: the `[render]` table, and the turn
//! defaults that `efr settings` shows (`permissions.mode`, `model.name`,
//! `model.effort`).
//!
//! `efr-config` reads and checks the whole file with the one schema that `efrd` uses
//! too, unknown keys refused, so a file the daemon would refuse is refused here as
//! well. A file the client cannot use costs a warning, never a failed prompt: the
//! defaults apply. The theme's name is checked here, because the themes live in
//! `efr-render`.
//!
//! ```toml
//! [render]
//! theme = "catppuccin-mocha"   # any name efr_render::Theme::from_name accepts
//! ```

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use efr_config::{CONFIG_FILE, ConfigError};
use efr_protocol::Mode;
use efr_render::Theme;

/// The client's settings and where they came from.
#[derive(Debug, Default)]
pub(crate) struct Settings {
    /// The theme for code blocks and diffs.
    pub(crate) theme: Theme,
    /// Where `theme` came from.
    pub(crate) theme_source: Source,
    /// The turn defaults that the file sets.
    pub(crate) turn: TurnDefaults,
    /// What went wrong while reading the file.
    pub(crate) warnings: Vec<Warning>,
}

/// The defaults of a turn's settings that `config.toml` sets; `None` for each one it
/// leaves out. The daemon reads its own copy of the file, so these are what a prompt
/// gets only when both read the same config root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TurnDefaults {
    /// The file, when it was read and is valid.
    pub(crate) path: Option<PathBuf>,
    /// `permissions.mode`.
    pub(crate) mode: Option<Mode>,
    /// `model.name`.
    pub(crate) model: Option<String>,
    /// `model.effort`.
    pub(crate) effort: Option<String>,
}

/// Where a setting came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Source {
    /// The built-in default.
    #[default]
    Default,
    /// `config.toml` at this path.
    File(PathBuf),
}

/// A problem with `config.toml` that the defaults covered.
#[derive(Debug)]
pub(crate) enum Warning {
    /// The file exists but could not be read.
    Unreadable { path: PathBuf, source: io::Error },
    /// The file is not valid TOML, has an unknown key, or a value `efr-config` refuses.
    Invalid { path: PathBuf, source: Box<ConfigError> },
    /// `render.theme` names no embedded theme.
    UnknownTheme { path: PathBuf, name: String },
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Warning::Unreadable { path, source } => {
                write!(f, "{} could not be read: {source}", path.display())
            }
            Warning::Invalid { path, source } => {
                write!(f, "{} is not valid: {}", path.display(), describe(source))
            }
            Warning::UnknownTheme { path, name } => {
                write!(f, "{} names the theme {name:?}, which does not exist", path.display())
            }
        }
    }
}

/// What is wrong in `error` on one line, with its key and place when it has them, such
/// as `unknown field ... (shell.idle_minuets, line 2, column 1)`.
pub(crate) fn describe(error: &ConfigError) -> String {
    let mut text = reason(error);
    match (error.location(), error.key()) {
        (Some(at), Some(key)) => text.push_str(&format!(" ({key}, {at})")),
        (Some(at), None) => text.push_str(&format!(" ({at})")),
        (None, Some(key)) => text.push_str(&format!(" ({key})")),
        (None, None) => {}
    }
    crate::format::one_line(&text)
}

/// Why `text`, the contents of the config file `path` (`None`: no file), cannot be
/// used, as `efrd` and `efr` see it: the daemon's checks, then the theme's name.
pub(crate) fn problem(path: &Path, text: Option<&str>) -> Option<String> {
    let settings = match efr_config::Settings::parse(path, text) {
        Ok(settings) => settings,
        Err(error) => return Some(describe(&error)),
    };
    let name = settings.render.theme?;
    Theme::from_name(&name)
        .is_err()
        .then(|| format!("render.theme names the theme {name:?}, which does not exist"))
}

/// What is wrong, without the path that the warning already names.
fn reason(error: &ConfigError) -> String {
    match error {
        // NOTE: the parser's Display quotes the file with a caret under the error; the
        // place follows the reason instead.
        ConfigError::Parse { source, .. } => source.message().to_owned(),
        ConfigError::Invalid { key, value, expected, .. } => {
            format!("{key} = {value} is not {expected}")
        }
        other => {
            let mut text = other.to_string();
            let mut source = std::error::Error::source(other);
            while let Some(cause) = source {
                text.push_str(": ");
                text.push_str(&cause.to_string());
                source = cause.source();
            }
            text
        }
    }
}

impl Settings {
    /// Reads `config.toml` from the config root `config_dir`.
    pub(crate) async fn load(config_dir: &Path) -> Settings {
        let path = config_dir.join(CONFIG_FILE);
        match tokio::fs::read_to_string(&path).await {
            Ok(text) => Settings::parse(&path, &text),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Settings::default(),
            Err(source) => Settings {
                warnings: vec![Warning::Unreadable { path, source }],
                ..Settings::default()
            },
        }
    }

    /// The settings in `text`, the contents of the file at `path`.
    pub(crate) fn parse(path: &Path, text: &str) -> Settings {
        let file = match efr_config::Settings::parse(path, Some(text)) {
            Ok(file) => file,
            Err(source) => {
                let path = path.to_path_buf();
                return Settings {
                    warnings: vec![Warning::Invalid { path, source: Box::new(source) }],
                    ..Settings::default()
                };
            }
        };
        let mut settings = Settings::default();
        let from_file = |key: &str| file.source(key) == efr_config::Source::File;
        settings.turn = TurnDefaults {
            path: Some(path.to_path_buf()),
            mode: from_file("permissions.mode").then_some(file.permissions.mode),
            model: file.model.name.clone(),
            effort: file.model.effort.clone(),
        };
        if let Some(name) = file.render.theme {
            match Theme::from_name(&name) {
                Ok(theme) => {
                    settings.theme = theme;
                    settings.theme_source = Source::File(path.to_path_buf());
                }
                Err(_) => {
                    settings.warnings.push(Warning::UnknownTheme { path: path.to_path_buf(), name })
                }
            }
        }
        settings
    }
}

#[cfg(test)]
mod tests;

//! The client's settings from `config.toml`: the `[render]` table.
//!
//! The daemon owns `config.toml` and validates all of it (`efr-daemon/src/config.rs`,
//! with unknown fields denied). The CLI reads only the table it needs and ignores the
//! rest, so a key it does not know is the daemon's business, not an error here. A
//! file it cannot use costs a warning, never a failed prompt: the defaults apply.
//!
//! ```toml
//! [render]
//! theme = "catppuccin-mocha"   # any name efr_render::Theme::from_name accepts
//! ```

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use efr_render::Theme;
use serde::Deserialize;

/// The file name under the config root.
pub(crate) const CONFIG_FILE: &str = "config.toml";

/// The client's settings and where they came from.
#[derive(Debug, Default)]
pub(crate) struct Settings {
    /// The theme for code blocks and diffs.
    pub(crate) theme: Theme,
    /// What went wrong while reading the file.
    pub(crate) warnings: Vec<Warning>,
}

/// A problem with `config.toml` that the defaults covered.
#[derive(Debug)]
pub(crate) enum Warning {
    /// The file exists but could not be read.
    Unreadable { path: PathBuf, source: io::Error },
    /// The file is not valid TOML, or `[render]` has the wrong shape.
    Invalid { path: PathBuf, source: Box<toml::de::Error> },
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
                let reason = source.message();
                write!(f, "{} is not valid: {reason}", path.display())
            }
            Warning::UnknownTheme { path, name } => {
                write!(f, "{} names the theme {name:?}, which does not exist", path.display())
            }
        }
    }
}

/// The part of `config.toml` that the CLI reads. Other tables are ignored.
#[derive(Debug, Default, Deserialize)]
struct File {
    #[serde(default)]
    render: RenderTable,
}

#[derive(Debug, Default, Deserialize)]
struct RenderTable {
    theme: Option<String>,
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
        let file: File = match toml::from_str(text) {
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
        if let Some(name) = file.render.theme {
            match Theme::from_name(&name) {
                Ok(theme) => {
                    settings.theme = theme;
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

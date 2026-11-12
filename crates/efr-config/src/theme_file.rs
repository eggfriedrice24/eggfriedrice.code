//! [`ThemeFile`]: the theme file that `render.palette` names, so a design system can
//! give `efr` its colours from one generated file.
//!
//! ```toml
//! code_theme = "../bat/eggfriedrice.tmTheme"             # optional
//! code_theme_ansi = "../bat/eggfriedrice-ansi.tmTheme"   # optional
//!
//! [colors]
//! accent = "#f2c14e"
//! muted = 8
//! code_bg = "#121722"
//! diff.add = "green"
//! diff.add_bg = "#192c17"
//! ```
//!
//! The `[colors]` table has the keys of `[render.colors]`. `code_theme` is the path of
//! a `.tmTheme` file: absolute, `~/...`, or relative to the theme file's directory.
//! `code_theme_ansi` is the path of a `.tmTheme` file in bat's ANSI encoding (an alpha
//! of 0 names a slot of the 16 colours), which `efr` uses in 16 colours in place of
//! `code_theme`. Unknown keys are an error, as in `config.toml`.

use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use toml_edit::Document;

use crate::location::{key_at, location, span_of};
use crate::tables::render::{THEME_COLOR_KEYS, expand_home};
use crate::validate::{self, Invalid};
use crate::{ConfigError, RenderColors};

/// A theme file, read and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ThemeFile {
    /// The file it was read from.
    pub path: PathBuf,
    /// The path of a `.tmTheme` file for code, as the file writes it. The code theme
    /// wins over `render.theme`; [`code_theme_path`](Self::code_theme_path) resolves it.
    pub code_theme: Option<PathBuf>,
    /// The path of a `.tmTheme` file in bat's ANSI encoding, as the file writes it, for
    /// code in 16 colours: it wins over `code_theme` there.
    /// [`code_theme_ansi_path`](Self::code_theme_ansi_path) resolves it.
    pub code_theme_ansi: Option<PathBuf>,
    /// The colour of each role. `[render.colors]` wins over these.
    pub colors: RenderColors,
}

/// The file as it is written.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Written {
    code_theme: Option<PathBuf>,
    code_theme_ansi: Option<PathBuf>,
    colors: RenderColors,
}

impl ThemeFile {
    /// Reads and checks the theme file at `path`. This blocks: async code calls it in
    /// `spawn_blocking`, or reads the file itself and calls [`parse`](Self::parse).
    pub fn load(path: &Path) -> Result<ThemeFile, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source: io::Error| {
            ConfigError::ThemeRead { path: path.to_path_buf(), source }
        })?;
        ThemeFile::parse(path, &text)
    }

    /// The theme file at `path` with contents `text`. Every key is checked: unknown
    /// keys, types and colours.
    pub fn parse(path: &Path, text: &str) -> Result<ThemeFile, ConfigError> {
        let written: Written = toml::from_str(text).map_err(|source| {
            let span = source.span();
            let document = Document::parse(text.to_owned()).ok();
            ConfigError::ThemeParse {
                path: path.to_path_buf(),
                location: span.as_ref().map(|span| location(text, span.start)),
                key: span.zip(document.as_ref()).and_then(|(span, doc)| key_at(doc, span.start)),
                source: Box::new(source),
            }
        })?;
        let invalid = |Invalid { key, value, expected }: Invalid| {
            let location = Document::parse(text.to_owned())
                .ok()
                .and_then(|document| span_of(&document, key))
                .map(|span| location(text, span.start));
            ConfigError::ThemeInvalid { path: path.to_path_buf(), key, value, expected, location }
        };
        for (key, path) in
            [("code_theme", &written.code_theme), ("code_theme_ansi", &written.code_theme_ansi)]
        {
            if path.as_ref().is_some_and(|path| path.as_os_str().is_empty()) {
                return Err(invalid(Invalid {
                    key,
                    value: "\"\"".to_owned(),
                    expected: "the path of a .tmTheme file",
                }));
            }
        }
        validate::colors(&written.colors, &THEME_COLOR_KEYS).map_err(invalid)?;
        Ok(ThemeFile {
            path: path.to_path_buf(),
            code_theme: written.code_theme,
            code_theme_ansi: written.code_theme_ansi,
            colors: written.colors,
        })
    }

    /// The path of the code theme: `~` replaced by `home`, and a relative path taken
    /// from the theme file's directory.
    pub fn code_theme_path(&self, home: &Path) -> Option<PathBuf> {
        self.resolve(self.code_theme.as_deref()?, home)
    }

    /// The path of the code theme for 16 colours, resolved as
    /// [`code_theme_path`](Self::code_theme_path) resolves its own.
    pub fn code_theme_ansi_path(&self, home: &Path) -> Option<PathBuf> {
        self.resolve(self.code_theme_ansi.as_deref()?, home)
    }

    fn resolve(&self, path: &Path, home: &Path) -> Option<PathBuf> {
        let path = expand_home(path, home);
        if path.is_absolute() {
            return Some(path);
        }
        let directory = self.path.parent().unwrap_or_else(|| Path::new(""));
        Some(directory.join(path))
    }
}

#[cfg(test)]
mod tests;

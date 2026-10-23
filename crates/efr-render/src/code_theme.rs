//! A code theme of the user's own: a `.tmTheme` file, as bat and Sublime Text read
//! it. The caller reads the file; this crate only parses the bytes, so it still does
//! no IO.

use std::fmt;
use std::io::Cursor;

use syntect::highlighting::{Highlighter, Theme as SyntectTheme, ThemeSet};

use crate::error::RenderError;
use crate::highlight::ThemeRef;

/// A code theme from a `.tmTheme` file. When [`RenderOptions`](crate::RenderOptions)
/// has one, code blocks and diffs use it and not the embedded [`Theme`](crate::Theme).
///
/// Clones share the parsed theme. Two code themes are equal only when one is a clone
/// of the other.
// NOTE: not `Copy`, because callers clone it and clippy rejects a clone of a value
// that is `Copy`.
#[derive(Clone)]
pub struct CodeTheme {
    theme: ThemeRef,
}

impl CodeTheme {
    /// Parses the contents of a `.tmTheme` file (an XML property list).
    ///
    /// The parsed theme and its highlighter stay in memory until the process ends, so
    /// that all code blocks share one highlighter. The CLI reads its theme file once
    /// for each process, so this memory does not grow.
    pub fn from_tmtheme(bytes: &[u8]) -> Result<CodeTheme, RenderError> {
        let theme = ThemeSet::load_from_reader(&mut Cursor::new(bytes))
            .map_err(|source| RenderError::CodeTheme { source })?;
        let theme: &'static SyntectTheme = Box::leak(Box::new(theme));
        let highlighter: &'static Highlighter<'static> =
            Box::leak(Box::new(Highlighter::new(theme)));
        Ok(CodeTheme { theme: ThemeRef { theme, highlighter } })
    }

    /// The name the file gives the theme, if it gives one.
    pub fn name(&self) -> Option<&str> {
        self.theme.theme.name.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn syntect(&self) -> &'static SyntectTheme {
        self.theme.theme
    }

    /// The theme with its highlighter, for a highlighted block.
    pub(crate) fn theme_ref(&self) -> ThemeRef {
        self.theme
    }
}

impl PartialEq for CodeTheme {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.theme.theme, other.theme.theme)
    }
}

impl Eq for CodeTheme {}

impl fmt::Debug for CodeTheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodeTheme").field("name", &self.name()).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;

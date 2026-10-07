//! A code theme of the user's own: a `.tmTheme` file, as bat and Sublime Text read
//! it. The caller reads the file; this crate only parses the bytes, so it still does
//! no IO.

use std::fmt;
use std::io::Cursor;
use std::sync::Arc;

use syntect::highlighting::{Theme as SyntectTheme, ThemeSet};

use crate::error::RenderError;

/// A code theme from a `.tmTheme` file. When [`RenderOptions`](crate::RenderOptions)
/// has one, code blocks and diffs use it and not the embedded [`Theme`](crate::Theme).
///
/// Clones share the parsed theme. Two code themes are equal only when one is a clone
/// of the other.
#[derive(Clone)]
pub struct CodeTheme {
    theme: Arc<SyntectTheme>,
}

impl CodeTheme {
    /// Parses the contents of a `.tmTheme` file (an XML property list).
    pub fn from_tmtheme(bytes: &[u8]) -> Result<CodeTheme, RenderError> {
        let theme = ThemeSet::load_from_reader(&mut Cursor::new(bytes))
            .map_err(|source| RenderError::CodeTheme { source })?;
        Ok(CodeTheme { theme: Arc::new(theme) })
    }

    /// The name the file gives the theme, if it gives one.
    pub fn name(&self) -> Option<&str> {
        self.theme.name.as_deref()
    }

    pub(crate) fn syntect(&self) -> &SyntectTheme {
        &self.theme
    }
}

impl PartialEq for CodeTheme {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.theme, &other.theme)
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

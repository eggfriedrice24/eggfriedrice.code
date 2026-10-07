//! `[render]`: how `efr` shows replies, and the colours of its roles.
//!
//! A role colour is `#rrggbb`, an ANSI slot from 0 to 15 (a number, or a number in a
//! string), or the name of one of the 16 colours: `black`, `red`, `green`, `yellow`,
//! `blue`, `magenta`, `cyan`, `white`, each also as `bright-<name>`. The roles are the
//! ones of `efr-render` (`efr_render::Role`): this crate may not depend on it, so the
//! names are listed here as well, and `efr`, which depends on both, must keep the two
//! lists equal.

use std::fmt;
use std::path::{Path, PathBuf};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize};

/// The names of the colour roles, in the order of their keys. `diff.add` is the key
/// `add` of the table `diff`.
pub const COLOR_ROLES: [&str; 13] = [
    "text",
    "muted",
    "accent",
    "heading",
    "link",
    "code",
    "success",
    "warning",
    "error",
    "quote",
    "diff.add",
    "diff.remove",
    "diff.hunk",
];

/// The dotted key of each role in `config.toml`, in the order of [`COLOR_ROLES`].
pub(crate) const CONFIG_COLOR_KEYS: [&str; 13] = [
    "render.colors.text",
    "render.colors.muted",
    "render.colors.accent",
    "render.colors.heading",
    "render.colors.link",
    "render.colors.code",
    "render.colors.success",
    "render.colors.warning",
    "render.colors.error",
    "render.colors.quote",
    "render.colors.diff.add",
    "render.colors.diff.remove",
    "render.colors.diff.hunk",
];

/// The dotted key of each role in a theme file, in the order of [`COLOR_ROLES`].
pub(crate) const THEME_COLOR_KEYS: [&str; 13] = [
    "colors.text",
    "colors.muted",
    "colors.accent",
    "colors.heading",
    "colors.link",
    "colors.code",
    "colors.success",
    "colors.warning",
    "colors.error",
    "colors.quote",
    "colors.diff.add",
    "colors.diff.remove",
    "colors.diff.hunk",
];

/// What a role colour must be, for the error of a value that is not one.
pub(crate) const COLOR_EXPECTED: &str =
    "a colour: \"#rrggbb\", an ANSI slot from 0 to 15, or a name such as \"yellow\"";

/// The value of `render.theme` that picks `theme_dark` or `theme_light` by the
/// terminal's background.
pub const AUTO_THEME: &str = "auto";

/// The default of `render.theme_dark`.
const DEFAULT_THEME_DARK: &str = "catppuccin-mocha";

/// The default of `render.theme_light`.
const DEFAULT_THEME_LIGHT: &str = "catppuccin-latte";

/// The names of the 16 colours, in the order of their ANSI slots from 0.
const NAMES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];

/// When `efr` shows the progress of a turn in the terminal's tab (OSC 9;4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Progress {
    /// Only in a terminal that is known to draw it: Ghostty 1.2 or later, kitty 0.47
    /// or later, Windows Terminal. Never inside tmux.
    #[default]
    Auto,
    /// Always, when stdout is a terminal.
    On,
    /// Never.
    Off,
}

impl Progress {
    /// The name in the file, such as `auto`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Progress::Auto => "auto",
            Progress::On => "on",
            Progress::Off => "off",
        }
    }
}

/// `[render]`: how `efr` shows replies. Only the client reads it; the daemon accepts it
/// so one file serves both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct RenderSettings {
    /// The colour theme of code blocks and diffs, such as `catppuccin-mocha`. Unset:
    /// the terminal's own 16 colours. `auto` takes `theme_dark` or `theme_light`, as
    /// the terminal's background is dark or light (`EFR_TERMINAL_BG`). A `code_theme`
    /// in the palette file wins over it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// The theme that `theme = "auto"` takes on a dark background, and when the
    /// background is not known.
    pub theme_dark: String,
    /// The theme that `theme = "auto"` takes on a light background.
    pub theme_light: String,
    /// A theme file with a colour for each role (its `[colors]` table) and, as
    /// `code_theme`, the path of a `.tmTheme` file for code: an absolute path or
    /// `~/...`. Unset: no theme file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub palette: Option<PathBuf>,
    /// Motion in the status row of a running turn: the spinner turns and a band in
    /// the `text` role moves over the state in the `muted` role. `false` shows a still
    /// dot and no band; the time still counts.
    pub motion: bool,
    /// One muted line at the end of each turn: how long it took and the tokens it
    /// used, such as `done in 42s, 18.2k tokens in, 1.1k out`.
    pub turn_summary: bool,
    /// The progress bar of the terminal's tab (OSC 9;4) while a turn runs: `auto`
    /// (only in Ghostty 1.2 or later, kitty 0.47 or later and Windows Terminal, never
    /// inside tmux), `on` or `off`.
    pub progress: Progress,
    /// The colour of each role. A role set here wins over the palette file, and the
    /// palette file over the terminal's 16 colours.
    pub colors: RenderColors,
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings {
            theme: None,
            theme_dark: DEFAULT_THEME_DARK.to_owned(),
            theme_light: DEFAULT_THEME_LIGHT.to_owned(),
            palette: None,
            motion: true,
            turn_summary: true,
            progress: Progress::Auto,
            colors: RenderColors::default(),
        }
    }
}

impl RenderSettings {
    /// The palette file's path with a leading `~` replaced by `home`.
    pub fn palette_path(&self, home: &Path) -> Option<PathBuf> {
        self.palette.as_deref().map(|path| expand_home(path, home))
    }
}

/// The colour of each role: `[render.colors]` in `config.toml`, `[colors]` in a theme
/// file. A role that is not set keeps the colour of the layer below it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct RenderColors {
    /// Prose and plain lines. Unset: the terminal's foreground.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<ColorValue>,
    /// Notes, tool call lines, labels, rules and the end-of-turn line. Unset: dim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<ColorValue>,
    /// The spinner and the colour of headings. Unset: slot 3 (yellow).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<ColorValue>,
    /// Headings of level 1 and 2. Unset: the accent colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading: Option<ColorValue>,
    /// Links. Unset: slot 4 (blue).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<ColorValue>,
    /// Inline code. Unset: slot 6 (cyan).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<ColorValue>,
    /// Done task boxes and good news. Unset: slot 2 (green).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<ColorValue>,
    /// Questions and refusals, in bold. Unset: slot 3 (yellow).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<ColorValue>,
    /// Failures. Unset: slot 1 (red).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ColorValue>,
    /// The text of a quote, in italic. Unset: the terminal's foreground.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<ColorValue>,
    /// The lines of a diff.
    pub diff: DiffColors,
}

/// The colours of the lines of a diff: `diff.add`, `diff.remove` and `diff.hunk`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct DiffColors {
    /// Added lines. Unset: slot 2 (green).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add: Option<ColorValue>,
    /// Removed lines. Unset: slot 1 (red).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove: Option<ColorValue>,
    /// Hunk headers. Unset: slot 6 (cyan).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hunk: Option<ColorValue>,
}

impl RenderColors {
    /// Every role, in the order of [`COLOR_ROLES`], with its value when it is set.
    pub fn entries(&self) -> [(&'static str, Option<&ColorValue>); 13] {
        let [
            text,
            muted,
            accent,
            heading,
            link,
            code,
            success,
            warning,
            error,
            quote,
            add,
            remove,
            hunk,
        ] = COLOR_ROLES;
        [
            (text, self.text.as_ref()),
            (muted, self.muted.as_ref()),
            (accent, self.accent.as_ref()),
            (heading, self.heading.as_ref()),
            (link, self.link.as_ref()),
            (code, self.code.as_ref()),
            (success, self.success.as_ref()),
            (warning, self.warning.as_ref()),
            (error, self.error.as_ref()),
            (quote, self.quote.as_ref()),
            (add, self.diff.add.as_ref()),
            (remove, self.diff.remove.as_ref()),
            (hunk, self.diff.hunk.as_ref()),
        ]
    }

    /// These colours laid over `base`: a role set here wins, and a role not set here
    /// keeps the colour of `base`.
    #[must_use]
    pub fn over(&self, base: &RenderColors) -> RenderColors {
        let pick = |own: &Option<ColorValue>, below: &Option<ColorValue>| {
            own.clone().or_else(|| below.clone())
        };
        RenderColors {
            text: pick(&self.text, &base.text),
            muted: pick(&self.muted, &base.muted),
            accent: pick(&self.accent, &base.accent),
            heading: pick(&self.heading, &base.heading),
            link: pick(&self.link, &base.link),
            code: pick(&self.code, &base.code),
            success: pick(&self.success, &base.success),
            warning: pick(&self.warning, &base.warning),
            error: pick(&self.error, &base.error),
            quote: pick(&self.quote, &base.quote),
            diff: DiffColors {
                add: pick(&self.diff.add, &base.diff.add),
                remove: pick(&self.diff.remove, &base.diff.remove),
                hunk: pick(&self.diff.hunk, &base.diff.hunk),
            },
        }
    }

    /// Every role that is set, with its colour, in the order of [`COLOR_ROLES`]. A
    /// checked file has only valid colours; a value that is not one is left out.
    pub fn colors(&self) -> Vec<(&'static str, RoleColor)> {
        self.entries()
            .into_iter()
            .filter_map(|(role, value)| {
                value.and_then(ColorValue::color).map(|color| (role, color))
            })
            .collect()
    }

    /// The first role whose value is not a colour, with that value.
    pub(crate) fn first_invalid(&self) -> Option<(usize, &ColorValue)> {
        self.entries().into_iter().enumerate().find_map(|(index, (_, value))| {
            value.filter(|value| value.color().is_none()).map(|value| (index, value))
        })
    }
}

/// A colour that a role can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RoleColor {
    /// One of the terminal's 16 colours, 0 to 15: it follows the terminal's theme.
    Slot(u8),
    /// 24-bit colour: red, green and blue. Without truecolor the nearest of the 16
    /// colours shows.
    Rgb(u8, u8, u8),
}

/// A role colour as the file writes it: a string or a number. [`ColorValue::color`]
/// reads it; the checks of the file refuse a value that is not a colour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ColorValue {
    /// A number, such as `3`: an ANSI slot.
    Number(i64),
    /// A string, such as `"#f2c14e"`, `"yellow"` or `"3"`.
    Text(String),
}

impl ColorValue {
    /// The colour this value names, or `None` when it names none.
    pub fn color(&self) -> Option<RoleColor> {
        match self {
            ColorValue::Number(number) => slot(*number),
            ColorValue::Text(text) => parse_color(text),
        }
    }

    /// The value as TOML, for an error message.
    pub(crate) fn as_toml(&self) -> String {
        match self {
            ColorValue::Number(number) => number.to_string(),
            ColorValue::Text(text) => toml::Value::String(text.clone()).to_string(),
        }
    }
}

fn slot(number: i64) -> Option<RoleColor> {
    u8::try_from(number).ok().filter(|slot| *slot < 16).map(RoleColor::Slot)
}

/// Reads a colour as the JSON schema's pattern describes it: `#rrggbb` in either case,
/// one or two digits, or a lowercase name.
fn parse_color(text: &str) -> Option<RoleColor> {
    if let Some(hex) = text.strip_prefix('#') {
        if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let part = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
        return Some(RoleColor::Rgb(part(0)?, part(2)?, part(4)?));
    }
    if (1..=2).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_digit()) {
        return text.parse::<i64>().ok().and_then(slot);
    }
    let (name, bright) = match text.strip_prefix("bright-") {
        Some(name) => (name, 8),
        None => (text, 0),
    };
    let index = NAMES.iter().position(|known| *known == name)?;
    u8::try_from(index).ok().map(|index| RoleColor::Slot(index + bright))
}

impl<'de> Deserialize<'de> for ColorValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ColorVisitor;

        impl Visitor<'_> for ColorVisitor {
            type Value = ColorValue;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(COLOR_EXPECTED)
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<ColorValue, E> {
                Ok(ColorValue::Number(value))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<ColorValue, E> {
                i64::try_from(value)
                    .map(ColorValue::Number)
                    .map_err(|_| E::invalid_value(de::Unexpected::Unsigned(value), &self))
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<ColorValue, E> {
                Ok(ColorValue::Text(value.to_owned()))
            }
        }

        deserializer.deserialize_any(ColorVisitor)
    }
}

impl JsonSchema for ColorValue {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ColorValue".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": ["string", "integer"],
            "pattern": "^(#[0-9a-fA-F]{6}|[0-9]{1,2}|(bright-)?(black|red|green|yellow|blue|magenta|cyan|white))$",
            "minimum": 0,
            "maximum": 15
        })
    }
}

/// `path` with a leading `~` component replaced by `home`.
pub(crate) fn expand_home(path: &Path, home: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests;

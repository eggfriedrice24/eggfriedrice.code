//! Colour roles: every colour of prose and of the CLI's own lines goes through a
//! [`Role`], and a [`Palette`] maps each role to a colour.
//!
//! The default palette uses only the terminal's 16 colours, so the output follows the
//! terminal's theme, and text in scrollback changes colour with it. A role can get any
//! [`Colour`] instead, from the user's config or theme file. An RGB colour falls back
//! to the nearest of the 16 colours in 16-colour mode. Without colour (`NO_COLOR`) a
//! role keeps its attributes, and some roles get one in place of the colour: `muted`
//! is dim, `accent`, `heading` and `error` are bold.

use std::fmt;
use std::str::FromStr;

use crate::error::RenderError;
use crate::options::ColourMode;
use crate::style::{BLUE, CYAN, Colour, GREEN, RED, Style, YELLOW, reduce_colour};

/// What a piece of output is, for its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    /// Prose and plain fact lines. Default: the terminal's own foreground.
    Text,
    /// Notes, tool call lines, output tails, labels, rules, status text and the
    /// end-of-turn line. Default: dim.
    Muted,
    /// The spinner and the colour of headings. Default: palette entry 3 (yellow).
    Accent,
    /// Headings of level 1 and 2; they are bold, level 1 also underlined. Default: the
    /// accent colour.
    Heading,
    /// Links: underlined. Default: palette entry 4 (blue).
    Link,
    /// Inline code. Default: palette entry 6 (cyan).
    Code,
    /// Done task boxes and other good news. Default: palette entry 2 (green).
    Success,
    /// Questions, the full-rights line, untrusted marks and refusals: bold. Default:
    /// palette entry 3 (yellow).
    Warning,
    /// Failures, such as a failed exit code. Default: palette entry 1 (red).
    Error,
    /// The text of a quote: italic. Default: the terminal's own foreground.
    Quote,
    /// Added lines of a diff. Default: palette entry 2 (green).
    DiffAdd,
    /// Removed lines of a diff. Default: palette entry 1 (red).
    DiffRemove,
    /// Hunk headers of a diff. Default: palette entry 6 (cyan).
    DiffHunk,
}

impl Role {
    /// Every role, in the order of the config keys.
    pub const ALL: [Role; 13] = [
        Role::Text,
        Role::Muted,
        Role::Accent,
        Role::Heading,
        Role::Link,
        Role::Code,
        Role::Success,
        Role::Warning,
        Role::Error,
        Role::Quote,
        Role::DiffAdd,
        Role::DiffRemove,
        Role::DiffHunk,
    ];

    /// The name of the role in the config and in a theme file, such as `muted` or
    /// `diff.add`.
    pub const fn name(self) -> &'static str {
        match self {
            Role::Text => "text",
            Role::Muted => "muted",
            Role::Accent => "accent",
            Role::Heading => "heading",
            Role::Link => "link",
            Role::Code => "code",
            Role::Success => "success",
            Role::Warning => "warning",
            Role::Error => "error",
            Role::Quote => "quote",
            Role::DiffAdd => "diff.add",
            Role::DiffRemove => "diff.remove",
            Role::DiffHunk => "diff.hunk",
        }
    }

    /// The role named `name`, as [`Role::name`] writes it.
    pub fn from_name(name: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|role| role.name() == name)
    }

    const fn index(self) -> usize {
        self as usize
    }

    /// The colour the role has when the palette sets none, and the attributes it
    /// always has and has only without a colour.
    const fn default_look(self) -> Look {
        const fn look(colour: Option<Colour>, always: Style, without_colour: Style) -> Look {
            Look { colour, always, without_colour }
        }
        let plain = Style::PLAIN;
        match self {
            Role::Text => look(None, plain, plain),
            Role::Muted => look(None, plain, plain.dim()),
            Role::Accent => look(Some(YELLOW), plain, plain.bold()),
            // The colour comes from the accent; see `Palette::colour`.
            Role::Heading => look(None, plain.bold(), plain),
            Role::Link => look(Some(BLUE), plain.underline(), plain),
            Role::Code | Role::DiffHunk => look(Some(CYAN), plain, plain),
            Role::Success | Role::DiffAdd => look(Some(GREEN), plain, plain),
            Role::Warning => look(Some(YELLOW), plain.bold(), plain),
            Role::Error => look(Some(RED), plain, plain.bold()),
            Role::Quote => look(None, plain.italic(), plain),
            Role::DiffRemove => look(Some(RED), plain, plain),
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Role {
    type Err = RenderError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Role::from_name(name).ok_or_else(|| RenderError::UnknownRole { name: name.to_owned() })
    }
}

/// How a role looks by default.
struct Look {
    colour: Option<Colour>,
    always: Style,
    without_colour: Style,
}

/// The colour of each [`Role`]. A role the palette does not set has its default from
/// the terminal's 16 colours (see each role). The default palette sets no role.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Palette {
    colours: [Option<Colour>; Role::ALL.len()],
}

impl Palette {
    /// The default palette: every role has its 16-colour default.
    pub fn new() -> Palette {
        Palette::default()
    }

    /// The palette with `role` set to `colour`.
    #[must_use]
    pub fn with(mut self, role: Role, colour: Colour) -> Palette {
        self.set(role, colour);
        self
    }

    /// Sets `role` to `colour`.
    pub fn set(&mut self, role: Role, colour: Colour) {
        if let Some(slot) = self.colours.get_mut(role.index()) {
            *slot = Some(colour);
        }
    }

    /// The colour this palette sets for `role`, without the defaults.
    pub fn get(&self, role: Role) -> Option<Colour> {
        self.colours.get(role.index()).copied().flatten()
    }

    /// The colour of `role` before the colour mode reduces it: the palette's own, else
    /// the default. `None` means the terminal's own foreground. A heading without a
    /// colour of its own has the accent's colour.
    pub fn colour(&self, role: Role) -> Option<Colour> {
        match (self.get(role), role) {
            (Some(colour), _) => Some(colour),
            (None, Role::Heading) => self.colour(Role::Accent),
            (None, role) => role.default_look().colour,
        }
    }

    /// The style of `role` in colour mode `mode`: its colour reduced to the mode, its
    /// attributes, and the attributes it has in place of a colour when there is none.
    pub(crate) fn style(&self, role: Role, mode: ColourMode) -> Style {
        let look = role.default_look();
        let colour = reduce_colour(self.colour(role), mode);
        let style = Style { fg: colour, ..look.always };
        if colour.is_none() { style.patch(look.without_colour) } else { style }
    }

    /// The colour of `role` in colour mode `mode` without the attributes it always has;
    /// without a colour, the attributes it has in place of one.
    pub(crate) fn tint(&self, role: Role, mode: ColourMode) -> Style {
        match reduce_colour(self.colour(role), mode) {
            Some(colour) => Style { fg: Some(colour), ..Style::PLAIN },
            None => Style::PLAIN.patch(role.default_look().without_colour),
        }
    }
}

#[cfg(test)]
mod tests;

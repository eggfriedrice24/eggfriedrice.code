//! What the CLI knows about the terminal: the variables that decide colour and
//! formatting, which standard streams are terminals, and the window size.
//!
//! `efr-render` reads nothing itself; this module turns these facts into its
//! `RenderOptions`.

use std::fmt;
use std::io::{self, IsTerminal as _};
use std::os::fd::AsFd as _;

use efr_render::{ColourMode, RenderOptions, Theme};

/// The terminal facts that come from the environment and the standard streams.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TermFacts {
    /// `NO_COLOR` is set to a non-empty value (no-color.org).
    pub(crate) no_color: bool,
    /// `TERM`, when set and not empty.
    pub(crate) term: Option<String>,
    /// `COLORTERM`, when set and not empty.
    pub(crate) colorterm: Option<String>,
    /// Stdout is a terminal.
    pub(crate) stdout_tty: bool,
    /// Stderr is a terminal.
    pub(crate) stderr_tty: bool,
    /// Stdin is a terminal, so one-key answers can be read from it.
    pub(crate) stdin_tty: bool,
    /// `TERM_PROGRAM`, when set and not empty, such as `ghostty`.
    pub(crate) term_program: Option<String>,
    /// `TERM_PROGRAM_VERSION`, when set and not empty, such as `1.3.1`.
    pub(crate) term_program_version: Option<String>,
    /// `TMUX` is set: the output goes through tmux.
    pub(crate) tmux: bool,
    /// `WT_SESSION` is set: the terminal is Windows Terminal.
    pub(crate) wt_session: bool,
}

impl TermFacts {
    /// Reads the facts from this process.
    pub(crate) fn from_process() -> TermFacts {
        // NOTE: efr_stdx::env::Var names only the EFR_* variables, and these are
        // terminal conventions that efr does not own; the CLI is the composition root
        // that decides the output format, so it reads them here, once.
        let text = |name: &str| {
            std::env::var_os(name)
                .and_then(|value| value.into_string().ok())
                .filter(|value| !value.is_empty())
        };
        TermFacts {
            no_color: std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()),
            term: text("TERM"),
            colorterm: text("COLORTERM"),
            stdout_tty: io::stdout().is_terminal(),
            stderr_tty: io::stderr().is_terminal(),
            stdin_tty: io::stdin().is_terminal(),
            term_program: text("TERM_PROGRAM"),
            term_program_version: text("TERM_PROGRAM_VERSION"),
            tmux: std::env::var_os("TMUX").is_some(),
            wt_session: std::env::var_os("WT_SESSION").is_some(),
        }
    }

    /// The colours the output may use: none under `NO_COLOR`, 24-bit when `COLORTERM`
    /// says `truecolor` or `24bit`, otherwise the 16-colour palette.
    pub(crate) fn colour(&self) -> ColourMode {
        if self.no_color {
            return ColourMode::None;
        }
        match self.colorterm.as_deref() {
            Some(value)
                if value.eq_ignore_ascii_case("truecolor")
                    || value.eq_ignore_ascii_case("24bit") =>
            {
                ColourMode::TrueColor
            }
            _ => ColourMode::Ansi16,
        }
    }

    /// True when replies are rendered: stdout is a terminal and `TERM` is not `dumb`.
    /// Otherwise the raw markdown is written.
    pub(crate) fn formats_stdout(&self) -> bool {
        self.stdout_tty && self.term.as_deref() != Some("dumb")
    }

    /// The options for rendering on a terminal `width` columns wide with `theme`.
    pub(crate) fn render_options(&self, width: u16, theme: Theme) -> RenderOptions {
        RenderOptions::new(width)
            .with_colour(self.colour())
            .with_theme(theme)
            .with_terminal(self.formats_stdout())
    }
}

/// `options` for a terminal `width` columns wide; everything else stays.
pub(crate) fn at_width(options: &RenderOptions, width: u16) -> RenderOptions {
    RenderOptions::new(width)
        .with_colour(options.colour())
        .with_theme(options.theme())
        .with_hyperlinks(options.hyperlinks())
        .with_terminal(options.is_terminal())
}

/// A terminal size in character cells. Zero means unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Size {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
}

/// Where the window size comes from: the terminal on stdout, or a fixed size in tests.
pub(crate) trait Screen: Send + Sync + fmt::Debug {
    /// The current size; asked again before every redraw, so a resize is noticed.
    fn size(&self) -> Size;
}

/// The window size of the terminal on stdout, through `TIOCGWINSZ`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct StdoutScreen;

impl Screen for StdoutScreen {
    fn size(&self) -> Size {
        rustix::termios::tcgetwinsize(io::stdout())
            .map(|size| Size { cols: size.ws_col, rows: size.ws_row })
            .unwrap_or_default()
    }
}

/// The name of the terminal on stdin, such as `/dev/pts/3`, for a prompt typed by
/// hand without the plugin's context.
pub(crate) fn stdin_tty_name() -> Option<String> {
    let stdin = io::stdin();
    let name = rustix::termios::ttyname(stdin.as_fd(), Vec::new()).ok()?;
    name.into_string().ok()
}

#[cfg(test)]
mod tests;

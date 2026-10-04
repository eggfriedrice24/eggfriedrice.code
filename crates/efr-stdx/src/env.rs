//! Typed access to the environment variables that efr reads.
//!
//! This is the one module that reads the process environment. Other crates call
//! [`var`], [`path`] or [`flag`] with a [`Var`], so the list of variables lives in one
//! enum. The crate README holds the same list, and a test fails when the two disagree.
//!
//! An empty value counts as unset, as the XDG specification does for its own
//! variables, so `EFR_DATA_DIR= efrd` behaves like plain `efrd`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

use crate::StdxError;

/// An environment variable that efr reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Var {
    /// `EFR_LOG`: the tracing filter for `efrd` and `efr`, in `EnvFilter` syntax.
    Log,
    /// `EFR_SCREEN`: the screen backend, `vt100` or `ghostty`.
    Screen,
    /// `EFR_CONFIG_DIR`: an absolute path that replaces `$XDG_CONFIG_HOME/efr`.
    ConfigDir,
    /// `EFR_DATA_DIR`: an absolute path that replaces `$XDG_DATA_HOME/efr`.
    DataDir,
    /// `EFR_STATE_DIR`: an absolute path that replaces `$XDG_STATE_HOME/efr`.
    StateDir,
    /// `EFR_RUNTIME_DIR`: an absolute path that replaces `$XDG_RUNTIME_DIR/efr`.
    RuntimeDir,
    /// `EFR_OPEN_BROWSER`: a flag; when it is on, `efr login openai` opens the login
    /// URL in a browser.
    OpenBrowser,
    /// `EFR_RECORD_TRANSCRIPT`: an absolute path; `efrd` writes an NDJSON transcript
    /// of provider traffic and PTY bytes to it.
    RecordTranscript,
    /// `EFR_TEST_ZSH`: a flag; tests that drive a real zsh run only when it is on.
    TestZsh,
}

impl Var {
    /// Every variable, in the order of the README table.
    pub const ALL: &'static [Var] = &[
        Var::Log,
        Var::Screen,
        Var::ConfigDir,
        Var::DataDir,
        Var::StateDir,
        Var::RuntimeDir,
        Var::OpenBrowser,
        Var::RecordTranscript,
        Var::TestZsh,
    ];

    /// The name of the variable in the environment.
    pub const fn name(self) -> &'static str {
        match self {
            Var::Log => "EFR_LOG",
            Var::Screen => "EFR_SCREEN",
            Var::ConfigDir => "EFR_CONFIG_DIR",
            Var::DataDir => "EFR_DATA_DIR",
            Var::StateDir => "EFR_STATE_DIR",
            Var::RuntimeDir => "EFR_RUNTIME_DIR",
            Var::OpenBrowser => "EFR_OPEN_BROWSER",
            Var::RecordTranscript => "EFR_RECORD_TRANSCRIPT",
            Var::TestZsh => "EFR_TEST_ZSH",
        }
    }
}

impl fmt::Display for Var {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The values a flag variable accepts, for the error message.
const FLAG_VALUES: &str = "a flag (1, true, yes, on, 0, false, no or off)";

/// Where variables are read from.
///
/// Production code reads the process environment through the free functions of this
/// module. Tests build a fixed environment instead: edition 2024 does not allow
/// `std::env::set_var` in safe code, because it races with other test threads.
///
/// ```
/// use efr_stdx::env::{Env, Var};
///
/// let env = Env::fixed([(Var::TestZsh, "1")]);
/// assert!(env.flag(Var::TestZsh)?);
/// assert_eq!(env.var(Var::Log)?, None);
/// # Ok::<(), efr_stdx::StdxError>(())
/// ```
#[derive(Debug, Clone)]
pub struct Env {
    source: Source,
}

#[derive(Debug, Clone)]
enum Source {
    Process,
    Fixed(BTreeMap<Var, OsString>),
}

impl Env {
    /// The environment of this process, read at each call.
    pub fn process() -> Self {
        Env { source: Source::Process }
    }

    /// An environment that holds only the given variables.
    pub fn fixed<I, V>(vars: I) -> Self
    where
        I: IntoIterator<Item = (Var, V)>,
        V: Into<OsString>,
    {
        let vars = vars.into_iter().map(|(var, value)| (var, value.into())).collect();
        Env { source: Source::Fixed(vars) }
    }

    /// The value of `var` as text, or `None` when it is unset or empty.
    pub fn var(&self, var: Var) -> Result<Option<String>, StdxError> {
        self.raw(var)
            .map(|value| value.into_string().map_err(|_| StdxError::NotUnicode { var }))
            .transpose()
    }

    /// The value of `var` as an absolute path, or `None` when it is unset or empty.
    ///
    /// The value may hold any bytes; only a relative path is an error.
    pub fn path(&self, var: Var) -> Result<Option<PathBuf>, StdxError> {
        let Some(value) = self.raw(var) else {
            return Ok(None);
        };
        let path = PathBuf::from(value);
        if path.is_absolute() {
            Ok(Some(path))
        } else {
            Err(StdxError::RelativeEnvPath { var, path })
        }
    }

    /// The value of `var` as a flag. Unset and empty mean off.
    pub fn flag(&self, var: Var) -> Result<bool, StdxError> {
        let Some(value) = self.var(var)? else {
            return Ok(false);
        };
        parse_flag(&value).ok_or(StdxError::InvalidEnvValue { var, value, expected: FLAG_VALUES })
    }

    fn raw(&self, var: Var) -> Option<OsString> {
        let value = match &self.source {
            Source::Process => std::env::var_os(var.name()),
            Source::Fixed(vars) => vars.get(&var).cloned(),
        };
        value.filter(|value| !value.is_empty())
    }
}

/// The value of `var` in the process environment as text, or `None` when it is unset
/// or empty. This is the replacement for `std::env::var` that `clippy.toml` names.
pub fn var(var: Var) -> Result<Option<String>, StdxError> {
    Env::process().var(var)
}

/// The value of `var` in the process environment as an absolute path, or `None` when
/// it is unset or empty.
pub fn path(var: Var) -> Result<Option<PathBuf>, StdxError> {
    Env::process().path(var)
}

/// The value of `var` in the process environment as a flag. Unset and empty mean off.
pub fn flag(var: Var) -> Result<bool, StdxError> {
    Env::process().flag(var)
}

fn parse_flag(value: &str) -> Option<bool> {
    const ON: &[&str] = &["1", "true", "yes", "on"];
    const OFF: &[&str] = &["0", "false", "no", "off"];
    if ON.iter().any(|on| value.eq_ignore_ascii_case(on)) {
        Some(true)
    } else if OFF.iter().any(|off| value.eq_ignore_ascii_case(off)) {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests;

//! What to run on a new PTY.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use efr_protocol::{PtyId, Size};
use serde::{Deserialize, Serialize};

use crate::HolderError;

/// Everything a holder needs to start one child process on a new PTY.
///
/// The child gets exactly [`env`](Self::env) as its environment; nothing is inherited
/// from the holder's own process. The daemon and `efr-ptyd` run with different
/// environments, and the daemon's carries the variables systemd sets for its unit, so a
/// holder that inherited would start a different shell depending on where it runs.
/// For the same reason [`program`](Self::program) is an absolute path and no holder
/// searches `PATH`.
///
/// The caller mints [`pty_id`](Self::pty_id). A spawn that is sent twice over the
/// holder socket, as a retry after a lost reply, then fails with
/// [`HolderError::AlreadyExists`] instead of starting a second shell.
///
/// The struct is `#[non_exhaustive]` so that later fields (the Landlock rules of
/// `efr-sandbox`, for one) do not break callers: build it with [`SpawnSpec::new`] and
/// the chained setters. Holders call [`SpawnSpec::validate`] before they open anything,
/// because a spec that arrives over the holder socket bypasses the constructor.
///
/// On the wire, `args` and `env` are left out when empty, and `program` and `cwd` are
/// strings, so a path that is not UTF-8 cannot cross the holder socket.
///
/// `Debug` lists the environment's names but not its values: the hidden shell's
/// environment is copied from the user's, which can hold tokens.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SpawnSpec {
    /// The id of the new PTY.
    pub pty_id: PtyId,
    /// The absolute path of the program to run, such as `/usr/bin/zsh`.
    pub program: PathBuf,
    /// The arguments after the program name. The program's own path is `argv[0]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// The absolute working directory of the child.
    pub cwd: PathBuf,
    /// The child's whole environment.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// The terminal size the PTY starts with.
    pub size: Size,
}

impl SpawnSpec {
    /// A spec that runs `program` in `cwd` on a PTY of `size`, with no arguments and an
    /// empty environment.
    pub fn new(
        pty_id: PtyId,
        program: impl Into<PathBuf>,
        cwd: impl Into<PathBuf>,
        size: Size,
    ) -> Self {
        SpawnSpec {
            pty_id,
            program: program.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: BTreeMap::new(),
            size,
        }
    }

    /// Appends one argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Appends several arguments.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Sets one environment variable, replacing an earlier value.
    #[must_use]
    pub fn var(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(name.into(), value.into());
        self
    }

    /// Sets several environment variables, replacing earlier values.
    #[must_use]
    pub fn vars<I, K, V>(mut self, vars: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.env.extend(vars.into_iter().map(|(name, value)| (name.into(), value.into())));
        self
    }

    /// Checks that a holder can start this spec as written: an absolute program and
    /// working directory, a size with at least one cell, environment names that are not
    /// empty and hold no `=`, and no NUL byte anywhere. It reports the first problem it
    /// finds, in field order.
    pub fn validate(&self) -> Result<(), HolderError> {
        if has_nul(self.program.as_os_str().as_encoded_bytes()) {
            return Err(HolderError::NulByte { field: "program" });
        }
        if !self.program.is_absolute() {
            return Err(HolderError::ProgramNotAbsolute { program: self.program.clone() });
        }
        if self.args.iter().any(|arg| has_nul(arg.as_bytes())) {
            return Err(HolderError::NulByte { field: "argument" });
        }
        validate_cwd(&self.cwd)?;
        for (name, value) in &self.env {
            if name.is_empty() || name.contains('=') || has_nul(name.as_bytes()) {
                return Err(HolderError::InvalidEnvName { name: name.clone() });
            }
            if has_nul(value.as_bytes()) {
                return Err(HolderError::NulInEnvValue { name: name.clone() });
            }
        }
        if self.size.cols == 0 || self.size.rows == 0 {
            return Err(HolderError::EmptySize { size: self.size });
        }
        Ok(())
    }
}

fn validate_cwd(cwd: &Path) -> Result<(), HolderError> {
    if has_nul(cwd.as_os_str().as_encoded_bytes()) {
        return Err(HolderError::NulByte { field: "cwd" });
    }
    if !cwd.is_absolute() {
        return Err(HolderError::CwdNotAbsolute { cwd: cwd.to_path_buf() });
    }
    Ok(())
}

fn has_nul(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

impl fmt::Debug for SpawnSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpawnSpec")
            .field("pty_id", &self.pty_id)
            .field("program", &self.program)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("env", &EnvNames(&self.env))
            .field("size", &self.size)
            .finish()
    }
}

/// Shows an environment as the list of its names.
struct EnvNames<'a>(&'a BTreeMap<String, String>);

impl fmt::Debug for EnvNames<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}

#[cfg(test)]
mod tests;

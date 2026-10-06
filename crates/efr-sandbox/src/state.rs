//! [`SandboxState`]: what later contained calls of a conversation inherit from earlier
//! ones (the spec's section 6.1), and `state.zsh`, its form for the child shell.
//!
//! Only the launcher writes the state, from filtered records. Functions, aliases and
//! exports made in the sandbox stay here and are never promoted; the exit child never
//! reads the state.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::SandboxError;
use crate::export_filter::overlay_denied;
use crate::names::is_variable_name;
use crate::records::Records;

/// The most bytes a `state.json` may have.
pub const MAX_STATE_BYTES: usize = 4 * 1024 * 1024;

/// Where the last contained call ended, when that was in the private tmp, which the
/// host does not have.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SandboxCwd {
    /// The directory, below `/tmp` or `/var/tmp` inside the sandbox.
    pub path: PathBuf,
    /// The trusted shell's directory at that time. The next call starts in `path` only
    /// when the shell is still there, so a `cd` that the user typed wins.
    pub shell_pwd: PathBuf,
}

/// The shell state that later contained calls of one conversation start from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct SandboxState {
    /// Exported variables, with their latest values; `PATH` too.
    pub exports: BTreeMap<String, String>,
    /// Exported variables that a call removed.
    pub unsets: BTreeSet<String>,
    /// Functions, with their bodies as `$functions` holds them.
    pub functions: BTreeMap<String, String>,
    /// Functions that a call removed, also ones of the user's snapshot.
    pub removed_functions: BTreeSet<String>,
    /// Aliases, with their values.
    pub aliases: BTreeMap<String, String>,
    /// Aliases that a call removed.
    pub removed_aliases: BTreeSet<String>,
    /// Where the last call ended in the private tmp.
    pub sandbox_cwd: Option<SandboxCwd>,
}

impl SandboxState {
    /// Lays one call's records over the state. A name of
    /// [`OVERLAY_DENY`](crate::OVERLAY_DENY) or a name that is not a shell name never
    /// enters it; names of efr's own functions (`_efr_*`) neither.
    pub fn apply(&mut self, records: &Records) {
        for (name, value) in &records.exports {
            if is_variable_name(name) && !overlay_denied(name) {
                self.unsets.remove(name);
                self.exports.insert(name.clone(), value.clone());
            }
        }
        for name in &records.unsets {
            if is_variable_name(name) && !overlay_denied(name) {
                self.exports.remove(name);
                self.unsets.insert(name.clone());
            }
        }
        for (name, body) in &records.functions {
            if shell_word(name) {
                self.removed_functions.remove(name);
                self.functions.insert(name.clone(), body.clone());
            }
        }
        for name in &records.removed_functions {
            if shell_word(name) {
                self.functions.remove(name);
                self.removed_functions.insert(name.clone());
            }
        }
        for (name, value) in &records.aliases {
            if shell_word(name) {
                self.removed_aliases.remove(name);
                self.aliases.insert(name.clone(), value.clone());
            }
        }
        for name in &records.removed_aliases {
            if shell_word(name) {
                self.aliases.remove(name);
                self.removed_aliases.insert(name.clone());
            }
        }
    }

    /// `state.zsh`: plain assignments, every word quoted, nothing evaluated twice.
    pub fn render(&self) -> String {
        let mut out = String::from("# efr sandbox state; the launcher writes it.\n");
        for name in &self.removed_functions {
            let key = quote(name);
            out.push_str(&format!("(( ${{+functions[{key}]}} )) && builtin unfunction -- {key}\n"));
        }
        for name in &self.removed_aliases {
            let key = quote(name);
            out.push_str(&format!("(( ${{+aliases[{key}]}} )) && builtin unalias -- {key}\n"));
        }
        for (name, body) in &self.functions {
            out.push_str(&format!("functions[{}]={}\n", quote(name), quote(body)));
        }
        for (name, value) in &self.aliases {
            out.push_str(&format!("aliases[{}]={}\n", quote(name), quote(value)));
        }
        for name in &self.unsets {
            out.push_str(&format!("builtin unset -- {name}\n"));
        }
        for (name, value) in &self.exports {
            out.push_str(&format!("builtin typeset -gx -- {name}={}\n", quote(value)));
        }
        out
    }

    /// The state as the bytes of `state.json`, which the launcher keeps next to
    /// `state.zsh`.
    pub fn to_json(&self) -> Result<Vec<u8>, SandboxError> {
        serde_json::to_vec(self).map_err(|source| SandboxError::Json { what: "state", source })
    }

    /// Reads `state.json`.
    pub fn from_json(bytes: &[u8]) -> Result<SandboxState, SandboxError> {
        if bytes.len() > MAX_STATE_BYTES {
            return Err(SandboxError::TooLarge {
                what: "state",
                len: bytes.len(),
                max: MAX_STATE_BYTES,
            });
        }
        serde_json::from_slice(bytes).map_err(|source| SandboxError::Json { what: "state", source })
    }
}

/// True for a function or alias name that the state keeps: no NUL, no newline, not
/// empty, and not one of efr's own.
fn shell_word(name: &str) -> bool {
    !name.is_empty() && !name.starts_with("_efr_") && !name.contains(['\0', '\n'])
}

/// `text` as one single-quoted zsh word.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests;

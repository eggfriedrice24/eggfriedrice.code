//! [`SandboxState`]: what later contained calls of a conversation inherit from earlier
//! ones (the spec's section 6.1), and `state.zsh`, its form for the child shell.
//!
//! Only the launcher writes the state, from filtered records. Functions, aliases and
//! exports made in the sandbox stay here and are never promoted; the exit child never
//! reads the state.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::SandboxError;
use crate::export_filter::overlay_denied;
use crate::names::is_variable_name;
use crate::records::Records;

/// The variable through which `state.zsh` names a function or an alias; it is unset at
/// the end of the file.
pub const STATE_NAME_VAR: &str = "_efr_state_name";

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
///
/// `Debug` shows names only: an exported value can be a token that a call set.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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

impl fmt::Debug for SandboxState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SandboxState")
            .field("exports", &self.exports.keys().collect::<Vec<_>>())
            .field("unsets", &self.unsets)
            .field("functions", &self.functions.keys().collect::<Vec<_>>())
            .field("removed_functions", &self.removed_functions)
            .field("aliases", &self.aliases.keys().collect::<Vec<_>>())
            .field("removed_aliases", &self.removed_aliases)
            .field("sandbox_cwd", &self.sandbox_cwd)
            .finish()
    }
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
    ///
    /// zsh keeps quotes inside a subscript as part of the key (`functions['f']=...`
    /// names a function `'f'`), so each function or alias name goes through the
    /// variable [`STATE_NAME_VAR`] and the subscript expands it.
    pub fn render(&self) -> String {
        let mut out = String::from("# efr sandbox state; the launcher writes it.\n");
        out.push_str("builtin zmodload zsh/parameter\n");
        let var = STATE_NAME_VAR;
        let name_line = |out: &mut String, name: &str| {
            out.push_str(&format!("builtin typeset -g {var}={}\n", quote(name)));
        };
        for name in &self.removed_functions {
            name_line(&mut out, name);
            out.push_str(&format!(
                "(( ${{+functions[${var}]}} )) && builtin unfunction -- \"${var}\"\n"
            ));
        }
        for name in &self.removed_aliases {
            name_line(&mut out, name);
            out.push_str(&format!(
                "(( ${{+aliases[${var}]}} )) && builtin unalias -- \"${var}\"\n"
            ));
        }
        for (name, body) in &self.functions {
            name_line(&mut out, name);
            out.push_str(&format!("functions[${var}]={}\n", quote(body)));
        }
        for (name, value) in &self.aliases {
            name_line(&mut out, name);
            out.push_str(&format!("aliases[${var}]={}\n", quote(value)));
        }
        out.push_str(&format!("builtin unset {var}\n"));
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

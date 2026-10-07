//! The files of one call dir and [`SandboxResult`], the launcher's `result.json`.
//!
//! The launcher writes `result.json` last, by an atomic rename, after bwrap exited. So
//! when the file exists, no sandboxed process of the call lives any more; efr-shell
//! ends the run on it together with the nonce mark (the spec's section 3.15).

use std::path::PathBuf;

use efr_protocol::SandboxSummary;
use serde::{Deserialize, Serialize};

use crate::SandboxError;

/// `$CALL/spec.json`: the [`SandboxSpec`](crate::SandboxSpec), written by efrd.
pub const SPEC_FILE: &str = "spec.json";
/// `$CALL/line`: the model's line, written by efr-shell, never typed.
pub const LINE_FILE: &str = "line";
/// `$CALL/nonce`: 128 random bits in hex, written by efrd; the end mark carries it.
pub const NONCE_FILE: &str = "nonce";
/// `$CALL/started`: written by the launcher once every bind source is open and the git
/// surface is recorded; efrd then releases the project's plan lock.
pub const STARTED_FILE: &str = "started";
/// `$CALL/apply`: the filtered `cd` and exports for the trusted shell.
pub const APPLY_FILE: &str = "apply";
/// `$CALL/result.json`: the [`SandboxResult`], written last.
pub const RESULT_FILE: &str = "result.json";
/// `$CALL/times`: how long the trusted shell's wrapper took for the snapshot, the
/// launcher and the apply file, in seconds, as `snapshot 0.001 launcher 0.012 apply
/// 0.0001`; only for efrd's debug log.
pub const TIMES_FILE: &str = "times";
/// `$R/sbx/<conversation>/state.json`: the [`SandboxState`](crate::SandboxState).
pub const STATE_JSON_FILE: &str = "state.json";
/// `$R/sbx/<conversation>/state.zsh`: the state for the child shell.
pub const STATE_ZSH_FILE: &str = "state.zsh";
/// `$R/sbx/<conversation>/snapshot.zsh`: the trusted shell's functions, aliases and
/// options, written by the wrapper.
pub const SNAPSHOT_FILE: &str = "snapshot.zsh";

/// The exit status of a call whose sandbox did not start, as bwrap uses it.
pub const SETUP_FAILURE_STATUS: i32 = 125;

/// The most bytes a `result.json` may have.
pub const MAX_RESULT_BYTES: usize = 1024 * 1024;

/// What the launcher reports about one call, for efrd. Names only, never values.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxResult {
    /// True when the launcher got past its checks and started bwrap or the exit child.
    pub started: bool,
    /// The child's exit code.
    pub exit_code: Option<i32>,
    /// The signal that ended the child.
    pub signal: Option<i32>,
    /// Why the sandbox did not start: bwrap's setup message, or the inner stage's.
    pub setup_error: Option<String>,
    /// Why the launcher failed after it started bwrap or the exit child, such as a
    /// failed wait: the command may have run.
    pub launch_error: Option<String>,
    /// The state that changed, by name.
    pub summary: SandboxSummary,
    /// The conversation's directory after the call: where the trusted shell is now,
    /// or the sandbox's own directory in the private tmp.
    pub cwd: Option<PathBuf>,
    /// The shell's directory that the sandbox hides, when the call started in scratch.
    pub hidden_cwd: Option<PathBuf>,
    /// The variable names that the environment filter removed.
    pub env_removed: Vec<String>,
    /// False when the records were dropped: "the shell state of this call was not
    /// kept".
    pub state_kept: bool,
    /// How long each step of the launcher took, in order, for efrd's debug log.
    pub timings: Vec<LaunchTiming>,
}

/// How long one step of the launcher took, such as `plan` or `bwrap_setup`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchTiming {
    /// The step.
    pub phase: String,
    /// Its time in microseconds.
    pub us: u64,
}

impl SandboxResult {
    /// The status that the launcher exits with and the shell reports: 125 for a setup
    /// failure, 128 plus the signal, else the exit code.
    pub fn status(&self) -> i32 {
        if self.setup_error.is_some() {
            return SETUP_FAILURE_STATUS;
        }
        match (self.signal, self.exit_code) {
            (Some(signal), _) => 128 + signal,
            (None, Some(code)) => code,
            (None, None) => SETUP_FAILURE_STATUS,
        }
    }

    /// The result as the bytes of `result.json`.
    pub fn to_json(&self) -> Result<Vec<u8>, SandboxError> {
        serde_json::to_vec(self).map_err(|source| SandboxError::Json { what: "result", source })
    }

    /// Reads `result.json`.
    pub fn from_json(bytes: &[u8]) -> Result<SandboxResult, SandboxError> {
        if bytes.len() > MAX_RESULT_BYTES {
            return Err(SandboxError::TooLarge {
                what: "result",
                len: bytes.len(),
                max: MAX_RESULT_BYTES,
            });
        }
        serde_json::from_slice(bytes)
            .map_err(|source| SandboxError::Json { what: "result", source })
    }
}

/// The nonce in lowercase hex, as `$CALL/nonce` and the end mark hold it.
pub fn nonce_hex(nonce: &[u8; 16]) -> String {
    nonce.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The nonce of a hex text of exactly 32 lowercase hex digits.
pub fn parse_nonce_hex(text: &str) -> Option<[u8; 16]> {
    let bytes = text.as_bytes();
    if bytes.len() != 32 || !bytes.iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b)) {
        return None;
    }
    let mut nonce = [0_u8; 16];
    for (at, slot) in nonce.iter_mut().enumerate() {
        let pair = text.get(at * 2..at * 2 + 2)?;
        *slot = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(nonce)
}

#[cfg(test)]
mod tests;

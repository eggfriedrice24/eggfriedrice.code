//! The shell's side of a sandboxed call of the auto mode: the call as the daemon
//! hands it over, the `line` file, and the facts that end the run.
//!
//! efrd prepares the call's dir (`$R/sbx/<conversation>/<call>`, mode 0700) with
//! `spec.json` and `nonce`. efr-shell writes the model's line to `line` there and types
//! only the fixed wrapper line into the shell (see `run::wrapped_line`). The wrapper
//! runs the launcher, applies its `apply` file and prints the end mark with the nonce.
//! The run ends on facts that sandboxed code cannot make (efr's auto spec, section
//! 3.15): the end mark, `D` after it, the shell in the terminal's foreground (the
//! holder's `foreground`), and the launcher's `started` and `result.json`.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use efr_holder::{PtyHolder, PtyId};
use efr_protocol::{CallId, ConversationId};
use efr_sandbox::{
    LINE_FILE, MAX_RESULT_BYTES, RESULT_FILE, STARTED_FILE, SandboxResult, SpecLaunch, TIMES_FILE,
};

use crate::ShellError;
use crate::run::Facts;

/// One sandboxed call, as the daemon prepared it.
///
/// `Debug` shows the nonce's length only: it is the one secret that tells the call's
/// own end mark from a mark that sandboxed code prints.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxRun {
    /// The call's dir, `$R/sbx/<conversation>/<call>`: the shell's sandbox dir
    /// ([`ShellConfig::sandbox_dir`](crate::ShellConfig::sandbox_dir)) with the
    /// conversation and the call.
    pub dir: PathBuf,
    /// The call; the wrapper line names it.
    pub call: CallId,
    /// The nonce of the call's end mark, as `$CALL/nonce` holds it in hex.
    pub nonce: [u8; 16],
    /// Contained in the sandbox, or the exit child of an approved exit. Hidden input
    /// reaches only the exit child.
    pub launch: SpecLaunch,
}

impl SandboxRun {
    /// A sandboxed call in `dir`.
    pub fn new(dir: impl Into<PathBuf>, call: CallId, nonce: [u8; 16], launch: SpecLaunch) -> Self {
        SandboxRun { dir: dir.into(), call, nonce, launch }
    }

    /// True when the command runs in the sandbox, false for the exit child.
    pub fn contained(&self) -> bool {
        self.launch == SpecLaunch::Contained
    }
}

impl fmt::Debug for SandboxRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SandboxRun")
            .field("dir", &self.dir)
            .field("call", &self.call)
            .field("nonce", &format_args!("[{} bytes]", self.nonce.len()))
            .field("launch", &self.launch)
            .finish()
    }
}

/// The conversation's sandbox dir below the shell's root: `$R/sbx/<conversation>`,
/// which the wrapper reads from `_EFR_HS_SBX_DIR`.
pub(crate) fn conversation_dir(root: &Path, conversation: ConversationId) -> PathBuf {
    root.join(conversation.to_string())
}

/// Checks that `run` is a call of `conversation` below `root`, where the wrapper looks
/// for it, and that `command` can run as a line.
pub(crate) fn check(
    root: Option<&Path>,
    conversation: ConversationId,
    run: &SandboxRun,
    command: &str,
) -> Result<(), ShellError> {
    let invalid = |reason| ShellError::Sandbox { conversation, reason };
    let root = root.ok_or(invalid("this shell has no sandbox dir"))?;
    if run.dir != conversation_dir(root, conversation).join(run.call.to_string()) {
        return Err(invalid("the call's dir is not where the wrapper looks for it"));
    }
    if command.contains('\0') {
        return Err(ShellError::InvalidCommand { reason: "it contains a NUL byte" });
    }
    if command.trim().is_empty() {
        return Err(ShellError::InvalidCommand { reason: "it is empty" });
    }
    Ok(())
}

/// Writes the model's line to `$CALL/line` (mode 0600). The file must not exist yet,
/// so nothing that was there before is ever run.
pub(crate) async fn write_line(dir: &Path, command: &str) -> Result<(), ShellError> {
    let path = dir.join(LINE_FILE);
    let bytes = command.as_bytes().to_vec();
    let target = path.clone();
    let written = tokio::task::spawn_blocking(move || {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        // create_new is O_EXCL, which refuses anything at the path, a link included.
        let mut file =
            std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&target)?;
        file.write_all(&bytes)?;
        file.sync_all()
    })
    .await
    .map_err(io::Error::other)
    .and_then(|written| written);
    written.map_err(|source| ShellError::SandboxFile { path, source })
}

/// Reads the facts that decide whether a sandboxed run ends (efr's auto spec, section
/// 3.15). A fact that cannot be read counts as false: the run then goes on, or fails
/// closed.
pub(crate) async fn facts(holder: &dyn PtyHolder, pty_id: PtyId, shell: u32, dir: &Path) -> Facts {
    let foreground = holder.foreground(pty_id).await;
    let shell_holds_terminal = matches!(foreground, Ok(Some(group)) if group == shell);
    if let Err(error) = &foreground {
        tracing::warn!(%pty_id, %error, "could not read who holds the terminal of a sandboxed call");
    }
    let started =
        tokio::fs::symlink_metadata(dir.join(STARTED_FILE)).await.is_ok_and(|meta| meta.is_file());
    let result = read_result(&dir.join(RESULT_FILE)).await;
    if result.is_some() && tracing::enabled!(tracing::Level::DEBUG) {
        log_times(dir).await;
    }
    Facts { shell_holds_terminal, started, result }
}

/// The most bytes of `$CALL/times` that efr reads.
const MAX_TIMES_BYTES: u64 = 256;

/// Logs the steps of the trusted shell's wrapper from `$CALL/times`: the `phase` lines
/// `wrapper_snapshot`, `wrapper_launcher` and `wrapper_apply` (docs/sandbox.md). Only a
/// log reads them, so a file that does not read is skipped.
async fn log_times(dir: &Path) {
    let path = dir.join(TIMES_FILE);
    let Ok(meta) = tokio::fs::symlink_metadata(&path).await else { return };
    if !meta.is_file() || meta.len() > MAX_TIMES_BYTES {
        return;
    }
    let Ok(text) = tokio::fs::read_to_string(&path).await else { return };
    let call_id = dir.file_name().map(|name| name.to_string_lossy().into_owned());
    for (step, seconds) in wrapper_times(&text) {
        let elapsed_ms = format!("{:.1}", seconds * 1000.0);
        tracing::debug!(?call_id, phase = %format!("wrapper_{step}"), %elapsed_ms, "phase={} elapsed_ms={}", format!("wrapper_{step}"), elapsed_ms);
    }
}

/// The steps of `$CALL/times` with their seconds; a word that is not a known step or a
/// time ends the list.
pub(crate) fn wrapper_times(text: &str) -> Vec<(&'static str, f64)> {
    let mut words = text.split_whitespace();
    let mut steps = Vec::new();
    while let (Some(step), Some(seconds)) = (words.next(), words.next()) {
        let step = match step {
            "snapshot" => "snapshot",
            "launcher" => "launcher",
            "apply" => "apply",
            _ => break,
        };
        match seconds.parse::<f64>() {
            Ok(seconds) if seconds.is_finite() && seconds >= 0.0 => steps.push((step, seconds)),
            _ => break,
        }
    }
    steps
}

/// `result.json`, when it is a plain file of a sane size that parses.
async fn read_result(path: &Path) -> Option<SandboxResult> {
    let meta = tokio::fs::symlink_metadata(path).await.ok()?;
    if !meta.is_file() || meta.len() > MAX_RESULT_BYTES as u64 {
        return None;
    }
    let bytes = tokio::fs::read(path).await.ok()?;
    match SandboxResult::from_json(&bytes) {
        Ok(result) => Some(result),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "a sandboxed call's result.json does not read");
            None
        }
    }
}

#[cfg(test)]
mod tests;

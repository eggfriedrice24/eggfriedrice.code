//! The text that is still in the input row when `efr` ends goes back to the user's
//! shell, so nothing typed is lost.
//!
//! The zsh plugin names a file in its runtime directory in `EFR_DRAFT_FILE`. `efr`
//! writes the text there as UTF-8, mode 0600, without a final newline and without the
//! `, ` that starts a prompt line; the plugin's precmd reads the file, removes it and
//! puts `, <text>` on the command line (`print -z`). The text never travels in an
//! argument, which any local user can read. Without the variable, or when the file
//! cannot be written, the text shows as one muted note instead.

use std::path::Path;

use efr_stdx::StdxError;
use efr_stdx::env::Var;

use crate::context::Context;

/// What starts the note of a text that could not go back to the shell.
const NOT_SENT: &str = "not sent: ";

/// Hands `text` back to the user's shell. `None` when it went to the plugin's file or
/// is blank; otherwise the note that shows it.
pub(crate) async fn hand_back(ctx: &Context, text: &str) -> Option<String> {
    let text = text.trim_end_matches(['\n', '\r']);
    if text.trim().is_empty() {
        return None;
    }
    let note = Some(format!("{NOT_SENT}{}", crate::format::one_line(text)));
    let path = match ctx.env.path(Var::DraftFile) {
        Ok(Some(path)) => path,
        Ok(None) => return note,
        Err(error) => {
            tracing::debug!(%error, "EFR_DRAFT_FILE cannot be used");
            return note;
        }
    };
    let bytes = text.as_bytes().to_vec();
    match tokio::task::spawn_blocking(move || write(&path, &bytes)).await {
        Ok(Ok(())) => None,
        Ok(Err(error)) => {
            tracing::debug!(%error, "the text of the input row could not be handed back");
            note
        }
        Err(error) => {
            tracing::debug!(%error, "the text of the input row could not be handed back");
            note
        }
    }
}

/// Writes `bytes` to `path` with mode 0600, after it creates the directory that holds
/// it (one level, mode 0700) when it is missing.
fn write(path: &Path, bytes: &[u8]) -> Result<(), StdxError> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
        && !dir.is_dir()
    {
        let _ = efr_stdx::fs::claim_dir(dir)?;
    }
    efr_stdx::fs::write_atomic(path, bytes)
}

#[cfg(test)]
mod tests;

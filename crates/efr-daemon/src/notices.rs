//! Notices for terminals that do not show their conversation right now.
//!
//! When a turn finishes or fails, or an approval waits, and no client in the
//! conversation's terminal follows it or was handed the event before it stopped
//! following, the daemon appends one line to
//! `$XDG_RUNTIME_DIR/efr/notices/<tty>` (`docs/storage.md`). The zsh plugin prints and
//! removes the file at its next prompt. `<tty>` is `$TTY` without `/dev/`, with `/` as
//! `-`, so `/dev/pts/3` is `pts-3`.

use std::fs::{DirBuilder, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{ConversationId, ErrorCode, Event, EventEnvelope};
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use crate::DaemonError;
use crate::state::State;

/// The directory under the runtime root.
pub(crate) const NOTICES_DIR: &str = "notices";

/// The longest notice, in characters; a terminal line is not the place for more.
const MAX_CHARS: usize = 200;

const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

/// The notice file name for `tty`, or `None` for a name that is empty, `.`, `..` or
/// holds anything but letters, digits, `.`, `_` and `-` once `/` became `-`.
pub(crate) fn file_name(tty: &str) -> Option<String> {
    let name = tty.strip_prefix("/dev/").unwrap_or(tty).replace('/', "-");
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    (!name.is_empty() && name != "." && name != ".." && name.chars().all(allowed)).then_some(name)
}

/// The next step after a turn failed for want of usable credentials. The provider's
/// message says what is wrong, not what to do, and a terminal that shows only the
/// notice has no other place to learn it.
const LOGIN_HINT: &str = "; run efr login openai";

/// The notice for `event` of a conversation titled `title`, if the event deserves one.
pub(crate) fn line(event: &Event, title: Option<&str>) -> Option<String> {
    let title = title.filter(|title| !title.trim().is_empty()).unwrap_or("a conversation");
    let text = match event {
        Event::TurnCompleted { .. } => format!("efr: turn finished: {title}"),
        Event::TurnFailed { error, .. } if error.code == ErrorCode::Unauthorized => {
            // NOTE: the hint goes on after the cut, so a long title cannot push it out.
            let text = format!("efr: turn failed: {title}: {}", error.message);
            let mut line = one_line(&text, MAX_CHARS - LOGIN_HINT.chars().count());
            line.push_str(LOGIN_HINT);
            return Some(line);
        }
        Event::TurnFailed { error, .. } => {
            format!("efr: turn failed: {title}: {}", error.message)
        }
        Event::ApprovalRequested { summary, .. } => {
            // NOTE: a summary's second line names the parts of a command line that
            // ask; on one line it follows after a `;`.
            format!("efr: approval waiting: {title}: {}", summary.replace('\n', "; "))
        }
        _ => return None,
    };
    Some(one_line(&text, MAX_CHARS))
}

/// The notice for the `count` prompts of `conversation` that waited when the daemon
/// stopped and were recorded as not run at the next start.
///
/// NOTE: it names the conversation, never the prompts' text. It goes to the terminal
/// name that the conversation recorded last, and after a restart, a reboot above all,
/// that name may belong to another terminal of the user by now, one that may be
/// shared; a prompt can hold a pasted secret. `efr history` shows the prompts to whoever
/// asks for them.
pub(crate) fn not_run(conversation: ConversationId, count: usize) -> String {
    let text = if count == 1 {
        format!(
            "efr restarted; a queued prompt did not run; see it with efr history {conversation} and send it again"
        )
    } else {
        format!(
            "efr restarted; {count} queued prompts did not run; see them with efr history {conversation} and send them again"
        )
    };
    one_line(&text, MAX_CHARS)
}

/// `text` without control characters, cut to `max` characters with `...` at the cut.
fn one_line(text: &str, max: usize) -> String {
    let clean = text.chars().map(|c| if c.is_control() { ' ' } else { c });
    if text.chars().count() <= max {
        return clean.collect();
    }
    let mut line: String = clean.take(max.saturating_sub(3)).collect();
    line.push_str("...");
    line
}

/// Appends `line` to the notice file of `tty` under `dir`. Blocks; async code calls it
/// on the blocking pool.
pub(crate) fn append(dir: &Path, tty: &str, line: &str) -> Result<Option<PathBuf>, DaemonError> {
    let Some(name) = file_name(tty) else {
        return Ok(None);
    };
    DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(dir)
        .map_err(|source| DaemonError::Io { path: dir.to_path_buf(), source })?;
    let path = dir.join(name);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(FILE_MODE)
        .open(&path)
        .map_err(|source| DaemonError::Io { path: path.clone(), source })?;
    file.write_all(format!("{line}\n").as_bytes())
        .map_err(|source| DaemonError::Io { path: path.clone(), source })?;
    Ok(Some(path))
}

/// Writes a notice for every committed event that deserves one, until `stop`.
pub(crate) async fn follow(state: Arc<State>, stop: CancellationToken) {
    let mut committed = state.writer.subscribe();
    loop {
        let batch = tokio::select! {
            () = stop.cancelled() => return,
            batch = committed.recv() => batch,
        };
        match batch {
            Ok(batch) => {
                for envelope in batch.events() {
                    notify(&state, envelope).await;
                }
            }
            Err(RecvError::Lagged(missed)) => {
                tracing::warn!(missed, "notices fell behind; some were not written");
            }
            Err(RecvError::Closed) => return,
        }
    }
}

async fn notify(state: &State, envelope: &EventEnvelope) {
    let Some(conversation_id) = envelope.conversation_id else {
        return;
    };
    if line(&envelope.event, None).is_none() {
        return;
    }
    let summary =
        state.readers.with(move |conn| efr_store::conversations::get(conn, conversation_id)).await;
    let summary = match summary {
        Ok(Some(summary)) => summary,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(error = %error, %conversation_id, "a notice could not be prepared");
            return;
        }
    };
    let Some(tty) = summary.tty else {
        return;
    };
    if state.connections.attached(&tty, conversation_id, state.clock.now(), envelope.seq) {
        return;
    }
    let Some(text) = line(&envelope.event, summary.title.as_deref()) else {
        return;
    };
    write(state.dirs.runtime().join(NOTICES_DIR), tty, text, conversation_id).await;
}

async fn write(dir: PathBuf, tty: String, text: String, conversation_id: ConversationId) {
    match tokio::task::spawn_blocking(move || append(&dir, &tty, &text)).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            tracing::warn!(error = %error, %conversation_id, "a notice could not be written");
        }
        Err(_) => tracing::warn!("writing a notice panicked"),
    }
}

#[cfg(test)]
mod tests;

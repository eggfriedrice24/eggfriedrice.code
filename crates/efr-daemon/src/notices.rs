//! Notices for terminals that do not show their conversation right now.
//!
//! When a turn finishes or fails, an approval waits, a background job ends, efr forwards
//! a port of a job or says why not (`efr: <title>: job j1 listens on 127.0.0.1:8765 ->
//! http://localhost:8765`, DL15), or a job's notice waits for the next prompt because
//! it could start no turn, and no subscription
//! from the conversation's terminal was handed the event (nor a live lease from it names
//! the conversation), the daemon appends one line to
//! `$XDG_RUNTIME_DIR/efr/notices/<tty>` (`docs/storage.md`). The zsh plugin prints and
//! removes the file at its next prompt. `<tty>` is `$TTY` without `/dev/`, with `/` as
//! `-`, so `/dev/pts/3` is `pts-3`.
//!
//! The end of a turn that job notices started (a wake turn) has no prompt of the user
//! to name, so its line names the jobs and shows the first line of the model's answer,
//! then `; efr history shows all` ([`Wake`], DV29). The file is 0600 below the runtime
//! root, and a wake turn needs a live conversation, so its terminal is the one that
//! the conversation uses now.
//!
//! A command that a call left running in the hidden shell gets a line too
//! ([`shell_line`]): when it stays there (`efr: <title>: the hidden shell still runs
//! rsync -a ...`), when it waits for input that efr keeps for the user, with the row that
//! asks and who approved the call, and when efr stopped it at its limit. The line says
//! to answer only after `,shell`, never at the prompt where the notice shows, so a
//! password never becomes a command line or a prompt to the agent. The events of such a
//! command name only its call, so [`LeftCommands`] keeps the line and the approver of
//! each one from its `shell_command_detached` until its `shell_command_ended`. The
//! command and the row went through the screen of the hidden shell; a line keeps
//! neither a control nor a format character of them.
//!
//! The decision waits while a client in the terminal may still show the event: `efr`
//! sends a prompt and then follows its turn, which may already have ended, and it
//! leaves as soon as it has shown the end, often before this task reads the commit.
//! `connections.rs` holds such a notice and hands it back here once nobody was handed
//! its event.

use std::collections::HashMap;
use std::fs::{DirBuilder, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{
    ApprovedBy, CallId, ConversationId, ErrorCode, Event, EventEnvelope, HeldReason, InputWait,
    JobState, JobStopBy, ShellCommandEnd, TurnId,
};
use efr_stdx::text::is_format;
use jiff::Timestamp;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use crate::DaemonError;
use crate::connections::{Decision, Notice};
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

/// What follows the first line of a job notice turn's answer: the rest is in the
/// history.
const HISTORY_HINT: &str = "; efr history shows all";

/// A turn that job notices started (a wake turn), for its notice: the jobs that it
/// names and the first line of its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Wake {
    /// The ids of the jobs whose notices started the turn, as the turn's prompt names
    /// them: `j3` or `j3, j4`.
    pub(crate) jobs: String,
    /// The first line of the turn's last answer that is not empty; `None` without one.
    pub(crate) answer: Option<String>,
}

/// The [`Wake`] of the job notice turn `turn`, whose prompt column is `prompt` (such as
/// `job notice: j3, j4`) and whose events through its end are `page`.
pub(crate) fn wake_of(turn: TurnId, prompt: &str, page: &[EventEnvelope]) -> Wake {
    let jobs = prompt.strip_prefix(efr_store::conversations::WAKE_PROMPT).unwrap_or(prompt);
    let answer = page.iter().rev().find_map(|envelope| match &envelope.event {
        Event::AssistantMessageCompleted { turn_id, text, .. } if *turn_id == turn => {
            text.lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_owned)
        }
        _ => None,
    });
    Wake { jobs: jobs.trim().to_owned(), answer }
}

/// The notice for `event` of a conversation titled `title`, if the event deserves one.
/// `wake` is the job notice turn that a `turn_completed` or `turn_failed` ends.
pub(crate) fn line(event: &Event, title: Option<&str>, wake: Option<&Wake>) -> Option<String> {
    let title = title.filter(|title| !title.trim().is_empty()).unwrap_or("a conversation");
    let text = match event {
        Event::TurnCompleted { .. } => {
            let Some(wake) = wake else {
                return Some(one_line(&format!("efr: turn finished: {title}"), MAX_CHARS));
            };
            let head =
                format!("efr: job notice turn finished ({}): {title}", job_words(&wake.jobs));
            // NOTE: the hint goes on after the cut, so a long answer cannot push it out.
            return Some(match &wake.answer {
                Some(answer) => with_extra(&format!("{head}: {answer}"), HISTORY_HINT),
                None => one_line(&head, MAX_CHARS),
            });
        }
        Event::TurnFailed { error, .. } => {
            let head = match wake {
                Some(wake) => format!("efr: job notice turn failed ({})", job_words(&wake.jobs)),
                None => "efr: turn failed".to_owned(),
            };
            let text = format!("{head}: {title}: {}", error.message);
            if error.code == ErrorCode::Unauthorized {
                // NOTE: the hint goes on after the cut, so a long title cannot push it out.
                return Some(with_extra(&text, LOGIN_HINT));
            }
            text
        }
        Event::ApprovalRequested { summary, .. } => {
            // NOTE: a summary's second line names the parts of a command line that
            // ask; on one line it follows after a `;`.
            format!("efr: approval waiting: {title}: {}", summary.replace('\n', "; "))
        }
        Event::JobEnded {
            job_id,
            state,
            exit_code,
            stopped_by,
            quarantined,
            survivors,
            launcher_lost,
            ..
        } => {
            let what = job_end(*state, *exit_code, *stopped_by, *quarantined);
            let mut extra = String::new();
            if *launcher_lost {
                extra.push_str("; its launcher was lost, and its processes may still run");
            }
            match survivors.len() {
                0 => {}
                1 => extra.push_str("; 1 process of the job may still run"),
                count => extra.push_str(&format!("; {count} processes of the job may still run")),
            }
            return Some(with_extra(&format!("efr: job {job_id} {what}: {title}"), &extra));
        }
        Event::JobNoticeHeld { job_id, reason, .. } => {
            let extra = format!("; no wake turn ({})", held_reason(*reason));
            let text = format!("efr: job {job_id} waits for your next prompt: {title}");
            return Some(with_extra(&text, &extra));
        }
        Event::JobPortForwarded { job_id, listen, port, outcome, by_job, reason } => {
            // NOTE: the title comes first, so a long one is cut and the URL at the end
            // of the line stays whole.
            let forward = crate::jobs::forward::line(
                job_id,
                listen,
                *port,
                *outcome,
                by_job.as_ref(),
                reason.as_deref(),
            );
            let forward = one_line(&forward, MAX_CHARS);
            return Some(with_extra(&format!("efr: {title}"), &format!(": {forward}")));
        }
        _ => return None,
    };
    Some(one_line(&text, MAX_CHARS))
}

/// The most characters of a command line in a notice, so the rest of the line fits.
const COMMAND_CHARS: usize = 60;

/// The most characters of the row that asks for input in a notice.
const PROMPT_CHARS: usize = 40;

/// The longest notice of a command left running that waits for input, in characters.
/// It is longer than [`MAX_CHARS`]: it must keep the row that asks, who approved the
/// call and where to type, and a cut must never take one of them.
const MAX_WAIT_CHARS: usize = 300;

/// The most characters after the command in such a notice: what it waits for, the row
/// that asks ([`PROMPT_CHARS`]), the approver and where to type.
const MAX_WAIT_EXTRA: usize = 170;

/// The most commands left running that the notices keep in mind. A command leaves at
/// its end, which a lagging subscription to the commits can miss, so the table is
/// emptied once it is full.
const MAX_LEFT: usize = 1024;

/// What the notices know of a command that a call left running in a hidden shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Left {
    /// The first characters of its line.
    pub(crate) command: String,
    /// When it started.
    pub(crate) since: Timestamp,
    /// True when it runs in the sandbox.
    pub(crate) contained: bool,
    /// Who approved its call.
    pub(crate) approved_by: Option<ApprovedBy>,
    /// True while its latest wait is for a secret: a hidden wait, or a visible one whose
    /// row looks like a password prompt.
    pub(crate) secret: bool,
}

/// The commands left running in the hidden shells, by call, from the committed events.
#[derive(Debug, Default)]
pub(crate) struct LeftCommands {
    by_call: HashMap<CallId, Left>,
}

impl LeftCommands {
    /// Takes the news of `event` and answers what is known of its command: at its end,
    /// the last that was known, and the command is forgotten.
    pub(crate) fn see(&mut self, event: &Event) -> Option<Left> {
        match event {
            Event::ShellCommandDetached {
                call_id,
                command,
                since,
                contained,
                input,
                looks_secret,
                approved_by,
                ..
            } => {
                if self.by_call.len() >= MAX_LEFT {
                    self.by_call.clear();
                }
                let left = Left {
                    command: command.clone(),
                    since: *since,
                    contained: *contained,
                    approved_by: *approved_by,
                    secret: secret_wait(*input, *looks_secret),
                };
                self.by_call.insert(*call_id, left.clone());
                Some(left)
            }
            Event::ShellCommandInputChanged { call_id, input, looks_secret, .. } => {
                let left = self.by_call.get_mut(call_id)?;
                left.secret = secret_wait(*input, *looks_secret);
                Some(left.clone())
            }
            Event::ShellCommandEnded { call_id, .. } => self.by_call.remove(call_id),
            _ => None,
        }
    }
}

/// True for a wait for a secret: a hidden one, or a visible one that looks like a
/// password prompt.
fn secret_wait(input: InputWait, looks_secret: bool) -> bool {
    input == InputWait::Hidden || (input == InputWait::Visible && looks_secret)
}

/// The notice for `event` of a command left running in a hidden shell, of a
/// conversation titled `title`, if the event deserves one. `left` is what
/// [`LeftCommands::see`] knew of the command, `at` when the event was recorded and
/// `limit` how long such a command may wait for input
/// (`shell.interactive_timeout_minutes`).
///
/// A wait that efr does not keep for the user gets no line: the hidden shell stops it
/// at once, and its end gets one. That is a wait for a secret in the sandbox, or one
/// of a call that neither the user nor the user's own rule approved.
pub(crate) fn shell_line(
    event: &Event,
    title: Option<&str>,
    left: Option<&Left>,
    at: Timestamp,
    limit: Duration,
) -> Option<String> {
    let title = title.filter(|title| !title.trim().is_empty()).unwrap_or("a conversation");
    match event {
        Event::ShellCommandDetached {
            command,
            contained,
            input,
            looks_secret,
            prompt,
            approved_by,
            ..
        } => {
            let command = one_line(command, COMMAND_CHARS);
            if *input == InputWait::None {
                let text = format!("efr: {title}: the hidden shell still runs {command}");
                return Some(with_extra(&text, "; ,shell shows it and stops it"));
            }
            let wait = Wait { input: *input, looks_secret: *looks_secret, prompt };
            wait_line(title, &command, *contained, &wait, *approved_by)
        }
        Event::ShellCommandInputChanged { input, contained, looks_secret, prompt, .. } => {
            if *input == InputWait::None {
                return None;
            }
            let (command, approved_by) = match left {
                Some(left) => (one_line(&left.command, COMMAND_CHARS), left.approved_by),
                None => ("a command".to_owned(), None),
            };
            let wait = Wait { input: *input, looks_secret: *looks_secret, prompt };
            wait_line(title, &command, *contained, &wait, approved_by)
        }
        Event::ShellCommandEnded { how: ShellCommandEnd::Limit, .. } => {
            let Some(left) = left else {
                let text = format!(
                    "efr: {title}: efr stopped a command in the hidden shell: it waited for \
                     input too long"
                );
                return Some(one_line(&text, MAX_CHARS));
            };
            let command = one_line(&left.command, COMMAND_CHARS);
            let waited = Duration::try_from(at.duration_since(left.since)).unwrap_or_default();
            // NOTE: both stops end as `limit`. A wait for a secret that nobody may answer
            // stops at the look that sees it, long before the limit.
            let why = if left.secret && left.contained {
                "it asked for a password in the sandbox, where efr types none".to_owned()
            } else if left.secret && waited < limit {
                "it asked for a password while nobody followed".to_owned()
            } else {
                match waited.as_secs() / 60 {
                    1 => "it waited for input for 1 minute".to_owned(),
                    minutes => format!("it waited for input for {minutes} minutes"),
                }
            };
            let text = format!("efr: {title}: efr stopped {command} in the hidden shell");
            Some(with_extra(&text, &format!(": {why}")))
        }
        _ => None,
    }
}

/// A wait of a command left running, as its event tells it.
struct Wait<'a> {
    input: InputWait,
    looks_secret: bool,
    /// The row that asks.
    prompt: &'a str,
}

/// The notice for a command left running that waits for input, or `None` when efr
/// does not keep the wait for the user.
fn wait_line(
    title: &str,
    command: &str,
    contained: bool,
    wait: &Wait<'_>,
    approved_by: Option<ApprovedBy>,
) -> Option<String> {
    let secret = secret_wait(wait.input, wait.looks_secret);
    let person = matches!(approved_by, Some(ApprovedBy::You | ApprovedBy::YourRule));
    if secret && (contained || !person) {
        return None;
    }
    let mut asked = Vec::new();
    let prompt = one_line(wait.prompt.trim(), PROMPT_CHARS);
    if !prompt.is_empty() {
        asked.push(format!("asked by \"{prompt}\""));
    }
    asked.extend(approved_by.and_then(approver).map(str::to_owned));
    let asked = if asked.is_empty() { String::new() } else { format!(" ({})", asked.join("; ")) };
    let (what, hint) = if secret {
        ("a password", ". Type the password only after ,shell; never at this prompt.")
    } else {
        ("input", ". Answer it only after ,shell; never at this prompt.")
    };
    let text = format!("efr: {title}: the hidden shell still runs {command}");
    let mut line = one_line(&text, MAX_WAIT_CHARS.saturating_sub(MAX_WAIT_EXTRA));
    line.push_str(&format!(" and waits for {what}{asked}{hint}"));
    Some(line)
}

/// Who let a call run, in a few words; `None` for a way that this build does not know.
fn approver(approved_by: ApprovedBy) -> Option<&'static str> {
    match approved_by {
        ApprovedBy::You => Some("you approved it"),
        ApprovedBy::YourRule => Some("your rule allowed it"),
        ApprovedBy::Reviewer => Some("the reviewer allowed it"),
        ApprovedBy::FullMode => Some("full mode let it run"),
        _ => None,
    }
}

/// `job j3`, or `jobs j3, j4` for the ids `jobs` of a job notice turn.
fn job_words(jobs: &str) -> String {
    if jobs.contains(',') { format!("jobs {jobs}") } else { format!("job {jobs}") }
}

/// `text` cut so that `extra` fits after it on one line of [`MAX_CHARS`], then `extra`:
/// a long title cannot push the end of the line out.
fn with_extra(text: &str, extra: &str) -> String {
    let mut line = one_line(text, MAX_CHARS.saturating_sub(extra.chars().count()));
    line.push_str(extra);
    line
}

/// How a job ended, in a few words: `ended (exit 0)`, `timed out`, `stopped by you`.
fn job_end(
    state: JobState,
    exit_code: Option<i32>,
    stopped_by: Option<JobStopBy>,
    quarantined: bool,
) -> String {
    match (state, stopped_by) {
        (JobState::TimedOut, _) | (_, Some(JobStopBy::TimeLimit)) => "timed out".to_owned(),
        (JobState::Failed, _) => "failed to start or run".to_owned(),
        (_, Some(JobStopBy::User)) => "stopped by you".to_owned(),
        (_, Some(JobStopBy::Model)) => "stopped by the model".to_owned(),
        (_, Some(JobStopBy::DaemonStop)) => "stopped because efrd stopped".to_owned(),
        (_, Some(JobStopBy::DaemonRestart)) => "stopped because efrd restarted".to_owned(),
        (_, Some(JobStopBy::Guard)) if quarantined => {
            "stopped: it changed git settings (quarantined)".to_owned()
        }
        (_, Some(JobStopBy::Guard)) => "stopped: it changed git settings".to_owned(),
        (_, Some(JobStopBy::FullOff)) => "stopped: full mode was turned off".to_owned(),
        (_, Some(_)) | (JobState::Stopped, None) => "stopped".to_owned(),
        (_, None) => match exit_code {
            Some(code) => format!("ended (exit {code})"),
            None => "ended".to_owned(),
        },
    }
}

/// Why a job's notice started no turn, in a few words.
fn held_reason(reason: HeldReason) -> &'static str {
    match reason {
        HeldReason::NotAuto => "the conversation is not in auto",
        HeldReason::WakeLimit => "wake limit reached",
        HeldReason::WakeTurnJob => "job of a wake turn",
        HeldReason::DaemonRestart => "efrd restarted",
        HeldReason::WakesOff => "jobs.max_wakes_per_hour is 0",
        _ => "held",
    }
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

/// `text` without control characters, cut to `max` characters with `...` at the cut. A
/// format character, which draws nothing or turns the text around it, shows as
/// `U+FFFD`.
fn one_line(text: &str, max: usize) -> String {
    let clean = text.chars().map(|c| match c {
        c if c.is_control() => ' ',
        c if is_format(c) => '\u{fffd}',
        c => c,
    });
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

/// Writes a notice for every committed event that deserves one, and every held notice
/// that became ready, until `stop`.
pub(crate) async fn follow(state: Arc<State>, stop: CancellationToken) {
    let mut committed = state.writer.subscribe();
    let mut left = LeftCommands::default();
    loop {
        let batch = tokio::select! {
            () = stop.cancelled() => return,
            batch = committed.recv() => Some(batch),
            () = state.connections.readied() => None,
        };
        // NOTE: what became ready before this batch was committed is written first, so
        // notices keep the order of the moments they were decided in.
        write_ready(&state).await;
        match batch {
            None => {}
            Some(Ok(batch)) => {
                for envelope in batch.events() {
                    let known = left.see(&envelope.event);
                    notify(&state, envelope, known.as_ref()).await;
                }
            }
            Some(Err(RecvError::Lagged(missed))) => {
                tracing::warn!(missed, "notices fell behind; some were not written");
            }
            Some(Err(RecvError::Closed)) => return,
        }
    }
}

/// Writes the held notices that became ready.
async fn write_ready(state: &State) {
    for notice in state.connections.take_ready() {
        write(state, notice).await;
    }
}

async fn notify(state: &State, envelope: &EventEnvelope, left: Option<&Left>) {
    let Some(conversation_id) = envelope.conversation_id else {
        return;
    };
    let minutes = state.settings.borrow().shell.interactive_timeout_minutes;
    let limit = Duration::from_secs(minutes.saturating_mul(60));
    let shell = |title| shell_line(&envelope.event, title, left, envelope.at, limit);
    if line(&envelope.event, None, None).is_none() && shell(None).is_none() {
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
    let wake = wake_turn(&state.readers, conversation_id, envelope).await;
    let title = summary.title.as_deref();
    let Some(text) = line(&envelope.event, title, wake.as_ref()).or_else(|| shell(title)) else {
        return;
    };
    let notice = Notice { tty, conversation: conversation_id, seq: envelope.seq, text };
    match state.connections.decide(notice, state.clock.now()) {
        Decision::Write(notice) => write(state, notice).await,
        Decision::Shown | Decision::Held => {}
    }
}

/// The [`Wake`] of the turn that `envelope` ends, when it is a `turn_completed` or a
/// `turn_failed` of a job notice turn; `None` for any other event or turn.
pub(crate) async fn wake_turn(
    readers: &efr_store::Readers,
    conversation_id: ConversationId,
    envelope: &EventEnvelope,
) -> Option<Wake> {
    let (Event::TurnCompleted { turn_id, .. } | Event::TurnFailed { turn_id, .. }) =
        &envelope.event
    else {
        return None;
    };
    let (turn_id, through) = (*turn_id, envelope.seq);
    let read = readers
        .with(move |conn| {
            let Some(turn) = efr_store::conversations::turn(conn, turn_id)? else {
                return Ok(None);
            };
            if !turn.wake {
                return Ok(None);
            }
            let page = efr_store::events::read_turn_range(
                conn,
                conversation_id,
                turn.queued_seq,
                through,
            )?;
            Ok(Some(wake_of(turn.id, &turn.prompt, &page)))
        })
        .await;
    read.unwrap_or_else(|error| {
        tracing::warn!(error = %error, %conversation_id, "the turn of a notice could not be read");
        None
    })
}

/// Appends a decided notice to its terminal's file under the runtime root.
async fn write(state: &State, notice: Notice) {
    let dir = state.dirs.runtime().join(NOTICES_DIR);
    let Notice { tty, conversation: conversation_id, text, .. } = notice;
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

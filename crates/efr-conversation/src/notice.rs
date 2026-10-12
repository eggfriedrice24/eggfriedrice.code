//! Job notices (efr's auto spec, section 3.18, phase C): the text that tells the model
//! that a background job ended or that a line of its output matched its `watch`
//! pattern.
//!
//! A notice is never the user's input. Its text says so on its own line, and it goes
//! into a turn as its own `job_notice` event, never as a prompt or a steer, so the
//! classifier's user messages and the name check never read it. The job wrote the
//! output that a notice quotes, so [`notice_text`] escapes it: the output cannot close
//! the `<job>` element, and no control or format character of it reaches the model
//! to hide text from the user.
//!
//! The daemon hands the actor a [`JobNoticeInput`] for each notice
//! ([`ConversationHandle::notice`](crate::ConversationHandle::notice)).

use std::fmt::Write as _;

use efr_protocol::{JobInfo, JobState, JobStopBy, NoticeKind, Seq};
use efr_stdx::text::is_format;

/// The line of every notice that says that no person wrote it.
pub const NOT_USER_INPUT: &str =
    "efr: this is a job notice, not a message from the user; no human input has occurred.";

/// The most lines of the job's output that a notice of its end quotes.
pub const NOTICE_TAIL_LINES: usize = 50;

/// The most bytes of the job's output that a notice of its end quotes.
pub const NOTICE_TAIL_BYTES: usize = 8 * 1024;

/// The most bytes of a matched line that a watch notice quotes.
pub const NOTICE_LINE_BYTES: usize = 4 * 1024;

/// The most characters of the job's command line that a notice names.
const COMMAND_CHARS: usize = 500;

/// What a notice tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeBody<'a> {
    /// The job ended.
    Ended {
        /// How it ended.
        state: JobState,
        /// The exit status of its command, when it exited.
        exit_code: Option<i32>,
        /// Who or what stopped it.
        stopped_by: Option<JobStopBy>,
        /// The end of its output, as the job wrote it.
        tail: &'a str,
    },
    /// A line of the job's output matched its watch pattern.
    Watch {
        /// The line, as the job wrote it.
        line: &'a str,
        /// How many more matches may wake the model.
        wakes_left: u8,
    },
}

/// One notice, as the daemon hands it to the conversation's actor.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct JobNoticeInput {
    /// The job, as its events tell it now.
    pub job: JobInfo,
    /// What the notice tells.
    pub kind: NoticeKind,
    /// The sequence number of the `job_ended` or `job_watch_matched` event that the
    /// notice tells about.
    pub source_seq: Seq,
    /// The text of the notice, from [`notice_text`].
    pub text: String,
    /// False when the notice must not start a turn of its own and waits for the next
    /// turn that a prompt starts, such as for a job that ran when efrd restarted. The
    /// actor also holds the notices of a job that a notice turn started.
    pub wakes: bool,
}

impl JobNoticeInput {
    /// The notice `text` of `kind` about `job`, from the event `source_seq`, which may
    /// start a turn when `wakes`.
    pub fn new(job: JobInfo, kind: NoticeKind, source_seq: Seq, text: String, wakes: bool) -> Self {
        JobNoticeInput { job, kind, source_seq, text, wakes }
    }
}

/// The text of a notice about `job`, with its output file as the model reads it
/// (`shown_output`, such as `$SCRATCH/jobs/j3.out`):
///
/// ```text
/// <job id="j3" state="exited" exit="0" command="npm test">
/// efr: this is a job notice, not a message from the user; no human input has occurred.
/// <the last lines of the output>
/// output: $SCRATCH/jobs/j3.out
/// </job>
/// ```
///
/// A watch notice has `state="running" watch="matched"` and quotes the matched line.
/// The command, the output and the line are escaped (`<`, `>`, `&`, `"`), and their
/// control and format characters are dropped; a tab becomes a space.
pub fn notice_text(job: &JobInfo, shown_output: &str, body: NoticeBody<'_>) -> String {
    let mut text = String::new();
    let _ = write!(text, "<job id=\"{}\"", job.id);
    match body {
        NoticeBody::Ended { state, exit_code, stopped_by, .. } => {
            let _ = write!(text, " state=\"{}\"", state.as_str());
            if let Some(code) = exit_code {
                let _ = write!(text, " exit=\"{code}\"");
            }
            if let Some(by) = stopped_by {
                let _ = write!(text, " stopped_by=\"{}\"", by.as_str());
            }
        }
        NoticeBody::Watch { .. } => text.push_str(" state=\"running\" watch=\"matched\""),
    }
    let _ = writeln!(text, " command=\"{}\">", escape(&one_line(&job.command, COMMAND_CHARS)));
    let _ = writeln!(text, "{NOT_USER_INPUT}");
    match body {
        NoticeBody::Ended { tail, .. } => {
            let tail = last_lines(tail);
            if tail.is_empty() {
                text.push_str("(no output)\n");
            }
            for line in tail {
                let _ = writeln!(text, "{}", escape(&clean(line)));
            }
        }
        NoticeBody::Watch { line, wakes_left } => {
            let line = cut_bytes(line, NOTICE_LINE_BYTES);
            let _ = writeln!(text, "matched: {}", escape(&one_line(line, usize::MAX)));
            let _ = writeln!(text, "wakes left: {wakes_left}");
        }
    }
    let _ = writeln!(text, "output: {}", escape(&clean(shown_output)));
    text.push_str("</job>");
    text
}

/// The text that a question about a background job adds to its facts: how long the
/// job and the grants of the question last (phase C). The same text serves a question
/// in `auto` and the question that a job asks in `cautious` and `manual`.
pub fn job_lifetime_fact(minutes: u32) -> String {
    format!("runs as a job for up to {minutes} minutes; these grants stay open until it ends")
}

/// The last [`NOTICE_TAIL_LINES`] lines of `tail`, within [`NOTICE_TAIL_BYTES`]
/// bytes, without a last empty line.
fn last_lines(tail: &str) -> Vec<&str> {
    let tail = tail.strip_suffix('\n').unwrap_or(tail);
    if tail.is_empty() {
        return Vec::new();
    }
    let mut start = tail.len().saturating_sub(NOTICE_TAIL_BYTES);
    while !tail.is_char_boundary(start) {
        start += 1;
    }
    let lines: Vec<&str> = tail[start..].split('\n').collect();
    let skip = lines.len().saturating_sub(NOTICE_TAIL_LINES);
    lines.into_iter().skip(skip).collect()
}

/// At most the first `max` bytes of `text`, cut at a character boundary.
fn cut_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `text` without control and format characters; a tab becomes a space.
fn clean(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\t' => Some(' '),
            c if c.is_control() || is_format(c) => None,
            c => Some(c),
        })
        .collect()
}

/// `text` on one line: each line break a space, then [`clean`], cut at `max`
/// characters with `...`.
fn one_line(text: &str, max: usize) -> String {
    let flat = clean(&text.replace(['\n', '\r'], " "));
    if flat.chars().count() <= max {
        return flat;
    }
    let mut cut: String = flat.chars().take(max).collect();
    cut.push_str("...");
    cut
}

/// `text` with `&`, `<`, `>` and `"` as entities, so it cannot end an attribute or
/// the element.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            c => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests;

//! The CLI's lines about background jobs (efr's auto spec, sections 3.18 and 14.8): the
//! start and the end of a job, a line that matched its `watch` pattern, a notice that
//! did not start a turn, a port that efr forwards or not (with the URL as an OSC 8
//! link on a terminal), the mark of a turn that a job notice started, the question of
//! the stop key, and the rows of `efr jobs list` (with the forwarded ports of a running
//! job).
//!
//! A job's command, its output and its tail come from a program, not from efr: every
//! piece of them goes through [`one_line`] or [`lines`], so a control character or a
//! format character (a bidi mark, a zero width character) shows as a stand-in and can
//! never drive the terminal or hide text.

use std::fmt::Write as _;
use std::path::Path;

use efr_protocol::{
    Blocked, Event, ForwardOutcome, HeldReason, JobId, JobInfo, JobState, JobStopBy, SurfaceChange,
};
use efr_render::{RenderOptions, WidthMethod};
use jiff::Timestamp;
use unicode_width::UnicodeWidthStr as _;

use super::{ago, cut, lines, one_line, sandbox, size, until};

/// The line of a turn that a job notice started, not the user: the view and
/// `efr history` show it where a prompt would stand.
pub(crate) const WAKE_MARK: &str = "\u{1f916} (job notice, not your input)";

/// The note of the stop key when no job of the conversation runs.
pub(crate) const NO_JOB: &str = "no job runs";

/// The keys of the stop key's question.
pub(crate) const STOP_KEYS: &[(&str, &str)] = &[("y", "yes"), ("n", "no")];

/// How many columns of a job's command a question or a row of `efr jobs list` shows.
const COMMAND_COLUMNS: usize = 60;

/// How many lines of a job's tail `efr jobs stop` shows.
pub(crate) const STOP_TAIL_LINES: usize = 10;

/// The output file of a job as the model names it: `$SCRATCH/jobs/j3.out`. A path
/// whose dir is not `jobs` shows whole, safe to print.
pub(crate) fn output_name(path: &Path) -> String {
    let in_jobs = path.parent().and_then(Path::file_name).is_some_and(|dir| dir == "jobs");
    match path.file_name().filter(|_| in_jobs) {
        Some(name) => format!("$SCRATCH/jobs/{}", one_line(&name.to_string_lossy())),
        None => one_line(&path.to_string_lossy()),
    }
}

/// A time in seconds as a job's lines say it: `12 seconds`, `1 minute`, `92 minutes`.
pub(crate) fn minutes(seconds: u64) -> String {
    match seconds {
        1 => "1 second".to_owned(),
        0..60 => format!("{seconds} seconds"),
        60..120 => "1 minute".to_owned(),
        _ => format!("{} minutes", seconds / 60),
    }
}

/// The seconds from `from` to `to`; 0 when `to` is earlier.
fn seconds_between(from: Timestamp, to: Timestamp) -> u64 {
    u64::try_from(to.duration_since(from).as_secs()).unwrap_or(0)
}

/// The first line of a job's command, safe to print and cut to `columns`, with `…` when
/// lines follow.
pub(crate) fn command(command: &str, columns: usize) -> String {
    let mut parts = command.lines().filter(|line| !line.trim().is_empty());
    let first = one_line(parts.next().unwrap_or_default().trim());
    let more = parts.next().is_some();
    let shown = cut(&first, columns, WidthMethod::CodePoint);
    if more && !shown.ends_with('\u{2026}') { format!("{shown} \u{2026}") } else { shown }
}

/// The line of a job's start: `job j3 started; output $SCRATCH/jobs/j3.out; ends in 180
/// minutes`.
pub(crate) fn started(job: &JobInfo) -> String {
    format!(
        "job {} started; output {}; ends in {}",
        job.id,
        output_name(&job.output),
        minutes(seconds_between(job.started_at, job.ends_by))
    )
}

/// The facts of a job's end, from its `job_ended` or from its [`JobInfo`].
#[derive(Debug, Clone, Default)]
pub(crate) struct Ended<'a> {
    pub(crate) state: Option<JobState>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) stopped_by: Option<JobStopBy>,
    pub(crate) output_bytes: u64,
    pub(crate) ran_secs: u64,
    pub(crate) surface_changes: &'a [SurfaceChange],
    pub(crate) quarantined: bool,
    pub(crate) survivors: &'a [String],
    pub(crate) launcher_lost: bool,
    pub(crate) output_truncated: bool,
    /// The connections that efr's proxy refused while the job ran, and the count of
    /// the rest (phase D2).
    pub(crate) blocked: &'a [Blocked],
    pub(crate) blocked_more: u32,
}

impl<'a> Ended<'a> {
    /// The end that `event` records, with its job; `None` for any other event.
    pub(crate) fn of(event: &'a Event) -> Option<(&'a JobId, Ended<'a>)> {
        let Event::JobEnded {
            job_id,
            state,
            exit_code,
            stopped_by,
            output_bytes,
            ran_secs,
            surface_changes,
            quarantined,
            survivors,
            launcher_lost,
            output_truncated,
            blocked,
            blocked_more,
            ..
        } = event
        else {
            return None;
        };
        let ended = Ended {
            state: Some(*state),
            exit_code: *exit_code,
            stopped_by: *stopped_by,
            output_bytes: *output_bytes,
            ran_secs: *ran_secs,
            surface_changes,
            quarantined: *quarantined,
            survivors,
            launcher_lost: *launcher_lost,
            output_truncated: *output_truncated,
            blocked,
            blocked_more: *blocked_more,
        };
        Some((job_id, ended))
    }

    /// The end of `job` as `jobs.list` or `jobs.stop` answers it. The facts that only
    /// the `job_ended` holds (the survivors, a lost launcher) are not known.
    pub(crate) fn from_info(job: &JobInfo) -> Ended<'static> {
        let ran = job.ended_at.map_or(0, |end| seconds_between(job.started_at, end));
        Ended {
            state: Some(job.state),
            exit_code: job.exit_code,
            stopped_by: job.stopped_by,
            output_bytes: job.output_bytes,
            ran_secs: ran,
            ..Ended::default()
        }
    }

    /// True when processes of the job may still run.
    pub(crate) fn leaves_processes(&self) -> bool {
        self.launcher_lost || !self.survivors.is_empty()
    }
}

/// The line of a job's end, such as `job j3 stopped by you after 92 minutes; 1.2 MB
/// output`, `job j3 ended with exit 3 after 1 minute; 2.0 KB output` or `job j3 stopped
/// because efrd restarted`, with what the user must know about it after `; `.
pub(crate) fn ended(job_id: &JobId, end: &Ended<'_>) -> String {
    let (what, timed) = match (end.state, end.stopped_by) {
        (_, Some(JobStopBy::DaemonRestart)) => ("stopped because efrd restarted".to_owned(), false),
        (_, Some(JobStopBy::DaemonStop)) => ("stopped because efrd stopped".to_owned(), false),
        (_, Some(JobStopBy::Guard)) => {
            let what = if end.quarantined {
                "stopped: it changed git settings (quarantined)"
            } else {
                "stopped: it changed git settings"
            };
            (what.to_owned(), false)
        }
        (_, Some(JobStopBy::FullOff)) => ("stopped: full mode was turned off".to_owned(), false),
        (Some(JobState::TimedOut), _) | (_, Some(JobStopBy::TimeLimit)) => {
            ("timed out".to_owned(), true)
        }
        (_, Some(JobStopBy::User)) => ("stopped by you".to_owned(), true),
        (_, Some(JobStopBy::Model)) => ("stopped by the model".to_owned(), true),
        (Some(JobState::Exited), _) => match end.exit_code {
            Some(code) => (format!("ended with exit {code}"), true),
            None => ("ended".to_owned(), true),
        },
        (Some(JobState::Failed), _) => match end.exit_code {
            Some(code) => (format!("failed with exit {code}"), true),
            None => ("failed".to_owned(), true),
        },
        (Some(JobState::Stopped), _) => ("stopped".to_owned(), true),
        (Some(JobState::Running), _) => ("runs".to_owned(), false),
        _ => ("ended".to_owned(), true),
    };
    let mut line = format!("job {job_id} {what}");
    if timed {
        let _ = write!(line, " after {}; {} output", minutes(end.ran_secs), size(end.output_bytes));
    }
    if !end.surface_changes.is_empty() && end.stopped_by != Some(JobStopBy::Guard) {
        line.push_str(if end.quarantined {
            "; it changed git settings (quarantined)"
        } else {
            "; it changed git settings"
        });
    }
    if end.launcher_lost {
        line.push_str("; its launcher was lost, and its processes may still run");
    }
    match end.survivors.len() {
        0 => {}
        1 => line.push_str("; 1 process of the job may still run"),
        count => {
            let _ = write!(line, "; {count} processes of the job may still run");
        }
    }
    if end.output_truncated {
        line.push_str("; efr could not write all of its output to the file");
    }
    if let Some(refused) = sandbox::blocked_joined(end.blocked, end.blocked_more) {
        let _ = write!(line, "; {refused}");
    }
    line
}

/// The line of an output line that matched a job's `watch` pattern: `job j3 matched:
/// <line>`, on one line and safe to print.
pub(crate) fn matched(job_id: &JobId, line: &str) -> String {
    format!("job {job_id} matched: {}", one_line(line))
}

/// The URL of a port that efr forwards for a job: `http://localhost:8765`.
pub(crate) fn forward_url(port: u16) -> String {
    format!("http://localhost:{port}")
}

/// The line of a `job_port_forwarded` event (DL15 of the v0.0.4 live items plan):
/// `job j1 listens on 127.0.0.1:8765 -> http://localhost:8765`, `job j1 listens on
/// [::1]:5173 -> ...`, `...; not forwarded: <why>`, or `job j1: port 8765 is no longer
/// forwarded (jobs.forward_ports is off)`. An outcome that this efr does not know shows
/// the plain listen, without a reason. The address and the reason come from efrd, and
/// still go through [`one_line`].
pub(crate) fn forward(
    job_id: &JobId,
    listen: &str,
    port: u16,
    outcome: ForwardOutcome,
    by_job: Option<&JobId>,
    reason: Option<&str>,
) -> String {
    let listen = one_line(listen);
    let address = if listen.contains(':') {
        format!("[{listen}]:{port}")
    } else {
        format!("{listen}:{port}")
    };
    let reason = reason.map(one_line);
    let head = format!("job {job_id} listens on {address}");
    let why = match outcome {
        ForwardOutcome::Forwarded => return format!("{head} -> {}", forward_url(port)),
        ForwardOutcome::InUse => match by_job {
            Some(other) => format!("127.0.0.1:{port} is in use by job {other}"),
            None => format!("127.0.0.1:{port} is in use on this machine"),
        },
        ForwardOutcome::Refused => match reason {
            Some(reason) => format!("efr does not forward port {port} ({reason})"),
            None => format!("efr does not forward port {port}"),
        },
        ForwardOutcome::Limit => {
            reason.unwrap_or_else(|| "efr forwards as many ports as it may".to_owned())
        }
        ForwardOutcome::Stopped => {
            return match reason {
                Some(reason) => {
                    format!("job {job_id}: port {port} is no longer forwarded ({reason})")
                }
                None => format!("job {job_id}: port {port} is no longer forwarded"),
            };
        }
        _ => return head,
    };
    format!("{head}; not forwarded: {why}")
}

/// The URL that the line of `event` links to: that of a forwarded port; `None` for any
/// other event.
pub(crate) fn link_of(event: &Event) -> Option<String> {
    match event {
        Event::JobPortForwarded { port, outcome: ForwardOutcome::Forwarded, .. } => {
            Some(forward_url(*port))
        }
        _ => None,
    }
}

/// `painted`, a line that the CLI rendered, with the last copy of `url` in it as an
/// OSC 8 hyperlink, when the output is a terminal and links are on; else unchanged.
/// `url` must be one that efr made ([`forward_url`]): it goes into the escape sequence
/// as it is. A line that wrapped `url` over two rows keeps it as plain text.
pub(crate) fn linked(painted: &str, url: Option<&str>, options: &RenderOptions) -> String {
    let Some(url) = url.filter(|_| options.is_terminal() && options.hyperlinks()) else {
        return painted.to_owned();
    };
    match painted.rfind(url) {
        Some(at) => {
            let (before, rest) = painted.split_at(at);
            let after = &rest[url.len()..];
            format!("{before}\x1b]8;;{url}\x1b\\{url}\x1b]8;;\x1b\\{after}")
        }
        None => painted.to_owned(),
    }
}

/// `ports 8765, 5173`: the ports that efr forwards for a running job; `None` for a job
/// that does not run or forwards none.
pub(crate) fn ports(job: &JobInfo) -> Option<String> {
    if job.state != JobState::Running || job.forwards.is_empty() {
        return None;
    }
    let ports: Vec<String> = job.forwards.iter().map(|forward| forward.port.to_string()).collect();
    Some(format!("ports {}", ports.join(", ")))
}

/// Why a job notice did not start a turn, in the words of the view.
pub(crate) fn held_reason(reason: HeldReason) -> &'static str {
    match reason {
        HeldReason::NotAuto => "the conversation is not in auto",
        HeldReason::WakeLimit => "wake limit reached",
        HeldReason::WakeTurnJob => "job of a wake turn",
        HeldReason::DaemonRestart => "efrd restarted",
        HeldReason::WakesOff => "jobs.max_wakes_per_hour is 0",
        _ => "for a reason that this efr does not know",
    }
}

/// The line of a notice that did not start a turn: `job j3: no wake turn (wake limit
/// reached); the notice waits for your next prompt`.
pub(crate) fn held(job_id: &JobId, reason: HeldReason) -> String {
    format!(
        "job {job_id}: no wake turn ({}); the notice waits for your next prompt",
        held_reason(reason)
    )
}

/// The note of a notice that rides in a turn that a prompt started.
pub(crate) fn notice_rides(job_id: &JobId) -> String {
    format!("the model also reads a notice of job {job_id}; it is not your input")
}

/// The line of a job event of the conversation, which belongs to no turn: the end of a
/// job, a matched line, a notice that did not start a turn, a port that efr forwards or
/// not. `None` for any other event.
pub(crate) fn conversation_line(event: &Event) -> Option<String> {
    match event {
        Event::JobEnded { .. } => Ended::of(event).map(|(job_id, end)| ended(job_id, &end)),
        Event::JobWatchMatched { job_id, line, .. } => Some(matched(job_id, line)),
        Event::JobNoticeHeld { job_id, reason, .. } => Some(held(job_id, *reason)),
        Event::JobPortForwarded { job_id, listen, port, outcome, by_job, reason } => {
            Some(forward(job_id, listen, *port, *outcome, by_job.as_ref(), reason.as_deref()))
        }
        _ => None,
    }
}

/// The last `count` lines of a job's tail with text in them, each safe to print.
pub(crate) fn tail_lines(tail: &str, count: usize) -> Vec<String> {
    let safe = lines(tail);
    let all: Vec<&str> = safe.lines().filter(|line| !line.trim().is_empty()).collect();
    all[all.len().saturating_sub(count)..].iter().map(|line| (*line).to_owned()).collect()
}

/// The title of the stop key's question about `job`: `stop job j3 (npm run dev)`.
pub(crate) fn stop_title(job: &JobInfo) -> String {
    format!("stop job {} ({})", job.id, command(&job.command, COMMAND_COLUMNS))
}

/// One row of `efr jobs list`.
#[derive(Debug, Clone)]
pub(crate) struct ListRow<'a> {
    pub(crate) job: &'a JobInfo,
    /// The title of the job's conversation, for `--all`.
    pub(crate) conversation: Option<String>,
    /// Processes of the job may still run: its `job_ended` named survivors or a lost
    /// launcher.
    pub(crate) leftover: bool,
}

/// The state of `job` in a word or two, for a row of `efr jobs list`.
fn state_words(job: &JobInfo) -> String {
    match (job.state, job.stopped_by) {
        (JobState::Running, _) => "running".to_owned(),
        (_, Some(JobStopBy::DaemonRestart)) => "stopped (efrd restarted)".to_owned(),
        (_, Some(JobStopBy::DaemonStop)) => "stopped (efrd stopped)".to_owned(),
        (_, Some(JobStopBy::Guard)) => "stopped (git settings)".to_owned(),
        (_, Some(JobStopBy::FullOff)) => "stopped (full mode off)".to_owned(),
        (JobState::TimedOut, _) | (_, Some(JobStopBy::TimeLimit)) => "timed out".to_owned(),
        (_, Some(JobStopBy::User)) => "stopped by you".to_owned(),
        (_, Some(JobStopBy::Model)) => "stopped by the model".to_owned(),
        (JobState::Exited, _) => {
            job.exit_code.map_or_else(|| "ended".to_owned(), |c| format!("exit {c}"))
        }
        (JobState::Failed, _) => "failed".to_owned(),
        (JobState::Stopped, _) => "stopped".to_owned(),
        _ => "unknown".to_owned(),
    }
}

/// When `job` ends or ended, at `now`: `ends in 52m 3s`, `ends now`, `ended 5m 3s ago`.
fn when(job: &JobInfo, now: Timestamp) -> String {
    match (job.state, job.ended_at) {
        // NOTE: a job past its time limit ends at once; the daemon's clock may differ.
        (JobState::Running, _) if job.ends_by <= now => "ends now".to_owned(),
        (JobState::Running, _) => format!("ends {}", until(job.ends_by, now)),
        (_, Some(ended)) => format!("ended {}", ago(ended, now)),
        _ => "ended".to_owned(),
    }
}

/// The rows of `efr jobs list` at `now`, in columns: the id, the state, when it ends or
/// ended, the size of its output, its command, and a mark when its processes may still
/// run. With `--all` the conversation's title comes first.
pub(crate) fn list(rows: &[ListRow<'_>], now: Timestamp) -> String {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let mut cells = Vec::new();
            if let Some(title) = &row.conversation {
                cells.push(cut(&one_line(title), 24, WidthMethod::CodePoint));
            }
            cells.push(row.job.id.to_string());
            cells.push(state_words(row.job));
            cells.push(when(row.job, now));
            cells.push(size(row.job.output_bytes));
            let mut command = command(&row.job.command, COMMAND_COLUMNS);
            if let Some(ports) = ports(row.job) {
                let _ = write!(command, "  ({ports})");
            }
            if row.leftover {
                command.push_str("  (its processes may still run)");
            }
            cells.push(command);
            cells
        })
        .collect();
    let columns = cells.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|at| {
            cells.iter().filter_map(|row| row.get(at)).map(|cell| cell.width()).max().unwrap_or(0)
        })
        .collect();
    // NOTE: the size is the cell before the command, and its numbers line up at the
    // right.
    let size_at = columns.saturating_sub(2);
    let mut out = String::new();
    for row in &cells {
        let last = row.len().saturating_sub(1);
        for (at, cell) in row.iter().enumerate() {
            let pad = " ".repeat(widths[at].saturating_sub(cell.width()));
            if at == last {
                out.push_str(cell);
            } else if at == size_at {
                let _ = write!(out, "{pad}{cell}  ");
            } else {
                let _ = write!(out, "{cell}{pad}  ");
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests;

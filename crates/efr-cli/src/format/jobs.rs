//! The CLI's lines about background jobs (efr's auto spec, sections 3.18 and 14.8): the
//! start and the end of a job, a line that matched its `watch` pattern, a notice that
//! did not start a turn, the mark of a turn that a job notice started, and the
//! question of the stop key.
//!
//! A job's command, its output and its tail come from a program, not from efr: every
//! piece of them goes through [`one_line`] or [`lines`], so a control character or a
//! format character (a bidi mark, a zero width character) shows as a stand-in and can
//! never drive the terminal or hide text.

use std::fmt::Write as _;
use std::path::Path;

use efr_protocol::{Event, HeldReason, JobId, JobInfo, JobState, JobStopBy, SurfaceChange};
use efr_render::WidthMethod;
use jiff::Timestamp;

use super::{cut, one_line, size};

/// The line of a turn that a job notice started, not the user: the view and
/// `efr history` show it where a prompt would stand.
pub(crate) const WAKE_MARK: &str = "\u{1f916} (job notice, not your input)";

/// The note of the stop key when no job of the conversation runs.
pub(crate) const NO_JOB: &str = "no job runs";

/// The keys of the stop key's question.
pub(crate) const STOP_KEYS: &[(&str, &str)] = &[("y", "yes"), ("n", "no")];

/// How many columns of a job's command a question shows.
const COMMAND_COLUMNS: usize = 60;

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
        };
        Some((job_id, ended))
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
    line
}

/// The line of an output line that matched a job's `watch` pattern: `job j3 matched:
/// <line>`, on one line and safe to print.
pub(crate) fn matched(job_id: &JobId, line: &str) -> String {
    format!("job {job_id} matched: {}", one_line(line))
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
/// job, a matched line, a notice that did not start a turn. `None` for any other event.
pub(crate) fn conversation_line(event: &Event) -> Option<String> {
    match event {
        Event::JobEnded { .. } => Ended::of(event).map(|(job_id, end)| ended(job_id, &end)),
        Event::JobWatchMatched { job_id, line, .. } => Some(matched(job_id, line)),
        Event::JobNoticeHeld { job_id, reason, .. } => Some(held(job_id, *reason)),
        _ => None,
    }
}

/// The title of the stop key's question about `job`: `stop job j3 (npm run dev)`.
pub(crate) fn stop_title(job: &JobInfo) -> String {
    format!("stop job {} ({})", job.id, command(&job.command, COMMAND_COLUMNS))
}

#[cfg(test)]
mod tests;

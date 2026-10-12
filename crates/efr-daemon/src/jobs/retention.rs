//! How long efrd keeps the output files of ended jobs (efr's auto spec, section 3.18,
//! phase C): an output goes [`KEEP_DAYS`] days after its job ended, and each
//! conversation keeps at most [`KEEP_PER_CONVERSATION`] outputs of ended jobs, the
//! newest. The output of a running job stays. The hourly collector of the snapshots
//! (`run.rs`) runs it.
//!
//! The paths come from the jobs projection, which efrd wrote. efrd removes only a file
//! or link named `j<n>.out` in a directory named `jobs`, never through a link: a
//! removal of a link removes the link.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use efr_protocol::{ConversationId, JobInfo, JobState};
use efr_store::Readers;
use jiff::{SignedDuration, Timestamp};

/// How many days efrd keeps the output of a job after its end.
pub(crate) const KEEP_DAYS: i64 = 7;

/// How many outputs of ended jobs each conversation keeps.
pub(crate) const KEEP_PER_CONVERSATION: usize = 20;

/// The output files of the ended jobs of `jobs` that go at `now`: those that ended
/// [`KEEP_DAYS`] days ago or more, and those of a conversation beyond its newest
/// [`KEEP_PER_CONVERSATION`].
pub(crate) fn doomed(jobs: &[JobInfo], now: Timestamp) -> Vec<PathBuf> {
    let oldest = now - SignedDuration::from_hours(24 * KEEP_DAYS);
    let mut ended: HashMap<ConversationId, Vec<(Timestamp, u32, &Path)>> = HashMap::new();
    for job in jobs.iter().filter(|job| job.state != JobState::Running) {
        let Some(at) = job.ended_at else { continue };
        let number = job.id.number().unwrap_or(0);
        ended.entry(job.conversation_id).or_default().push((at, number, &job.output));
    }
    let mut out = Vec::new();
    for mut outputs in ended.into_values() {
        outputs.sort_by_key(|(at, number, _)| std::cmp::Reverse((*at, *number)));
        for (place, (at, _, path)) in outputs.into_iter().enumerate() {
            if place >= KEEP_PER_CONVERSATION || at <= oldest {
                out.push(path.to_path_buf());
            }
        }
    }
    out.sort();
    out
}

/// Removes the job output `path` when it is one; true when it went. It blocks.
pub(crate) fn remove(path: &Path) -> bool {
    let named = path.file_name().and_then(|name| name.to_str()).is_some_and(is_output_name);
    let in_jobs = path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|dir| dir == std::ffi::OsStr::new(efr_sandbox::JOB_DIR));
    if !named || !in_jobs {
        return false;
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() || meta.file_type().is_symlink() => {
            std::fs::remove_file(path).is_ok()
        }
        _ => false,
    }
}

/// True for `j<n>.out`.
fn is_output_name(name: &str) -> bool {
    name.strip_prefix('j').and_then(|rest| rest.strip_suffix(".out")).is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// Removes the outputs that [`doomed`] names for the jobs of the store at `now`; how
/// many went.
pub(crate) async fn collect(readers: &Readers, now: Timestamp) -> usize {
    let rows = readers.with(|conn| efr_store::jobs::all(conn, u32::MAX)).await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, "the jobs whose outputs may go could not be read");
            return 0;
        }
    };
    let jobs: Vec<JobInfo> = rows.into_iter().map(|row| row.info).collect();
    let paths = doomed(&jobs, now);
    if paths.is_empty() {
        return 0;
    }
    tokio::task::spawn_blocking(move || paths.iter().filter(|path| remove(path)).count())
        .await
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;

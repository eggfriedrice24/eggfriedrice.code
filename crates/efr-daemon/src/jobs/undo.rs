//! Which running job keeps an undo from running (efr's auto spec, sections 3.18 and
//! 10.5, phase C).
//!
//! A job can write in the roots of a turn while the undo restores them, and a job's
//! files join the turn that started it at the job's end. So `turn.undo` refuses while a
//! job of the same conversation runs, and while a job of another conversation can write
//! in a root of the turn: a contained job whose write roots hold one of the turn's
//! roots or lie in one, and every exit child job, which writes as the user wherever the
//! user can. The user stops the job first.

use std::path::{Path, PathBuf};

use efr_conversation::UndoBusyReason;
use efr_protocol::{ConversationId, TurnId};

use super::{JobTable, RunningJob};
use crate::tools::CallSnapshots;

/// Why an undo of the turn `turn` of `conversation` must wait for a running job of
/// `jobs`; `None` when no job is in the way. The turn's roots are the roots whose
/// snapshot store holds the turn.
pub(crate) async fn blocker(
    jobs: &JobTable,
    snapshots: Option<&CallSnapshots>,
    conversation: ConversationId,
    turn: TurnId,
) -> Option<UndoBusyReason> {
    let running = jobs.running();
    if running.is_empty() {
        return None;
    }
    if let Some(own) = newest_of(&running, conversation) {
        return Some(UndoBusyReason::Job { number: own, same_conversation: true });
    }
    let roots = match snapshots {
        Some(snapshots) => snapshots
            .store()
            .turns_with_refs(conversation)
            .await
            .into_iter()
            .find(|refs| refs.turn == turn)
            .map(|refs| refs.roots)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let running = tokio::task::spawn_blocking(move || {
        running
            .into_iter()
            .map(|mut job| {
                job.write_roots = job.write_roots.iter().map(|root| canonical(root)).collect();
                job
            })
            .collect::<Vec<_>>()
    })
    .await
    .ok()?;
    other_in(&running, conversation, &roots)
        .map(|number| UndoBusyReason::Job { number, same_conversation: false })
}

/// The number of the newest running job of `conversation`.
pub(crate) fn newest_of(running: &[RunningJob], conversation: ConversationId) -> Option<u32> {
    running
        .iter()
        .filter(|job| job.key.conversation == conversation)
        .map(|job| job.key.number)
        .max()
}

/// The number of the first running job of another conversation than `conversation`
/// that can write in one of `roots` (canonical paths, as the job's write roots).
pub(crate) fn other_in(
    running: &[RunningJob],
    conversation: ConversationId,
    roots: &[PathBuf],
) -> Option<u32> {
    let mut found: Vec<&RunningJob> = running
        .iter()
        .filter(|job| job.key.conversation != conversation && writes_in(job, roots))
        .collect();
    found.sort_by_key(|job| (job.started_at, job.key.number));
    found.first().map(|job| job.key.number)
}

/// True when `job` can write in one of `roots`.
fn writes_in(job: &RunningJob, roots: &[PathBuf]) -> bool {
    // NOTE: an exit child job runs as the user, outside every write root.
    if !job.contained {
        return true;
    }
    job.write_roots.iter().any(|written| {
        roots.iter().any(|root| written.starts_with(root) || root.starts_with(written))
    })
}

/// `path` with its links resolved, or as it is when it cannot be resolved. It blocks.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests;

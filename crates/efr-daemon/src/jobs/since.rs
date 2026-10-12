//! Since when a program counts as changed in a turn (efr's auto spec 9.1, phase C).
//!
//! A program that a call of the turn wrote asks the user before it runs outside the
//! sandbox (`sandbox/facts.rs`). A background job writes between turns too, so the time
//! starts at the earliest of the turn's start and the start of each job of the
//! conversation that ran since the previous turn started: a program that a job wrote
//! after the previous turn still counts as changed.

use efr_protocol::{ConversationId, JobInfo, TurnId};
use jiff::Timestamp;

use super::JobTable;

impl JobTable {
    /// The time from which a program counts as changed in the turn `turn` of
    /// `conversation`, which started at `turn_start`.
    pub(crate) async fn changed_from(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        turn_start: Timestamp,
    ) -> Timestamp {
        let jobs = match self.list(Some(conversation)).await {
            Ok(jobs) if !jobs.is_empty() => jobs,
            Ok(_) => return turn_start,
            Err(error) => {
                tracing::warn!(%error, "the jobs of a conversation could not be read for its facts");
                return turn_start;
            }
        };
        let turns = self
            .inner
            .readers
            .with(move |conn| efr_store::conversations::turns(conn, conversation));
        let previous = match turns.await {
            Ok(turns) => {
                let before = turns.iter().position(|known| known.id == turn).unwrap_or(turns.len());
                turns[..before].iter().filter_map(|known| known.started_at).max()
            }
            Err(error) => {
                tracing::warn!(%error, "the turns of a conversation could not be read for its facts");
                None
            }
        };
        earliest(turn_start, previous, &jobs)
    }
}

/// The earliest of `turn_start` and the start of each of `jobs` that ran at or after
/// `previous`, the start of the previous turn; every job counts when there was none.
pub(crate) fn earliest(
    turn_start: Timestamp,
    previous: Option<Timestamp>,
    jobs: &[JobInfo],
) -> Timestamp {
    jobs.iter()
        .filter(|job| match (job.ended_at, previous) {
            (Some(ended), Some(previous)) => ended >= previous,
            _ => true,
        })
        .map(|job| job.started_at)
        .fold(turn_start, Timestamp::min)
}

#[cfg(test)]
mod tests;

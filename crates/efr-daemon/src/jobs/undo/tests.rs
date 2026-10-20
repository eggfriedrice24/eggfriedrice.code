use std::path::PathBuf;

use efr_protocol::{CallId, ConversationId, TurnId};
use jiff::Timestamp;

use super::{newest_of, other_in, undo_roots};
use crate::jobs::{JobKey, RunningJob};

fn conversation(n: u128) -> ConversationId {
    ConversationId::from_uuid(uuid::Uuid::from_u128(n))
}

fn job(conversation_n: u128, number: u32, write_roots: &[&str], contained: bool) -> RunningJob {
    RunningJob {
        key: JobKey { conversation: conversation(conversation_n), number },
        call: CallId::from_uuid(uuid::Uuid::from_u128(u128::from(number) + 100)),
        turn: TurnId::from_uuid(uuid::Uuid::from_u128(7)),
        guard_roots: Vec::new(),
        write_roots: write_roots.iter().map(PathBuf::from).collect(),
        started_at: Timestamp::from_second(1_790_000_000 + i64::from(number)).unwrap(),
        contained,
        call_dir: PathBuf::from("/r/call"),
        state_dir: PathBuf::from("/s/state"),
    }
}

/// Any running job of the conversation keeps its undo waiting; the newest one is named.
#[test]
fn a_job_of_the_same_conversation_blocks_its_undo() {
    let running = [job(1, 2, &["/p/app"], true), job(1, 5, &["/p/other"], true)];
    assert_eq!(newest_of(&running, conversation(1)), Some(5));
    assert_eq!(newest_of(&running, conversation(2)), None);
}

/// A job of another conversation blocks only when it can write in a root of the turn:
/// a write root at, above or below one of them, or any exit child job.
#[test]
fn a_job_of_another_conversation_blocks_only_where_it_writes() {
    let roots = [PathBuf::from("/p/app")];
    let elsewhere = [job(2, 1, &["/p/other", "/d/scratch/b"], true)];
    assert_eq!(other_in(&elsewhere, conversation(1), &roots), None);
    for write_roots in [&["/p/app"][..], &["/p"], &["/p/app/src"]] {
        let running = [job(2, 3, write_roots, true)];
        assert_eq!(other_in(&running, conversation(1), &roots), Some(3), "{write_roots:?}");
    }
    let outside = [job(2, 4, &[], false)];
    assert_eq!(other_in(&outside, conversation(1), &roots), Some(4));
    assert_eq!(other_in(&outside, conversation(1), &[]), Some(4));
}

fn refs(turn: u128, roots: &[&str], started_undo: bool) -> efr_snapshot::TurnRefs {
    efr_snapshot::TurnRefs {
        turn: TurnId::from_uuid(uuid::Uuid::from_u128(turn)),
        roots: roots.iter().map(PathBuf::from).collect(),
        has_post: true,
        undone: false,
        home: false,
        started_undo: started_undo.then(|| "u1".to_owned()),
        auto: true,
        full: false,
    }
}

/// The roots that a job of another conversation must not write in: those of the named
/// turn and of an undo that stopped half way; with no named turn, or for a redo, those
/// of every turn, since the undo picks its turn itself.
#[test]
fn undo_roots_cover_each_turn_that_the_undo_may_act_on() {
    let turns =
        [refs(1, &["/p/new"], false), refs(2, &["/p/old"], false), refs(3, &["/p/open"], true)];
    let named = TurnId::from_uuid(uuid::Uuid::from_u128(2));
    let paths = |roots: &[&str]| roots.iter().map(PathBuf::from).collect::<Vec<_>>();
    assert_eq!(undo_roots(&turns, Some(named)), paths(&["/p/old", "/p/open"]));
    assert_eq!(undo_roots(&turns, None), paths(&["/p/new", "/p/old", "/p/open"]));
    let running = [job(2, 1, &["/p/old"], true)];
    // NOTE: the newest turn is not the one that the undo acts on when it names none:
    // the undo can pick an older turn, so the job blocks.
    assert_eq!(other_in(&running, conversation(1), &undo_roots(&turns, None)), Some(1));
    assert_eq!(other_in(&running, conversation(1), &undo_roots(&turns[..1], None)), None);
}

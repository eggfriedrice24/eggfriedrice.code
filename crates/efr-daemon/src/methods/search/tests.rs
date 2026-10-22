use efr_protocol::{ErrorCode, ErrorFrame, SearchKind, SearchLine, Seq};
use efr_store::StoreError;
use efr_store::search::FoundHit;
use efr_test_support::TestClock;
use jiff::Timestamp;
use pretty_assertions::assert_eq;
use tokio::sync::oneshot;

use super::{HARD_CAP, Ran, Run, SOFT_CAP, search_error, shown_title, wire_hit};
use crate::DaemonError;
use crate::methods::cursor;

/// A key of a form that the redaction recognises, with a body of 20 characters.
const KEY: &str = "sk-ant-AbCdEfGhIjKlMnOpQrSt";

#[test]
fn the_search_title_wins_over_the_projection() {
    let shown =
        shown_title(Some("deploy [***] now".to_owned()), Some(&format!("deploy {KEY} now")));
    assert_eq!(shown.as_deref(), Some("deploy [***] now"));
}

#[test]
fn a_projection_title_shows_with_its_secrets_replaced() {
    let shown = shown_title(None, Some(&format!("deploy {KEY} now")));
    assert_eq!(shown.as_deref(), Some("deploy [***] now"));
    assert_eq!(shown_title(None, None), None);
}

#[test]
fn a_cut_projection_title_loses_its_last_word() {
    // NOTE: the cut left 10 characters of the key's body, too few for the redaction
    // to recognise; the word goes with the cut.
    let cut = "run the deploy of the app with the key sk-ant-AbCdEfGhIj...";
    let shown = shown_title(None, Some(cut)).unwrap();
    assert_eq!(shown, "run the deploy of the app with the key ...");
    assert!(!shown.contains("AbCd"), "{shown}");
    let one_word =
        "sk-ant-AbCdEfGhIjAbCdEfGhIjAbCdEfGhIjAbCdEfGhIjAbCdEfGhIjAbCdEfGhIjAbCdEfGhIjA...";
    assert_eq!(shown_title(None, Some(one_word)).as_deref(), Some("..."));
}

#[test]
fn a_hit_opens_the_history_at_its_event() {
    let hit = FoundHit {
        kind: SearchKind::Command,
        seq: Seq::new(41),
        turn_id: None,
        at: Timestamp::UNIX_EPOCH,
        lines: vec![SearchLine { text: "rsync -a".to_owned(), ranges: Vec::new() }],
    };
    let wire = wire_hit(hit);
    // The page of the cursor holds the events before 42: it ends at the hit.
    assert_eq!(wire.history_cursor, cursor(Seq::new(42)));
    assert_eq!((wire.kind, wire.seq), (SearchKind::Command, Seq::new(41)));
}

/// The code and the message of `error` on the wire.
fn wire(error: DaemonError) -> (ErrorCode, String) {
    let body = ErrorFrame::from(error).error;
    (body.code, body.message)
}

#[test]
fn search_errors_keep_their_codes_and_show_no_text_of_the_store() {
    let cases = [
        (
            StoreError::QueryTooLong { chars: 300, limit: 256 },
            ErrorCode::Invalid,
            "the query is longer than 256 characters",
        ),
        (
            StoreError::QueryTooManyTerms { terms: 17, limit: 16 },
            ErrorCode::Invalid,
            "the query has more than 16 words",
        ),
        (StoreError::SearchBusy, ErrorCode::Busy, "every search lane is busy"),
        (StoreError::SearchStopped, ErrorCode::Cancelled, "the search was stopped"),
        (
            StoreError::TaskPanicked { task: "search lane" },
            ErrorCode::Internal,
            "the search failed in the store",
        ),
    ];
    for (error, code, message) in cases {
        assert_eq!(wire(search_error(error)), (code, message.to_owned()));
    }
}

#[tokio::test]
async fn a_search_that_ends_in_time_is_done() {
    let clock = TestClock::new();
    let mut run = Run::on(&clock, None);
    let ran = run.finish(async { Ok::<_, StoreError>(7) }).await.unwrap();
    assert!(matches!(ran, Ran::Done(7)), "{ran:?}");
}

#[tokio::test]
async fn the_soft_cap_ends_the_row_loop_and_the_search_still_answers() {
    let clock = TestClock::new();
    let mut run = Run::on(&clock, None);
    let stop = run.stop();
    let (go, wait) = oneshot::channel::<()>();
    let work = async move {
        wait.await.unwrap();
        Ok::<_, StoreError>("ranked what it saw")
    };
    let (ran, ()) = tokio::join!(run.finish(work), async {
        clock.advance(SOFT_CAP);
        // NOTE: `join!` may poll this future first again; a few turns of the task let
        // the search see its soft cap.
        for _ in 0..10 {
            if stop.is_stopped() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(stop.is_stopped(), "the row loop ends at the next row");
        assert!(!stop.is_hard(), "nothing is interrupted yet");
        go.send(()).unwrap();
    });
    assert!(matches!(ran, Ok(Ran::Done("ranked what it saw"))), "{ran:?}");
}

#[tokio::test]
async fn the_hard_cap_answers_without_the_work_and_interrupts_it() {
    let clock = TestClock::new();
    let mut run = Run::on(&clock, None);
    let stop = run.stop();
    run.started().now();
    let (ran, ()) =
        tokio::join!(run.finish(std::future::pending::<Result<u8, StoreError>>()), async {
            clock.advance(HARD_CAP);
        });
    assert!(matches!(ran, Ok(Ran::TimeCap)), "{ran:?}");
    assert!(stop.is_hard());
}

#[tokio::test]
async fn a_search_with_no_lane_at_the_hard_cap_is_busy() {
    let clock = TestClock::new();
    let mut run = Run::on(&clock, None);
    let (ran, ()) =
        tokio::join!(run.finish(std::future::pending::<Result<u8, StoreError>>()), async {
            clock.advance(HARD_CAP);
        });
    assert!(matches!(ran, Err(DaemonError::SearchBusy)), "{ran:?}");
}

#[test]
fn a_dropped_run_stops_its_search() {
    let clock = TestClock::new();
    let run = Run::on(&clock, None);
    let stop = run.stop();
    drop(run);
    assert!(stop.is_hard());
}

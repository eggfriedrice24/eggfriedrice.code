//! The body events of the turns that the window never rebuilds (`events::BODY_KINDS`):
//! a turn's start leaves them out of the page and never decodes them. The page holds
//! no event that the log does not hold, and reaches back as far. The window, the
//! request that the model gets, the context breakdown and every fact that a turn reads
//! from the page stay byte for byte the same as with the whole page: with any cache of
//! the actor, an empty one as after an idle trim or a restart included.

use std::collections::HashMap;

use efr_protocol::{EventEnvelope, TurnId};
use efr_store::conversations::TurnStatus;
use efr_store::events::BODY_KINDS;
use efr_test_support::{TestRng, TestStore};
use pretty_assertions::assert_eq;

use super::covered::{LIMITS, random_conversation};
use super::held::caches;
use super::{conversation, key};
use crate::compaction::with_window;
use crate::context::breakdown::{self, end_usage};
use crate::context::context_base;
use crate::history::{Bodies, Covered, HistoryLimits, Snapshot, bodiless, summary_cut};
use crate::testing::request;

/// Checks that the page of `thin`, a read that leaves out bodies, is the page of
/// `full`, a read of every event, without the body events of exactly the turns that
/// the window of `thin` never rebuilds, and that both start at the same event.
pub(super) fn assert_page_without_bodies(thin: &Snapshot, full: &Snapshot) {
    let without = bodiless(&thin.turns, &thin.saved, summary_cut(&thin.compactions));
    let body = |envelope: &EventEnvelope| {
        BODY_KINDS.contains(&envelope.event.kind())
            && envelope.event.turn_id().is_some_and(|turn| without.contains(&turn))
    };
    let expected: Vec<EventEnvelope> = full.page.iter().filter(|e| !body(e)).cloned().collect();
    assert_eq!(thin.page, expected, "the page leaves out only bodies that no one reads");
    assert_eq!(full.first, full.page.first().map(|envelope| envelope.seq));
    assert_eq!(thin.first, full.first, "the page reaches back as far");
}

/// The page facts that a turn's start, a manual compaction and the context breakdown
/// read: the previous directory, the hidden shell, the newest settings, the base of the
/// estimate, the last real count and the ends of the turns.
fn assert_same_facts(thin: &Snapshot, full: &Snapshot, what: &str) {
    for turn in &full.turns {
        assert_eq!(thin.previous_cwd(turn.id), full.previous_cwd(turn.id), "{what}");
    }
    assert_eq!(thin.agent_cwd(), full.agent_cwd(), "{what}");
    assert_eq!(thin.newest_settings(), full.newest_settings(), "{what}");
    for model in ["m", "n"] {
        assert_eq!(context_base(&thin.page, model), context_base(&full.page, model), "{what}");
    }
    assert_eq!(breakdown::counted(&thin.page), breakdown::counted(&full.page), "{what}");
    let ends = |snapshot: &Snapshot| -> Vec<EventEnvelope> {
        snapshot.page.iter().filter(|e| end_usage(&e.event).is_some()).cloned().collect()
    };
    assert_eq!(ends(thin), ends(full), "{what}");
}

/// For each of [`LIMITS`] and each cache of [`caches`], a read that leaves out the
/// bodies against a read of the whole page: the reads, the page facts, the windows,
/// the requests as JSON and the context breakdowns must be equal. Gives the count of
/// events that the reads left out.
async fn assert_same(
    store: &TestStore,
    current: Option<TurnId>,
    running: Option<TurnId>,
    rng: &TestRng,
) -> usize {
    let base = request(Vec::new());
    let mut left = 0;
    for (max_events, max_bytes) in LIMITS {
        let limits = HistoryLimits::new(max_events, max_bytes);
        let none = HashMap::new();
        let reference =
            Snapshot::read(store.readers(), conversation(), limits, &none).await.expect("read");
        for cache in caches(&reference, rng) {
            let mut thin = Snapshot::read(store.readers(), conversation(), limits, &cache)
                .await
                .expect("read");
            let mut full = Snapshot::read_with(
                store.readers(),
                conversation(),
                limits,
                Covered::Skip,
                Bodies::Keep,
                &cache,
            )
            .await
            .expect("read");
            let what = format!("{limits:?}, {} cached", cache.len());
            assert_page_without_bodies(&thin, &full);
            left += full.page.len() - thin.page.len();
            assert_eq!(thin.turns, full.turns, "{what}");
            assert_eq!(thin.older, full.older, "the same ranges are read, {what}");
            assert_eq!(thin.compactions, full.compactions, "{what}");
            assert_eq!(thin.user_events, full.user_events, "{what}");
            assert_eq!(thin.summary, full.summary, "{what}");
            assert_eq!(thin.saved, full.saved, "{what}");
            assert_same_facts(&thin, &full, &what);

            for view in [&mut thin, &mut full] {
                for turn in &mut view.turns {
                    if Some(turn.id) == running && turn.status == TurnStatus::Running {
                        turn.status = TurnStatus::Completed;
                    }
                }
            }
            let numbers: HashMap<TurnId, u32> = full
                .turns
                .iter()
                .filter(|turn| turn.started_at.is_some())
                .zip(1_u32..)
                .map(|(turn, number)| (turn.id, number))
                .collect();
            // NOTE: the window may take another cache than the read, as after a trim
            // between the two; a turn without bodies is always in `saved`.
            for window_cache in [&cache, &none] {
                for model in [key("replay", "m"), key("other", "m")] {
                    let window = |snapshot: &Snapshot| {
                        snapshot.window(current, window_cache, &model, limits, Some("fresh"))
                    };
                    let (new, old) = (window(&thin), window(&full));
                    assert_eq!(new, old, "{what}, {model:?}");
                    let json =
                        |window| serde_json::to_string(&with_window(&base, window)).expect("json");
                    assert_eq!(json(&new), json(&old), "the request, {what}, {model:?}");
                    assert_eq!(
                        breakdown::measure(&base, &new, &numbers),
                        breakdown::measure(&base, &old, &numbers),
                        "the breakdown, {what}, {model:?}"
                    );
                }
            }
        }
    }
    left
}

/// Random conversations (the ones of `covered.rs`: random bodies and ends, saved
/// messages for some turns, steers, every kind of compaction, the hidden shell, turn
/// settings and counts), each checked as a turn starts and as the context breakdown
/// reads it: the page without the bodies changes nothing that the model or the user
/// sees.
#[tokio::test]
async fn leaving_out_the_bodies_changes_no_window_of_random_conversations() {
    let rng = TestRng::new(6);
    let mut left = 0;
    for seed in 0..150 {
        let (store, running) = random_conversation(seed).await;
        left += assert_same(&store, running, None, &rng).await;
        left += assert_same(&store, None, running, &rng).await;
    }
    assert!(left > 1000, "the reads left out {left} events");
}

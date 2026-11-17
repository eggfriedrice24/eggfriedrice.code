//! The `status` and the `attention` of a conversation, and the predicate that says
//! which events can change them.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use efr_protocol::{ApprovalDecision, Attention, InputWait, Origin, UserAnswer};
use pretty_assertions::assert_eq;
use rusqlite::Connection;

use super::super::*;
use crate::testing::{self, question};
use crate::{Migrations, db, projection};

/// A migrated in-memory store and the sequence number of its next event.
struct Log {
    conn: Connection,
    seq: u64,
}

impl Log {
    fn new() -> Log {
        let mut conn = db::open_in_memory().unwrap();
        Migrations::new().migrate(&mut conn, None).unwrap();
        Log { conn, seq: 0 }
    }

    fn envelope(&mut self, event: Event) -> EventEnvelope {
        self.seq += 1;
        EventEnvelope {
            seq: Seq::new(self.seq),
            conversation_id: Some(testing::conversation(1)),
            at: sql::truncate_to_micros(testing::start()),
            event,
        }
    }

    fn apply(&mut self, event: Event) -> Result<(), StoreError> {
        let envelope = self.envelope(event);
        projection::apply(&self.conn, &envelope)
    }

    fn row(&self) -> (ConversationStatus, Option<Attention>) {
        let summary = get(&self.conn, testing::conversation(1)).unwrap().unwrap();
        (summary.status, summary.attention)
    }
}

fn approval(call: u64, local_only: bool) -> Event {
    Event::ApprovalRequested {
        turn_id: testing::turn(1),
        call_id: testing::call(call),
        summary: "sudo systemctl restart nginx".to_owned(),
        diff_preview: None,
        interactive: false,
        exit: None,
        expires_in_secs: None,
        local_only,
        reason: None,
    }
}

fn resolved(call: u64) -> Event {
    Event::ApprovalResolved {
        turn_id: testing::turn(1),
        call_id: testing::call(call),
        decision: ApprovalDecision::Allow,
        origin: Origin::Shell,
    }
}

fn input(call: u64, input: InputWait) -> Event {
    Event::ToolCallInputChanged {
        turn_id: testing::turn(1),
        call_id: testing::call(call),
        input,
        looks_secret: false,
    }
}

fn asked(n: u64) -> Event {
    Event::UserQuestionRequested {
        turn_id: testing::turn(1),
        call_id: testing::call(9),
        question_id: question(n),
        question: "Which database?".to_owned(),
        options: Vec::new(),
    }
}

fn answered(n: u64) -> Event {
    Event::UserQuestionAnswered {
        turn_id: testing::turn(1),
        question_id: question(n),
        answer: UserAnswer::Unanswered { away: true },
        origin: None,
    }
}

fn surface(n: u64) -> Event {
    Event::SurfaceQuestionRequested {
        turn_id: testing::turn(1),
        call_id: testing::call(8),
        question_id: question(n),
        changes: Vec::new(),
    }
}

/// The end `index` of [`testing::turn_ends`] of the turn `turn`: 0 completed, 1
/// failed, 2 interrupted, 3 cancelled.
fn end(turn: u64, index: usize) -> Event {
    testing::turn_ends(turn)[index].clone()
}

#[test]
fn status_and_attention_follow_what_waits() {
    use Attention::{Answer, Failed, Terminal};
    use ConversationStatus::{AwaitingApproval, Idle, Running};

    type Row = (ConversationStatus, Option<Attention>);
    let cases: Vec<(&str, Vec<Event>, Row)> = vec![
        ("nothing waits", vec![], (Running, None)),
        ("an approval", vec![approval(1, false)], (AwaitingApproval, None)),
        ("a local-only approval", vec![approval(1, true)], (AwaitingApproval, Some(Terminal))),
        (
            "a local-only and a shared approval",
            vec![approval(1, true), approval(2, false)],
            (AwaitingApproval, None),
        ),
        ("a resolved local-only approval", vec![approval(1, true), resolved(1)], (Running, None)),
        ("an input wait", vec![input(1, InputWait::Visible)], (Running, Some(Terminal))),
        ("a hidden input wait", vec![input(1, InputWait::Hidden)], (Running, Some(Terminal))),
        (
            "an input wait that ended",
            vec![input(1, InputWait::Visible), input(1, InputWait::None)],
            (Running, None),
        ),
        (
            "an input wait and an approval",
            vec![input(1, InputWait::Visible), approval(2, false)],
            (AwaitingApproval, None),
        ),
        (
            "an input wait and a local-only approval",
            vec![input(1, InputWait::Visible), approval(2, true)],
            (AwaitingApproval, Some(Terminal)),
        ),
        ("a quarantine question", vec![surface(1)], (AwaitingApproval, None)),
        (
            "an input wait and a quarantine question",
            vec![input(1, InputWait::Visible), surface(1)],
            (AwaitingApproval, None),
        ),
        ("a question of the model", vec![asked(1)], (Running, Some(Answer))),
        (
            "a question and a local-only approval",
            vec![asked(1), approval(2, true)],
            (AwaitingApproval, Some(Answer)),
        ),
        ("an answered question", vec![asked(1), answered(1)], (Running, None)),
        ("a completed turn", vec![end(1, 0)], (Idle, None)),
        ("a failed turn", vec![end(1, 1)], (Idle, Some(Failed))),
        ("an interrupted turn", vec![end(1, 2)], (Idle, None)),
        ("a cancelled turn", vec![end(1, 3)], (Idle, None)),
        (
            "a failed turn that waited",
            vec![asked(1), approval(2, true), end(1, 1)],
            (Idle, Some(Failed)),
        ),
        (
            "a failed turn and a queued one",
            vec![end(1, 1), testing::queued(2, "again")],
            (Idle, Some(Failed)),
        ),
        (
            "a failed turn and a running one",
            vec![end(1, 1), testing::queued(2, "again"), testing::started(2, "/")],
            (Running, None),
        ),
        (
            "a failed turn and a completed one",
            vec![end(1, 1), testing::queued(2, "again"), testing::started(2, "/"), end(2, 0)],
            (Idle, None),
        ),
        (
            "a failed turn and a withdrawn prompt",
            vec![
                end(1, 1),
                testing::queued(2, "again"),
                Event::PromptWithdrawn { turn_id: testing::turn(2), origin: Origin::Shell },
            ],
            (Idle, Some(Failed)),
        ),
    ];

    for (name, events, expected) in cases {
        let mut log = Log::new();
        for event in [testing::created(None), testing::queued(1, "x"), testing::started(1, "/")]
            .into_iter()
            .chain(events)
        {
            log.apply(event).unwrap();
        }
        assert_eq!(log.row(), expected, "{name}");
    }
}

#[test]
fn a_new_conversation_has_no_attention() {
    let mut log = Log::new();
    log.apply(testing::created(None)).unwrap();
    assert_eq!(log.row(), (ConversationStatus::Idle, None));
}

/// The event fixtures of the protocol: one file per kind of event, at least. A test of
/// efr-protocol fails when a kind has no fixture, so this set holds every kind.
fn event_fixtures() -> Vec<(String, Event)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../efr-protocol/fixtures/v1/events");
    let mut fixtures: Vec<(String, Event)> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
            let event = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            (name, event)
        })
        .collect();
    fixtures.sort_by(|a, b| a.0.cmp(&b.0));
    fixtures
}

/// The fixture of the kind `name`.
fn fixture(fixtures: &[(String, Event)], name: &str) -> Event {
    fixtures.iter().find(|(fixture, _)| fixture == name).unwrap().1.clone()
}

/// The `status` and `attention` of every conversation, as text.
fn status_rows(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT id, status, COALESCE(attention, '-') FROM conversations ORDER BY id")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(format!(
            "{} {} {}",
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// Every event that [`changes_status`] skips leaves the `status` and the `attention`
/// as a refresh after it would make them. Each fixture of the protocol is applied in
/// several states of a conversation, which the same fixtures make: none, created,
/// running, waiting in each way, and failed. Each fixture must apply in at least one
/// state, so no kind of event is left out.
#[test]
fn the_skipped_events_never_change_status_or_attention() {
    let fixtures = event_fixtures();
    let running = || vec![fixture(&fixtures, "prompt_queued"), fixture(&fixtures, "turn_started")];
    let with = |more: &[&str]| {
        let mut events = running();
        events.extend(more.iter().map(|name| fixture(&fixtures, name)));
        events
    };
    let states: Vec<(&str, Option<Vec<Event>>)> = vec![
        ("no conversation", None),
        ("created", Some(Vec::new())),
        ("running", Some(running())),
        ("local", Some(with(&["approval_requested_local_only", "tool_call_input_changed"]))),
        ("shared", Some(with(&["approval_requested", "user_question_requested"]))),
        ("surface", Some(with(&["surface_question_requested"]))),
        ("failed", Some(with(&["turn_failed"]))),
    ];

    let mut applied = BTreeSet::new();
    for (state, events) in &states {
        let mut log = Log::new();
        if let Some(events) = events {
            log.apply(testing::created(None)).unwrap();
            for event in events.iter().cloned() {
                log.apply(event).unwrap();
            }
        }
        for (name, event) in &fixtures {
            let envelope = log.envelope(event.clone());
            log.conn.execute_batch("SAVEPOINT probe").unwrap();
            let skipped = projection::apply(&log.conn, &envelope).map(|()| status_rows(&log.conn));
            log.conn.execute_batch("ROLLBACK TO probe").unwrap();
            let refreshed = projection::apply(&log.conn, &envelope)
                .and_then(|()| refresh_status(&log.conn, testing::conversation(1)))
                .map(|()| status_rows(&log.conn));
            log.conn.execute_batch("ROLLBACK TO probe; RELEASE probe").unwrap();
            if let (Ok(skipped), Ok(refreshed)) = (skipped, refreshed) {
                assert_eq!(skipped, refreshed, "{name} in the state {state}");
                applied.insert(name.as_str());
            }
        }
    }

    let all: BTreeSet<&str> = fixtures.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(applied, all);
    let kinds: BTreeSet<&str> = fixtures.iter().map(|(_, event)| event.kind()).collect();
    assert!(kinds.contains("user_question_answered") && kinds.len() > 60, "{kinds:?}");
}

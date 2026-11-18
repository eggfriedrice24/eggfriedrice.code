use efr_protocol::{HeldReason, InputWait, JobId, QuestionId, Seq, TurnId};
use pretty_assertions::assert_eq;

use super::*;

fn conversation() -> ConversationId {
    ConversationId::from_uuid(uuid::Uuid::from_u128(1))
}

fn turn() -> TurnId {
    TurnId::from_uuid(uuid::Uuid::from_u128(2))
}

fn call() -> CallId {
    CallId::from_uuid(uuid::Uuid::from_u128(3))
}

fn envelope(event: Event) -> EventEnvelope {
    EventEnvelope { seq: Seq::new(9), conversation_id: None, at: Timestamp::UNIX_EPOCH, event }
}

fn completed() -> Event {
    Event::TurnCompleted {
        turn_id: turn(),
        usage: None,
        changes: None,
        context: None,
        stop: None,
        undo: None,
    }
}

/// Rule 1: what a row is about, so the follower and a waiting turn name one question
/// alike, and which rows are questions.
#[test]
fn a_row_names_its_call_its_turn_or_its_event() {
    let wake = Wake { jobs: "j3".to_owned(), answer: None };
    let question = Event::UserQuestionRequested {
        turn_id: turn(),
        call_id: call(),
        question_id: QuestionId::from_uuid(uuid::Uuid::from_u128(4)),
        question: "Which branch?".to_owned(),
        options: Vec::new(),
    };
    let input = Event::ShellCommandInputChanged {
        call_id: call(),
        input: InputWait::Visible,
        contained: false,
        looks_secret: false,
        prompt: "Continue?".to_owned(),
    };
    let held = Event::JobNoticeHeld {
        job_id: JobId::nth(2),
        source_seq: Seq::new(7),
        reason: HeldReason::WakeLimit,
    };
    let waits = |what, question| What::Waits { what, question };
    let cases = [
        (question, None, Subject::Call(call()), waits(QUESTION_WAITS, true)),
        (input, None, Subject::Call(call()), waits("a command waits for input", false)),
        (completed(), Some(&wake), Subject::Turn(turn()), waits("job notice turn finished", false)),
        (
            held,
            None,
            Subject::Event(Seq::new(9)),
            waits("job notices wait for your next prompt", false),
        ),
    ];
    for (event, wake, subject, what) in cases {
        let row = waiting(conversation(), &envelope(event), wake);
        assert_eq!(row, Some(Row { conversation: conversation(), subject, what }));
    }
    assert_eq!(waiting(conversation(), &envelope(completed()), None), None, "a prompt's end");
    let from_turn = Row::question(conversation(), call());
    assert_eq!(from_turn.subject, Subject::Call(call()), "the same question as the follower's");
}

/// The body follows the privacy rule of the desktop notice; a question lasts the away
/// wait, every other push an hour.
#[test]
fn the_body_names_the_place_and_a_question_lasts_the_away_wait() {
    let mut settings = Settings::default();
    settings.presence.away_wait_secs = 120;
    let question = Row::question(conversation(), call());
    let ended = Row {
        conversation: conversation(),
        subject: Subject::Turn(turn()),
        what: What::Ended { took: Duration::from_secs(252), failed: false },
    };

    assert_eq!(question.body("app"), "app: a question waits");
    assert_eq!(ended.body("app"), "app: done in 4m 12s");
    assert_eq!(question.ttl(&settings), Duration::from_secs(120));
    assert_eq!(ended.ttl(&settings), NOTICE_TTL);
}

#[test]
fn a_full_queue_drops_the_newest_row() {
    let (queue, mut rows) = PushQueue::channel();
    for _ in 0..=QUEUE_CAPACITY {
        queue.hand_over(Row::question(conversation(), call()));
    }

    let mut taken = 0;
    while rows.try_recv().is_ok() {
        taken += 1;
    }
    assert_eq!(taken, QUEUE_CAPACITY);
}

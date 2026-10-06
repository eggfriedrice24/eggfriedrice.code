use efr_protocol::{QuestionId, TurnId};
use efr_stdx::id::uuid_v7;
use efr_test_support::{TestClock, TestRng};

use super::Questions;

fn ids() -> (TurnId, QuestionId) {
    let clock = TestClock::new();
    (
        TurnId::from_uuid(uuid_v7(&clock, &TestRng::new(1))),
        QuestionId::from_uuid(uuid_v7(&clock, &TestRng::new(2))),
    )
}

#[tokio::test]
async fn a_taken_question_hands_the_answer_to_the_waiting_turn() {
    let (turn_id, question_id) = ids();
    let questions = Questions::default();
    let answer = questions.park(turn_id, question_id);
    assert_eq!(questions.parked(), vec![question_id]);

    let taken = questions.take(question_id).expect("the question waits");
    assert_eq!(taken.turn_id, turn_id);
    assert!(taken.answer(true));
    assert_eq!(answer.await, Ok(true));
    assert!(questions.parked().is_empty());
}

#[test]
fn a_question_is_taken_or_withdrawn_once() {
    let (turn_id, question_id) = ids();
    let questions = Questions::default();
    let _answer = questions.park(turn_id, question_id);

    assert!(questions.take(question_id).is_some());
    // NOTE: the turn that expires after the actor took the question must wait for the
    // answer, so it records no second one.
    assert!(!questions.withdraw(question_id));
    assert!(questions.take(question_id).is_none());
}

#[test]
fn a_withdrawn_question_takes_no_answer() {
    let (turn_id, question_id) = ids();
    let questions = Questions::default();
    let _answer = questions.park(turn_id, question_id);

    assert!(questions.withdraw(question_id));
    assert!(questions.take(question_id).is_none());
}

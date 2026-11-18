use super::*;

fn conversation(n: u128) -> ConversationId {
    ConversationId::from_uuid(uuid::Uuid::from_u128(n))
}

fn call(n: u128) -> Subject {
    Subject::Call(CallId::from_uuid(uuid::Uuid::from_u128(n)))
}

#[test]
fn one_subject_of_one_conversation_pushes_once() {
    let mut repeats = Repeats::default();

    assert!(repeats.first(conversation(1), call(1)));
    assert!(!repeats.first(conversation(1), call(1)), "the second hand-over of a question");
    assert!(repeats.first(conversation(2), call(1)));
    assert!(repeats.first(conversation(1), call(2)));
    assert!(repeats.first(conversation(1), Subject::Event(Seq::new(1))));
}

#[test]
fn the_oldest_push_is_forgotten_after_1024_others() {
    let mut repeats = Repeats::default();
    let count = u128::try_from(MAX_REMEMBERED).unwrap();
    for n in 0..=count {
        assert!(repeats.first(conversation(1), call(n)));
    }

    assert_eq!(repeats.seen.len(), MAX_REMEMBERED);
    assert!(repeats.first(conversation(1), call(0)), "call 0 went first");
    assert!(!repeats.first(conversation(1), call(count)));
}

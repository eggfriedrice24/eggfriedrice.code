use efr_protocol::{
    ChainStep, ConversationId, Judgement, Risk, TurnId, UserAuthorization, Verdict,
};
use efr_stdx::id::uuid_v7;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;

use super::{Cached, MAX_CONVERSATIONS, Memory};

fn conversation(seed: u64) -> ConversationId {
    ConversationId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(seed)))
}

fn turn(seed: u64) -> TurnId {
    TurnId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(1_000 + seed)))
}

fn cached() -> Cached {
    Cached {
        judgement: Judgement {
            verdict: Verdict::Deny,
            risk: Risk::High,
            user_authorization: UserAuthorization::None,
            category: "exfiltration".to_owned(),
            rationale: "nobody asked for it".to_owned(),
        },
        model: "gpt-6-luna".to_owned(),
        chain_step: ChainStep::ClassifierModel,
    }
}

#[test]
fn a_verdict_lasts_one_turn() {
    let memory = Memory::default();
    let (one, two) = (conversation(1), conversation(2));
    memory.keep_verdict(one, turn(1), "key".to_owned(), cached());
    assert_eq!(memory.verdict(one, turn(1), "key"), Some(cached()));
    assert_eq!(memory.verdict(one, turn(1), "other"), None);
    assert_eq!(memory.verdict(two, turn(1), "key"), None);
    // The next turn drops the verdicts of the turn before, also for a later ask of it.
    assert_eq!(memory.verdict(one, turn(2), "key"), None);
    assert_eq!(memory.verdict(one, turn(1), "key"), None);
}

#[test]
fn a_skipped_model_stays_skipped_in_its_conversation_only() {
    let memory = Memory::default();
    let (one, two) = (conversation(1), conversation(2));
    memory.skip(one, "gpt-6-luna", "refused");
    assert_eq!(memory.skipped(one, "gpt-6-luna"), Some("refused"));
    assert_eq!(memory.skipped(one, "gpt-6.1-sol"), None);
    assert_eq!(memory.skipped(two, "gpt-6-luna"), None);
    // A new turn keeps the skipped models.
    memory.verdict(one, turn(2), "key");
    assert_eq!(memory.skipped(one, "gpt-6-luna"), Some("refused"));
}

#[test]
fn a_refused_format_lasts_for_every_conversation() {
    let memory = Memory::default();
    assert!(memory.takes_format("gpt-6-luna"));
    memory.refuses_format("gpt-6-luna");
    assert!(!memory.takes_format("gpt-6-luna"));
    assert!(memory.takes_format("gpt-6.1-sol"));
}

#[test]
fn a_full_memory_drops_the_conversation_that_asked_least_recently() {
    let memory = Memory::default();
    for seed in 1..=u64::try_from(MAX_CONVERSATIONS).unwrap() {
        memory.skip(conversation(seed), "gpt-6-luna", "missing");
    }
    // The first conversation asks again, so the second is now the oldest.
    assert_eq!(memory.verdict(conversation(1), turn(1), "key"), None);
    memory.skip(conversation(10_000), "gpt-6-luna", "missing");
    assert_eq!(memory.skipped(conversation(1), "gpt-6-luna"), Some("missing"));
    assert_eq!(memory.skipped(conversation(2), "gpt-6-luna"), None);
    assert_eq!(memory.skipped(conversation(10_000), "gpt-6-luna"), Some("missing"));
}

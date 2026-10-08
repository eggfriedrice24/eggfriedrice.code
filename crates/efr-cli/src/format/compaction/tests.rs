use efr_protocol::{Compaction, CompactionTrigger, Usage};
use pretty_assertions::assert_eq;

use super::compacted;
use crate::testing::{conversation, turn};

fn compaction(trigger: CompactionTrigger) -> Compaction {
    Compaction {
        compaction_id: conversation().to_string().parse().expect("an id"),
        turn_id: Some(turn()),
        trigger,
        focus: None,
        model: "gpt-5.5".to_owned(),
        window: 272_000,
        limit: 206_720,
        tokens_before: 231_000,
        tokens_after: 24_000,
        through_turn: turn(),
        through_message: Some(6),
        kept_turns: 3,
        pruned_outputs: 0,
        pruned_tokens: 0,
        summary: Some("## Task and state\nFree space on /var.".to_owned()),
        usage: Some(Usage::new(205_000, 3_200)),
    }
}

#[test]
fn an_auto_compaction_names_the_tokens_the_tail_and_the_summary() {
    assert_eq!(
        compacted(&compaction(CompactionTrigger::Auto)),
        "context compacted (auto): 231k -> 24.0k tokens, kept 3 turns, summary 3.2k"
    );
}

#[test]
fn a_manual_compaction_says_efr_compact_and_counts_a_summary_without_usage() {
    let mut manual = compaction(CompactionTrigger::Manual);
    manual.turn_id = None;
    manual.kept_turns = 1;
    manual.usage = None;
    manual.summary = Some("x".repeat(4_000));

    assert_eq!(
        compacted(&manual),
        "context compacted (efr compact): 231k -> 24.0k tokens, kept 1 turn, summary 1.0k"
    );
}

#[test]
fn a_pruning_alone_counts_the_outputs() {
    let mut pruned = compaction(CompactionTrigger::Auto);
    pruned.summary = None;
    pruned.usage = None;
    pruned.pruned_outputs = 12;
    pruned.tokens_after = 140_000;

    assert_eq!(
        compacted(&pruned),
        "context compacted (auto): 231k -> 140k tokens, kept 3 turns, pruned 12 outputs"
    );
}

#[test]
fn an_overflow_and_a_miss_say_that_the_context_is_full() {
    let mut overflow = compaction(CompactionTrigger::Overflow);
    overflow.tokens_before = 281_000;
    assert_eq!(
        compacted(&overflow),
        "context full: the request was 281k of 272k tokens; compacted and retried"
    );

    let mut miss = compaction(CompactionTrigger::Auto);
    miss.tokens_after = 240_000;
    assert_eq!(
        compacted(&miss),
        "context full: compaction did not free enough room (still 240k); run ,compact or efr new"
    );
}

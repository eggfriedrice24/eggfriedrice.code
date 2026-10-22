use std::collections::HashSet;

use pretty_assertions::assert_eq;

use super::*;

fn kind_counts(events: &[Event]) -> (usize, usize, usize, usize) {
    let count = |kind: &str| events.iter().filter(|event| event.kind() == kind).count();
    (
        count("prompt_queued"),
        count("assistant_message_completed"),
        count("tool_call_started"),
        count("turn_completed"),
    )
}

#[test]
fn the_sizes_have_their_turns() {
    assert_eq!(CorpusSize::S.turns(), 1_000);
    assert_eq!(CorpusSize::L.turns(), 20_000);
    assert_eq!(CorpusSize::Xl.turns(), 50_000);
    assert_eq!(CorpusSize::Xl.name(), "xl");
}

#[test]
fn corpus_s_has_its_conversations_and_turn_shape() {
    let conversations: Vec<CorpusConversation> = SearchCorpus::new(CorpusSize::S, 7).collect();

    assert_eq!(conversations.len(), 100);
    let ids: HashSet<ConversationId> = conversations.iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), 100);
    let mut long_prompts = 0;
    for conversation in &conversations {
        assert_eq!(conversation.turns.len(), 10);
        assert_eq!(conversation.turns[0][0].kind(), "conversation_created");
        for turn in &conversation.turns {
            assert_eq!(kind_counts(turn), (1, 3, 8, 1));
            for event in turn {
                if let Event::PromptQueued { text, .. } = event
                    && text.len() >= 100_000
                {
                    long_prompts += 1;
                }
            }
        }
    }
    // One prompt of about 100 KB per 1,000 turns.
    assert_eq!(long_prompts, 1);
}

#[test]
fn the_same_seed_gives_the_same_corpus() {
    let first: Vec<Vec<Vec<Event>>> =
        SearchCorpus::new(CorpusSize::S, 3).take(3).map(|c| c.turns).collect();
    let again: Vec<Vec<Vec<Event>>> =
        SearchCorpus::new(CorpusSize::S, 3).take(3).map(|c| c.turns).collect();
    let other: Vec<Vec<Vec<Event>>> =
        SearchCorpus::new(CorpusSize::S, 4).take(3).map(|c| c.turns).collect();

    assert_eq!(first, again);
    assert_ne!(first, other);
}

#[test]
fn the_first_values_stay_the_same() {
    // A changed sequence must come with a new CORPUS_VERSION.
    let mut corpus = SearchCorpus::new(CorpusSize::S, 1);
    let words: Vec<&str> = (0..3).map(|rank| corpus.word(rank)).collect();
    assert_eq!(words, ["natobola", "kubupai", "kipu"]);
    let first = corpus.next().unwrap();
    assert_eq!(first.id.to_string(), "01a106c9-0600-773c-80c5-003d650c8b3c");
    assert_eq!(CORPUS_VERSION, 1);
}

#[test]
fn the_vocabulary_holds_distinct_words_and_zipf_favours_the_first() {
    let corpus = SearchCorpus::new(CorpusSize::S, 1);
    let words: HashSet<&str> = (0..VOCABULARY).map(|rank| corpus.word(rank)).collect();
    assert_eq!(words.len(), VOCABULARY);

    let draws: Vec<&str> = (0..10_000).map(|_| corpus.zipf_word()).collect();
    let top = draws.iter().filter(|word| **word == corpus.word(0)).count();
    let tail = draws.iter().filter(|word| **word == corpus.word(4_000)).count();
    // Rank 1 takes about 1 / H(5000), about 11%, of the draws.
    assert!((900..1_400).contains(&top), "{top}");
    assert!(tail < 10, "{tail}");
}

#[test]
fn chunks_end_on_character_boundaries() {
    assert_eq!(chunks("abcdef", 4), ["abcd", "ef"]);
    assert_eq!(chunks("aéb", 2), ["aé", "b"]);
    assert_eq!(chunks("", 3), Vec::<&str>::new());
}

use std::collections::BTreeSet;

use pretty_assertions::assert_eq;

use super::{doomed_turns, mirror_key, turn_key, undo_key};

fn key(conversation: &str, turn: &str) -> (String, String) {
    (conversation.to_owned(), turn.to_owned())
}

#[test]
fn the_newest_turns_of_each_conversation_stay() {
    let seen = [key("a", "1"), key("a", "2"), key("a", "3"), key("b", "1"), key("a", "2")];
    let doomed = doomed_turns(seen.iter(), 2);
    assert_eq!(doomed, BTreeSet::from([key("a", "1")]));
    assert_eq!(doomed_turns(seen.iter(), 0).len(), 4);
}

#[test]
fn refs_name_their_turn_or_their_undo() {
    assert_eq!(turn_key("refs/efr/c/t/pre"), Some(key("c", "t")));
    assert_eq!(turn_key("refs/efr/c/t/home/3"), Some(key("c", "t")));
    assert_eq!(turn_key("refs/efr-mirror/c/t/heads/main"), Some(key("c", "t")));
    assert_eq!(mirror_key("refs/efr/c/t/pre"), None);
    assert_eq!(turn_key("refs/efr-undo/c/u/before"), None);
    assert_eq!(
        undo_key("refs/efr-undo/c/u/mirror/heads/main"),
        Some((key("c", "u"), "mirror/heads/main"))
    );
    assert_eq!(undo_key("refs/efr/c/t/pre"), None);
}

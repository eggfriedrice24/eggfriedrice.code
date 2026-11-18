use efr_protocol::ConversationId;
use efr_provider::EncodedBytes;
use uuid::Uuid;

use super::Cache;

fn conversation(n: u128) -> ConversationId {
    ConversationId::from_uuid(Uuid::from_u128(n))
}

fn text(bytes: usize) -> EncodedBytes {
    EncodedBytes::new("A".repeat(bytes))
}

#[test]
fn an_entry_comes_back_by_conversation_and_hash() {
    let cache = Cache::new(100);
    cache.insert(conversation(1), "aa", text(10));
    assert_eq!(cache.get(conversation(1), "aa"), Some(text(10)));
    assert_eq!(cache.get(conversation(2), "aa"), None, "another conversation");
    assert_eq!(cache.get(conversation(1), "bb"), None);
}

#[test]
fn the_oldest_entries_go_first_when_a_new_one_does_not_fit() {
    let cache = Cache::new(100);
    cache.insert(conversation(1), "a1", text(40));
    cache.insert(conversation(2), "a2", text(40));
    cache.insert(conversation(1), "a3", text(40));
    assert_eq!(cache.get(conversation(1), "a1"), None);
    assert!(cache.get(conversation(2), "a2").is_some());
    assert!(cache.get(conversation(1), "a3").is_some());
}

#[test]
fn the_cache_never_keeps_an_entry_larger_than_itself() {
    let cache = Cache::new(100);
    cache.insert(conversation(1), "small", text(60));
    cache.insert(conversation(1), "huge", text(101));
    assert_eq!(cache.get(conversation(1), "huge"), None);
    assert!(cache.get(conversation(1), "small").is_some(), "nothing went for it");
}

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

#[test]
fn a_removed_entry_gives_its_room_back() {
    let cache = Cache::new(100);
    cache.insert(conversation(1), "a1", text(60));
    cache.insert(conversation(2), "a2", text(30));
    cache.remove(conversation(1), "a1");
    assert_eq!(cache.get(conversation(1), "a1"), None);
    cache.insert(conversation(1), "a3", text(70));
    assert!(cache.get(conversation(2), "a2").is_some(), "the room of a1 was free");
    assert!(cache.get(conversation(1), "a3").is_some());
}

#[test]
fn the_delete_of_a_conversation_removes_its_entries_only() {
    let cache = Cache::new(100);
    cache.insert(conversation(1), "a1", text(30));
    cache.insert(conversation(1), "a2", text(30));
    cache.insert(conversation(2), "b1", text(30));
    cache.remove_conversation(conversation(1));
    assert_eq!((cache.get(conversation(1), "a1"), cache.get(conversation(1), "a2")), (None, None));
    cache.insert(conversation(2), "b2", text(70));
    assert!(cache.get(conversation(2), "b1").is_some(), "the room of conversation 1 was free");
}

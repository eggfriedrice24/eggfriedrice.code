use pretty_assertions::assert_eq;

use super::{ObjectId, blob_id, link_id};

#[test]
fn blob_ids_equal_the_ids_that_git_gives() {
    // NOTE: `printf '' | git hash-object --stdin` and `printf 'hello\n' | git
    // hash-object --stdin`.
    assert_eq!(blob_id(b"").as_str(), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    assert_eq!(blob_id(b"hello\n").as_str(), "ce013625030ba8dba906f756967f9e9ca394464a");
    assert_eq!(link_id(b"hello\n"), blob_id(b"hello\n"));
}

#[test]
fn an_object_id_has_forty_hex_characters() {
    let id = ObjectId::parse("CE013625030BA8DBA906F756967F9E9CA394464A\n").unwrap();
    assert_eq!(id.to_string(), "ce013625030ba8dba906f756967f9e9ca394464a");
    assert_eq!(ObjectId::parse("ce01"), None);
    assert_eq!(ObjectId::parse(&"g".repeat(40)), None);
}

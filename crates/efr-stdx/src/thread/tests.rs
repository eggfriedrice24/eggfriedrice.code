use pretty_assertions::assert_eq;

use super::spawn_named;
use crate::StdxError;

#[test]
fn thread_runs_under_its_name() {
    let handle = spawn_named("screen-conversation-0a1b", 256 * 1024, || {
        std::thread::current().name().map(str::to_owned)
    })
    .unwrap();
    assert_eq!(handle.join().unwrap().as_deref(), Some("screen-conversation-0a1b"));
}

#[test]
fn thread_returns_its_result() {
    let handle = spawn_named("sum", 256 * 1024, || (1..=10).sum::<u32>()).unwrap();
    assert_eq!(handle.join().unwrap(), 55);
}

#[test]
fn name_with_a_nul_byte_is_an_error() {
    let err = spawn_named("bad\0name", 256 * 1024, || ()).unwrap_err();
    match err {
        StdxError::InvalidThreadName { name } => assert_eq!(name, "bad\0name"),
        other => panic!("unexpected error {other:?}"),
    }
}

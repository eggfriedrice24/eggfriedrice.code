use std::error::Error as _;

use efr_stdx::StdxError;
use pretty_assertions::assert_eq;

use super::ScreenError;

#[test]
fn closed_names_the_screen() {
    let err = ScreenError::Closed { name: "screen-0a1b".to_owned() };
    assert_eq!(err.to_string(), "the screen screen-0a1b has stopped");
    assert!(err.source().is_none());
}

#[test]
fn spawn_keeps_the_thread_error_as_its_source() {
    let err = ScreenError::Spawn {
        name: "bad\0name".to_owned(),
        source: StdxError::InvalidThreadName { name: "bad\0name".to_owned() },
    };
    assert_eq!(err.to_string(), "could not start the screen thread bad\0name");
    let source = err.source().map(ToString::to_string);
    assert_eq!(source.as_deref(), Some("the thread name \"bad\\0name\" holds a NUL byte"));
}

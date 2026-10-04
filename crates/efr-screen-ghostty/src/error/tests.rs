use std::error::Error as _;

use pretty_assertions::assert_eq;

use super::GhosttyError;

#[test]
fn create_names_the_size_and_keeps_the_libghostty_error() {
    let err = GhosttyError::Create { cols: 0, rows: 3, source: libghostty_vt::Error::InvalidValue };
    assert_eq!(err.to_string(), "could not create a 0x3 ghostty terminal");
    assert_eq!(err.source().map(ToString::to_string).as_deref(), Some("invalid value"));
}

#[test]
fn messages_leave_the_source_text_out() {
    let source = libghostty_vt::Error::OutOfMemory;
    let errors = [GhosttyError::Configure { source }];
    for err in errors {
        assert!(!err.to_string().contains("out of memory"), "{err}");
        assert_eq!(err.source().map(ToString::to_string).as_deref(), Some("out of memory"));
    }
}

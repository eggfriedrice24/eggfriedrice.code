use std::error::Error as _;

use pretty_assertions::assert_eq;
use rusqlite::ffi;

use super::*;

fn sqlite_failure(code: i32) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(ffi::Error::new(code), None)
}

#[test]
fn busy_and_locked_become_busy() {
    assert!(matches!(StoreError::from(sqlite_failure(ffi::SQLITE_BUSY)), StoreError::Busy { .. }));
    assert!(matches!(
        StoreError::from(sqlite_failure(ffi::SQLITE_LOCKED)),
        StoreError::Busy { .. }
    ));
}

#[test]
fn other_sqlite_errors_stay_sqlite() {
    assert!(matches!(
        StoreError::from(sqlite_failure(ffi::SQLITE_CONSTRAINT)),
        StoreError::Sqlite { .. }
    ));
    assert!(matches!(
        StoreError::from(rusqlite::Error::QueryReturnedNoRows),
        StoreError::Sqlite { .. }
    ));
}

#[test]
fn messages_leave_the_cause_to_the_source_chain() {
    let error = StoreError::from(sqlite_failure(ffi::SQLITE_CONSTRAINT));
    assert_eq!(error.to_string(), "a database statement failed");
    assert!(error.source().is_some());
}

#[test]
fn the_error_stays_small() {
    // Every fallible function returns this type; clippy warns above 128 bytes.
    assert!(size_of::<StoreError>() <= 128);
}

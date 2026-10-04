//! Conversions between Rust values and SQLite columns, kept in one place so that every
//! table stores sequence numbers, times, ids and JSON the same way.
//!
//! Times are `INTEGER` microseconds since the Unix epoch. Sequence numbers are
//! `INTEGER`; SQLite integers are signed, and a sequence number never comes near
//! `i64::MAX`.

use std::str::FromStr;

use efr_protocol::Seq;
use jiff::Timestamp;
use serde::Serialize;

use crate::StoreError;

/// A sequence number as a column value. A bound from a client may exceed `i64::MAX`;
/// it saturates, which keeps "after" and "before" comparisons right.
pub(crate) fn seq(seq: Seq) -> i64 {
    i64::try_from(seq.get()).unwrap_or(i64::MAX)
}

/// A sequence number read from a column.
pub(crate) fn to_seq(
    value: i64,
    table: &'static str,
    column: &'static str,
) -> Result<Seq, StoreError> {
    u64::try_from(value).map(Seq::new).map_err(|source| decode_error(table, column, source))
}

/// A time as a column value.
pub(crate) fn micros(at: Timestamp) -> i64 {
    at.as_microsecond()
}

/// A time read from a column.
pub(crate) fn to_timestamp(
    value: i64,
    table: &'static str,
    column: &'static str,
) -> Result<Timestamp, StoreError> {
    Timestamp::from_microsecond(value).map_err(|source| decode_error(table, column, source))
}

/// `at` without its sub-microsecond part, so that a value handed out before it is
/// stored equals the value read back.
pub(crate) fn truncate_to_micros(at: Timestamp) -> Timestamp {
    Timestamp::from_microsecond(at.as_microsecond()).unwrap_or(at)
}

/// An id or other value parsed from a text column.
pub(crate) fn parse<T>(
    text: &str,
    table: &'static str,
    column: &'static str,
) -> Result<T, StoreError>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    text.parse().map_err(|source| decode_error(table, column, source))
}

/// `value` as JSON text for a column.
pub(crate) fn to_json<T: Serialize>(value: &T, what: &'static str) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|source| StoreError::Encode { what, source })
}

pub(crate) fn decode_error(
    table: &'static str,
    column: &'static str,
    source: impl std::error::Error + Send + Sync + 'static,
) -> StoreError {
    StoreError::DecodeColumn { table, column, source: Box::new(source) }
}

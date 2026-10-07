//! Places in the config file: byte offsets to lines and columns, and the dotted key
//! that holds an offset or that a key path names.
//!
//! The parser reports a byte span; the user needs a line, a column and the key. The
//! spans of every key and value come from `toml_edit`'s parsed document.

use std::ops::Range;

use toml_edit::{Document, Item, Table, TableLike, Value};

use crate::Location;

/// The line and column of byte `offset` in `text`, both counted from 1. The column
/// counts characters, not bytes.
pub(crate) fn location(text: &str, offset: usize) -> Location {
    let offset = offset.min(text.len());
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |at| at + 1);
    let column = before.get(line_start..).map_or(0, |part| part.chars().count()) + 1;
    Location { line: saturate(line), column: saturate(column) }
}

fn saturate(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The deepest dotted key in `document` whose key or value holds byte `offset`, such
/// as `shell.idle_minutes` or `permissions.rules[2].effect`.
pub(crate) fn key_at(document: &Document<String>, offset: usize) -> Option<String> {
    let mut path = Vec::new();
    in_table(document.as_table(), offset, &mut path).then(|| join(&path))
}

/// The span of the value of the dotted key `key`, such as `shell.login`, or of its table
/// when the value has no span of its own.
pub(crate) fn span_of(document: &Document<String>, key: &str) -> Option<Range<usize>> {
    let mut table: &dyn TableLike = document.as_table();
    let mut parts = key.split('.').peekable();
    while let Some(part) = parts.next() {
        let item = table.get(part)?;
        if parts.peek().is_none() {
            return item.span().or_else(|| item.as_table().and_then(Table::span));
        }
        table = item.as_table_like()?;
    }
    None
}

/// The span of `permissions.rules[index]`, written as a `[[permissions.rules]]` table or
/// as an element of an inline array.
pub(crate) fn span_of_rule(document: &Document<String>, index: usize) -> Option<Range<usize>> {
    let rules = document.as_table().get("permissions")?.as_table()?.get("rules")?;
    match rules {
        Item::ArrayOfTables(tables) => tables.get(index)?.span(),
        Item::Value(Value::Array(array)) => array.get(index)?.span(),
        _ => None,
    }
}

fn join(path: &[String]) -> String {
    let mut out = String::new();
    for part in path {
        if part.starts_with('[') {
            out.push_str(part);
        } else {
            if !out.is_empty() {
                out.push('.');
            }
            out.push_str(part);
        }
    }
    out
}

fn holds(span: Option<Range<usize>>, offset: usize) -> bool {
    span.is_some_and(|span| span.start <= offset && offset < span.end.max(span.start + 1))
}

/// True when an entry of `table` holds `offset`; `path` then ends with its key path.
fn in_table(table: &Table, offset: usize, path: &mut Vec<String>) -> bool {
    for (key, item) in table.iter() {
        path.push(key.to_owned());
        let key_span = table.key(key).and_then(|key| key.span());
        if in_item(item, offset, path) || holds(key_span, offset) {
            return true;
        }
        path.pop();
    }
    false
}

fn in_item(item: &Item, offset: usize, path: &mut Vec<String>) -> bool {
    match item {
        Item::Table(table) => in_table(table, offset, path) || holds(table.span(), offset),
        Item::ArrayOfTables(tables) => {
            for (index, table) in tables.iter().enumerate() {
                path.push(format!("[{index}]"));
                if in_table(table, offset, path) || holds(table.span(), offset) {
                    return true;
                }
                path.pop();
            }
            false
        }
        Item::Value(value) => in_value(value, offset, path),
        Item::None => false,
    }
}

fn in_value(value: &Value, offset: usize, path: &mut Vec<String>) -> bool {
    match value {
        Value::InlineTable(table) => {
            for (key, inner) in table.iter() {
                path.push(key.to_owned());
                if in_value(inner, offset, path) {
                    return true;
                }
                path.pop();
            }
        }
        Value::Array(array) => {
            for (index, inner) in array.iter().enumerate() {
                path.push(format!("[{index}]"));
                if in_value(inner, offset, path) {
                    return true;
                }
                path.pop();
            }
        }
        _ => {}
    }
    holds(value.span(), offset)
}

#[cfg(test)]
mod tests;

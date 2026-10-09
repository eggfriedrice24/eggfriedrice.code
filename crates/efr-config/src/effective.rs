//! The effective view: every key with its value and where the value came from.

use std::fmt::{self, Write as _};

use efr_stdx::env::Var;
use toml_edit::{DocumentMut, Item, Value};

use crate::{Settings, keys};

/// A string value longer than this is shown as its length, so a system prompt does not
/// fill the screen.
const LONG_STRING: usize = 200;

/// Where a value came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Source {
    /// The built-in default.
    #[default]
    Default,
    /// The config file.
    File,
    /// An environment variable.
    Env(Var),
    /// A flag, such as `--log`.
    Flag(&'static str),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Default => f.write_str("default"),
            Source::File => f.write_str("file"),
            Source::Env(var) => write!(f, "env {var}"),
            Source::Flag(flag) => write!(f, "flag {flag}"),
        }
    }
}

/// One key of the effective view.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Entry {
    /// The dotted key, such as `shell.idle_minutes`.
    pub key: String,
    /// The value as inline TOML, `(unset)` for a key without a value, `(built in)` for
    /// the model list, or `<N bytes>` for a long string.
    pub value: String,
    /// Where the value came from.
    pub source: Source,
}

impl Settings {
    /// Every key of the file with its effective value and its source, in the order of
    /// [`keys`](crate::keys).
    pub fn entries(&self) -> Vec<Entry> {
        let document = toml_edit::ser::to_document(self).ok();
        keys()
            .into_iter()
            .map(|key| {
                let value = document
                    .as_ref()
                    .and_then(|document| lookup(document, &key))
                    .map_or_else(|| unset(&key).to_owned(), show);
                let source = self.source(&key);
                Entry { key, value, source }
            })
            .collect()
    }

    /// Every value with its source, one `key = value  # source` line each after a
    /// comment with the file's path, for `efrd --print-config`.
    pub fn effective(&self) -> String {
        let mut out = format!("# {}\n", self.path.display());
        for Entry { key, value, source } in self.entries() {
            // Writing to a String cannot fail.
            let _ = writeln!(out, "{key} = {value}  # {source}");
        }
        out
    }
}

/// What a key without a value shows.
fn unset(key: &str) -> &'static str {
    match key {
        "openai.models" => "(built in)",
        "anthropic.models" => "(from the API)",
        _ => "(unset)",
    }
}

/// The item of the dotted `key` in `document`, when it has one.
pub(crate) fn lookup<'a>(document: &'a DocumentMut, key: &str) -> Option<&'a Item> {
    let mut item = document.as_item();
    for part in key.split('.') {
        item = item.get(part)?;
    }
    Some(item)
}

/// `item` as one line of inline TOML, whole, so two values compare by their text.
pub(crate) fn exact(item: &Item) -> String {
    match as_value(item.clone()) {
        Some(value) => inline(value),
        None => "(unset)".to_owned(),
    }
}

fn as_value(item: Item) -> Option<Value> {
    match item {
        Item::Value(value) => Some(value),
        Item::Table(table) => Some(Value::InlineTable(table.into_inline_table())),
        Item::ArrayOfTables(tables) => Some(Value::Array(tables.into_array())),
        Item::None => None,
    }
}

/// `item` as one line of inline TOML.
pub(crate) fn show(item: &Item) -> String {
    let Some(value) = as_value(item.clone()) else {
        return "(unset)".to_owned();
    };
    if let Value::String(text) = &value {
        let text = text.value();
        if text.len() > LONG_STRING || text.contains('\n') {
            return format!("<{} bytes>", text.len());
        }
    }
    inline(value)
}

/// `value` without the layout the serializer gave it, with one space around `=` and
/// inside braces.
fn inline(mut value: Value) -> String {
    normalize(&mut value);
    value.decor_mut().clear();
    value.to_string()
}

fn normalize(value: &mut Value) {
    match value {
        Value::Array(array) => {
            for inner in array.iter_mut() {
                normalize(inner);
            }
            array.fmt();
        }
        Value::InlineTable(table) => {
            for (_, inner) in table.iter_mut() {
                normalize(inner);
            }
            table.fmt();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;

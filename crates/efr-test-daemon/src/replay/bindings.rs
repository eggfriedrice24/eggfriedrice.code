//! Placeholders for the ids the daemon mints.
//!
//! A transcript cannot hold the conversation, turn, call and PTY ids of a run: they
//! come from the daemon's generator in an order that a fixture should not depend on.
//! It names them `<conversation:1>`, `<turn:2>`, `<call:1>`, `<pty:1>` instead: the
//! n-th distinct id of that kind the replay saw, in the order it saw them. A result or
//! an event binds a new id; a client frame that names a placeholder gets the id back.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, Value};

/// The members that hold minted ids, and the kind their placeholders name.
const ID_MEMBERS: &[(&str, &str)] = &[
    ("conversation_id", "conversation"),
    ("turn_id", "turn"),
    ("call_id", "call"),
    ("pty_id", "pty"),
];

/// The ids a replay has seen, by placeholder and by value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bindings {
    by_placeholder: BTreeMap<String, String>,
    by_value: HashMap<String, String>,
    counts: BTreeMap<&'static str, usize>,
}

impl Bindings {
    /// No ids seen yet.
    pub fn new() -> Self {
        Bindings::default()
    }

    /// The id behind `placeholder`, such as `<turn:1>`.
    pub fn get(&self, placeholder: &str) -> Option<&str> {
        self.by_placeholder.get(placeholder).map(String::as_str)
    }

    /// The placeholder of the id `value`, once it is bound.
    pub fn placeholder(&self, value: &str) -> Option<&str> {
        self.by_value.get(value).map(String::as_str)
    }

    /// `value` with every id in a `conversation_id`, `turn_id`, `call_id` or `pty_id`
    /// member replaced by its placeholder, binding the ids it has not seen, in document
    /// order.
    pub fn normalize(&mut self, value: &Value) -> Value {
        match value {
            Value::Object(members) => {
                let mut out = Map::with_capacity(members.len());
                for (name, member) in members {
                    let kind = ID_MEMBERS.iter().find(|(id, _)| id == name).map(|(_, kind)| *kind);
                    let member = match (kind, member) {
                        (Some(kind), Value::String(id)) if !is_placeholder(id) => {
                            Value::String(self.bind(kind, id))
                        }
                        _ => self.normalize(member),
                    };
                    out.insert(name.clone(), member);
                }
                Value::Object(out)
            }
            Value::Array(items) => {
                Value::Array(items.iter().map(|item| self.normalize(item)).collect())
            }
            other => other.clone(),
        }
    }

    /// `value` with every placeholder in its strings replaced by the id it stands for.
    /// Fails with the first placeholder that nothing bound.
    pub fn substitute(&self, value: &Value) -> Result<Value, String> {
        match value {
            Value::String(text) => self.substitute_text(text).map(Value::String),
            Value::Array(items) => items
                .iter()
                .map(|item| self.substitute(item))
                .collect::<Result<_, _>>()
                .map(Value::Array),
            Value::Object(members) => {
                let mut out = Map::with_capacity(members.len());
                for (name, member) in members {
                    out.insert(name.clone(), self.substitute(member)?);
                }
                Ok(Value::Object(out))
            }
            other => Ok(other.clone()),
        }
    }

    fn substitute_text(&self, text: &str) -> Result<String, String> {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find('<') {
            out.push_str(&rest[..start]);
            let candidate = &rest[start..];
            match candidate.find('>').map(|end| &candidate[..=end]) {
                Some(placeholder) if is_placeholder(placeholder) => {
                    let id = self.get(placeholder).ok_or_else(|| placeholder.to_owned())?;
                    out.push_str(id);
                    rest = &candidate[placeholder.len()..];
                }
                _ => {
                    out.push('<');
                    rest = &candidate[1..];
                }
            }
        }
        out.push_str(rest);
        Ok(out)
    }

    fn bind(&mut self, kind: &'static str, id: &str) -> String {
        if let Some(placeholder) = self.by_value.get(id) {
            return placeholder.clone();
        }
        let count = self.counts.entry(kind).or_default();
        *count += 1;
        let placeholder = format!("<{kind}:{count}>");
        self.by_value.insert(id.to_owned(), placeholder.clone());
        self.by_placeholder.insert(placeholder.clone(), id.to_owned());
        placeholder
    }
}

/// True for `<kind:n>` with a kind of [`ID_MEMBERS`] and a decimal `n`.
fn is_placeholder(text: &str) -> bool {
    let Some(inner) = text.strip_prefix('<').and_then(|rest| rest.strip_suffix('>')) else {
        return false;
    };
    let Some((kind, number)) = inner.split_once(':') else {
        return false;
    };
    ID_MEMBERS.iter().any(|(_, known)| *known == kind)
        && !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests;

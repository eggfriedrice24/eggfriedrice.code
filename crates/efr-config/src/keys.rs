//! Every key of the file, derived from the typed tables: their order, their kind and
//! the JSON schema.
//!
//! The tables are the one place a key is written. The JSON schema comes from their
//! `JsonSchema` derives, and the order of the keys from their `Deserialize` derives,
//! which name the fields in the order they are declared. A test keeps the two lists
//! equal, so a table that [`table_fields`] forgets fails it.

use std::fmt;

use serde::de::{self, DeserializeOwned, Deserializer, Visitor};
use serde_json::Value as Json;

use crate::{
    CompactionSettings, ConversationSettings, DiffColors, ModelSettings, OpenAiSettings,
    PermissionSettings, RenderColors, RenderSettings, SandboxSettings, Settings, ShellSettings,
    SnapshotSettings,
};

/// Where the JSON schema of the file is published. The first line of the example file
/// names it, so editors that read `#:schema` lines check the file as it is typed.
pub const SCHEMA_URL: &str = "https://raw.githubusercontent.com/eggfriedrice24/eggfriedrice.code/main/docs/config.schema.json";

/// The keys a change applies to only after the daemon restarts. Every other key of the
/// daemon applies to the next turn, prompt or tool call; the `render` keys belong to
/// `efr`.
pub const RESTART_KEYS: &[&str] = &[
    "screen",
    "model.provider",
    "openai.originator",
    "openai.subscription_base_url",
    "openai.api_base_url",
    "openai.websocket",
];

/// What a key holds, as the JSON schema says.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Kind {
    /// A string.
    String,
    /// One of these names.
    Choice(Vec<String>),
    /// A whole number.
    Integer,
    /// `true` or `false`.
    Boolean,
    /// A list of strings.
    List,
    /// `[[permissions.rules]]`: tables, which only an editor or the rule operations
    /// change.
    Rules,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::String => f.write_str("a string"),
            Kind::Choice(names) => match names.split_last() {
                Some((last, [])) => write!(f, "{last}"),
                Some((last, rest)) => write!(f, "{} or {last}", rest.join(", ")),
                None => f.write_str("nothing"),
            },
            Kind::Integer => f.write_str("a whole number"),
            Kind::Boolean => f.write_str("true or false"),
            Kind::List => f.write_str("a list of strings"),
            Kind::Rules => f.write_str("a list of rules"),
        }
    }
}

/// Every dotted key of the file, in the order the tables declare them: `log`, `screen`,
/// then `model.provider` and the rest of each table.
pub fn keys() -> Vec<String> {
    let mut keys = Vec::new();
    for field in fields::<Settings>() {
        add_keys((*field).to_owned(), &mut keys);
    }
    keys
}

/// Adds `key`, or every key of the table that `key` names.
fn add_keys(key: String, keys: &mut Vec<String>) {
    match table_fields(&key) {
        Some(names) => {
            for name in names {
                add_keys(format!("{key}.{name}"), keys);
            }
        }
        None => keys.push(key),
    }
}

/// The fields of the table named `table` (a dotted key), or `None` for a key that is
/// not a table.
fn table_fields(table: &str) -> Option<&'static [&'static str]> {
    Some(match table {
        "model" => fields::<ModelSettings>(),
        "openai" => fields::<OpenAiSettings>(),
        "permissions" => fields::<PermissionSettings>(),
        "shell" => fields::<ShellSettings>(),
        "conversation" => fields::<ConversationSettings>(),
        "compaction" => fields::<CompactionSettings>(),
        "sandbox" => fields::<SandboxSettings>(),
        "snapshot" => fields::<SnapshotSettings>(),
        "render" => fields::<RenderSettings>(),
        "render.colors" => fields::<RenderColors>(),
        "render.colors.diff" => fields::<DiffColors>(),
        _ => return None,
    })
}

/// True when the dotted `key` names a table of keys, such as `render.colors`.
pub(crate) fn is_table(key: &str) -> bool {
    table_fields(key).is_some()
}

/// The JSON schema of the file.
pub fn json_schema() -> Json {
    let mut schema = schemars::schema_for!(Settings).to_value();
    if let Some(object) = schema.as_object_mut() {
        object.insert("$id".to_owned(), Json::String(SCHEMA_URL.to_owned()));
        object.insert("title".to_owned(), Json::String("efr config.toml".to_owned()));
    }
    schema
}

/// When a change of a key takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Applies {
    /// The daemon applies it without a restart: to the next turn, prompt, tool call or
    /// new shell.
    Live,
    /// The daemon applies it only after a restart: a key of [`RESTART_KEYS`].
    Restart,
    /// Only `efr` reads it, at its next run.
    Client,
}

impl Applies {
    /// When a change of `key` takes effect.
    pub fn of(key: &str) -> Applies {
        if RESTART_KEYS.contains(&key) {
            Applies::Restart
        } else if key.starts_with("render.") {
            Applies::Client
        } else {
            Applies::Live
        }
    }

    /// `live`, `restart` or `client`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Applies::Live => "live",
            Applies::Restart => "restart",
            Applies::Client => "client",
        }
    }
}

/// What `key` holds, or `None` when the file has no such key.
pub fn kind(key: &str) -> Option<Kind> {
    let schema = json_schema();
    let node = property(&schema, key)?;
    kind_of(&schema, resolve(&schema, node))
}

/// The description of `key` in the JSON schema, the doc comment of its field, or `None`
/// when the file has no such key.
pub fn description(key: &str) -> Option<String> {
    let schema = json_schema();
    let node = property(&schema, key)?;
    let text = node.get("description").or_else(|| resolve(&schema, node).get("description"));
    text.and_then(Json::as_str).map(str::to_owned)
}

/// The schema node of the property `key`, a dotted key.
fn property<'a>(schema: &'a Json, key: &str) -> Option<&'a Json> {
    let mut node = schema;
    for part in key.split('.') {
        node = resolve(schema, node).get("properties")?.get(part)?;
    }
    Some(node)
}

/// The schema `node` points to with `$ref`, or `node` itself.
fn resolve<'a>(schema: &'a Json, node: &'a Json) -> &'a Json {
    let Some(reference) = node.get("$ref").and_then(Json::as_str) else {
        return node;
    };
    reference
        .strip_prefix("#/")
        .map(|path| path.split('/').try_fold(schema, |at, part| at.get(part)))
        .and_then(|found| found)
        .unwrap_or(node)
}

fn kind_of(schema: &Json, node: &Json) -> Option<Kind> {
    let names: Option<Vec<String>> = match (node.get("enum"), node.get("oneOf")) {
        (Some(Json::Array(values)), _) => {
            values.iter().map(|value| value.as_str().map(str::to_owned)).collect()
        }
        (_, Some(Json::Array(variants))) => variants
            .iter()
            .map(|variant| variant.get("const").and_then(Json::as_str).map(str::to_owned))
            .collect(),
        _ => None,
    };
    if let Some(names) = names {
        return Some(Kind::Choice(names));
    }
    let types: Vec<&str> = match node.get("type") {
        Some(Json::String(kind)) => vec![kind.as_str()],
        Some(Json::Array(kinds)) => kinds.iter().filter_map(Json::as_str).collect(),
        _ => Vec::new(),
    };
    match types.iter().find(|kind| **kind != "null").copied() {
        Some("string") => Some(Kind::String),
        Some("integer") => Some(Kind::Integer),
        Some("boolean") => Some(Kind::Boolean),
        Some("array") => {
            let items = node.get("items").map(|items| resolve(schema, items));
            // NOTE: an entry of `openai.models` is a string or a table. Such a list is
            // still set from text as a list of strings; only an editor writes tables.
            let takes_string = |items: &Json| {
                items.get("type").and_then(Json::as_str) == Some("string")
                    || items.get("anyOf").and_then(Json::as_array).is_some_and(|variants| {
                        variants.iter().any(|variant| {
                            resolve(schema, variant).get("type").and_then(Json::as_str)
                                == Some("string")
                        })
                    })
            };
            if items.is_some_and(takes_string) { Some(Kind::List) } else { Some(Kind::Rules) }
        }
        _ => None,
    }
}

/// The names of the fields of `T`, in the order it declares them.
///
/// NOTE: serde's derive hands the field names to `deserialize_struct`; this
/// deserializer only records them and stops. It is the one way to read the declared
/// order, because the JSON schema keeps its properties sorted.
fn fields<T: DeserializeOwned>() -> &'static [&'static str] {
    let mut found = None;
    // The deserializer always stops with an error, after it recorded the names.
    let _ = T::deserialize(FieldNames { found: &mut found });
    found.unwrap_or(&[])
}

struct FieldNames<'a> {
    found: &'a mut Option<&'static [&'static str]>,
}

#[derive(Debug)]
struct Stop;

impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("only the field names were read")
    }
}

impl std::error::Error for Stop {}

impl de::Error for Stop {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Stop
    }
}

impl<'de> Deserializer<'de> for FieldNames<'_> {
    type Error = Stop;

    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Stop> {
        Err(Stop)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        _: V,
    ) -> Result<V::Value, Stop> {
        *self.found = Some(fields);
        Err(Stop)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes
        byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map enum
        identifier ignored_any
    }
}

#[cfg(test)]
mod tests;

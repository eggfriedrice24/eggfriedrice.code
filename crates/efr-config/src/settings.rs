//! [`Settings`]: the whole file, typed, with the defaults filled in and the source of
//! every value.

use std::io;
use std::path::{Path, PathBuf};

use efr_permissions::{Policy, Rule};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use toml_edit::{Document, Item};

use crate::keys::{self, Kind};
use crate::location::{key_at, location, span_of, span_of_rule};
use crate::validate::{self, Invalid};
use crate::{
    ConfigError, ConversationSettings, DEFAULT_LOG, Location, ModelSettings, OpenAiSettings,
    PermissionSettings, RenderSettings, ScreenChoice, ShellSettings, Source,
};

/// The file name under the config root.
pub const CONFIG_FILE: &str = "config.toml";

/// The effective settings: the built-in defaults, then `config.toml`, then whatever
/// environment variables and flags the program applies with
/// [`apply_override`](Self::apply_override).
///
/// A missing file is an empty one, so every value is its default. The fields are
/// public so a test can start from [`Settings::default`] and change what it needs; a
/// changed field keeps the source it had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct Settings {
    /// The file these settings were read from, whether or not it exists.
    #[serde(skip)]
    pub path: PathBuf,
    /// The tracing filter of the daemon, in `EnvFilter` syntax, such as `info` or
    /// `info,efr_=debug`. Needs a restart.
    pub log: String,
    /// The screen backend of the hidden shells: `auto`, `vt100` or `ghostty`. Needs a
    /// restart.
    pub screen: ScreenChoice,
    /// The provider, the default model of a turn and what each request carries.
    pub model: ModelSettings,
    /// The OpenAI providers.
    pub openai: OpenAiSettings,
    /// The permission mode, extra secrets and the user's rules.
    pub permissions: PermissionSettings,
    /// How hidden shells start and when idle ones stop.
    pub shell: ShellSettings,
    /// Queues, approvals, streaming and terminals.
    pub conversation: ConversationSettings,
    /// How `efr` shows replies.
    pub render: RenderSettings,
    #[serde(skip)]
    sources: Vec<(String, Source)>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            path: PathBuf::from(CONFIG_FILE),
            log: DEFAULT_LOG.to_owned(),
            screen: ScreenChoice::Auto,
            model: ModelSettings::default(),
            openai: OpenAiSettings::default(),
            permissions: PermissionSettings::default(),
            shell: ShellSettings::default(),
            conversation: ConversationSettings::default(),
            render: RenderSettings::default(),
            sources: Vec::new(),
        }
    }
}

impl Settings {
    /// Reads `config.toml` from the config root `config_dir`; a missing file, or a link
    /// to nothing, is an empty one. This blocks: async code calls it in
    /// `spawn_blocking`, or reads the file itself and calls [`parse`](Self::parse).
    pub fn load(config_dir: &Path) -> Result<Settings, ConfigError> {
        let path = config_dir.join(CONFIG_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(source) if source.kind() == io::ErrorKind::NotFound => None,
            Err(source) => return Err(ConfigError::Read { path, source }),
        };
        Settings::parse(&path, text.as_deref())
    }

    /// The settings of the file at `path` with contents `text`, `None` when it does not
    /// exist. Every key is checked: unknown keys, types, sets, ranges and rules.
    pub fn parse(path: &Path, text: Option<&str>) -> Result<Settings, ConfigError> {
        let Some(text) = text else {
            return Ok(Settings { path: path.to_path_buf(), ..Settings::default() });
        };
        let mut settings: Settings = toml::from_str(text).map_err(|source| {
            let span = source.span();
            let document = Document::parse(text.to_owned()).ok();
            ConfigError::Parse {
                path: path.to_path_buf(),
                location: span.as_ref().map(|span| location(text, span.start)),
                key: span.zip(document.as_ref()).and_then(|(span, doc)| key_at(doc, span.start)),
                source: Box::new(source),
            }
        })?;
        // The text parsed as TOML above, so it parses here too; without the document
        // only the places of later errors are lost.
        let document = Document::parse(text.to_owned()).ok();
        settings.path = path.to_path_buf();
        settings.permissions.rules = rules(path, text, document.as_ref())?;
        if let Some(document) = &document {
            settings.sources = file_keys(document);
        }
        validate::check(&settings).map_err(|Invalid { key, value, expected }| {
            let location = document
                .as_ref()
                .and_then(|document| span_of(document, key))
                .map(|span| location(text, span.start));
            ConfigError::Invalid { path: path.to_path_buf(), key, value, expected, location }
        })?;
        Ok(settings)
    }

    /// Where `key` (a dotted key, as [`keys`](crate::keys) lists it) came from.
    pub fn source(&self, key: &str) -> Source {
        self.sources
            .iter()
            .find(|(known, _)| known == key)
            .map(|(_, source)| source.clone())
            .unwrap_or_default()
    }

    /// Records that `key` came from `source`, for a program that sets a field itself.
    pub fn set_source(&mut self, key: &str, source: Source) {
        self.sources.retain(|(known, _)| known != key);
        // A key without an entry is a default, so equal settings compare equal.
        if source != Source::Default {
            self.sources.push((key.to_owned(), source));
        }
    }

    /// Takes the source of every key from `other`.
    pub(crate) fn copy_sources(&mut self, other: &Settings) {
        self.sources.clone_from(&other.sources);
    }

    /// Sets `key` to `text`, the value of an environment variable or a flag, as the file
    /// would: a string as it is, a number, `true` or `false`, a list as a TOML array or
    /// as words separated by commas. The value is checked like one from the file.
    pub fn apply_override(
        &mut self,
        key: &str,
        text: &str,
        from: Source,
    ) -> Result<(), ConfigError> {
        let refuse = |expected: String| ConfigError::InvalidOverride {
            key: key.to_owned(),
            value: text.to_owned(),
            from: from.clone(),
            expected,
        };
        let kind =
            keys::kind(key).ok_or_else(|| ConfigError::UnknownKey { key: key.to_owned() })?;
        let value = value_from_text(&kind, text).ok_or_else(|| refuse(kind.to_string()))?;
        let mut document =
            toml_edit::ser::to_document(self).map_err(|source| ConfigError::Encode { source })?;
        let mut item = document.as_item_mut();
        for part in key.split('.') {
            item = &mut item[part];
        }
        *item = Item::Value(value);
        let mut next: Settings = toml::from_str(&document.to_string())
            .map_err(|source| refuse(source.message().to_owned()))?;
        next.path = self.path.clone();
        next.sources = self.sources.clone();
        next.permissions.rules = self.permissions.rules.clone();
        validate::check(&next).map_err(|invalid| refuse(invalid.expected.to_owned()))?;
        *self = next;
        self.set_source(key, from);
        Ok(())
    }

    /// The name of the default model when the effective model list `known` does not
    /// hold it. The daemon warns at load; a prompt that names an unknown model fails.
    pub fn unknown_model<'a>(&'a self, known: &[&str]) -> Option<&'a str> {
        self.model.name.as_deref().filter(|name| !known.contains(name))
    }
}

/// A value of kind `kind` from `text`, or `None` when `text` is not one.
pub(crate) fn value_from_text(kind: &Kind, text: &str) -> Option<toml_edit::Value> {
    match kind {
        Kind::String => Some(text.into()),
        Kind::Choice(names) => names.iter().any(|name| name == text).then(|| text.into()),
        Kind::Integer => text.trim().parse::<i64>().ok().map(Into::into),
        Kind::Boolean => text.trim().parse::<bool>().ok().map(Into::into),
        Kind::List => {
            let trimmed = text.trim();
            if trimmed.starts_with('[') {
                let value = trimmed.parse::<toml_edit::Value>().ok()?;
                let all_strings =
                    value.as_array().is_some_and(|array| array.iter().all(|item| item.is_str()));
                return all_strings.then_some(value);
            }
            let words = trimmed.split(',').map(str::trim).filter(|word| !word.is_empty());
            Some(toml_edit::Value::Array(words.collect()))
        }
        Kind::Rules => None,
    }
}

/// The rules of `permissions.rules`, read one by one so an error names the rule's
/// place.
fn rules(
    path: &Path,
    text: &str,
    document: Option<&Document<String>>,
) -> Result<Policy, ConfigError> {
    #[derive(Default, Deserialize)]
    struct RulesOnly {
        #[serde(default)]
        permissions: RulesTable,
    }
    #[derive(Default, Deserialize)]
    struct RulesTable {
        #[serde(default)]
        rules: Vec<toml::Value>,
    }
    let at = |index: usize| -> Option<Location> {
        document
            .and_then(|document| span_of_rule(document, index))
            .map(|span| location(text, span.start))
    };
    let only: RulesOnly = toml::from_str(text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        location: None,
        key: Some("permissions.rules".to_owned()),
        source: Box::new(source),
    })?;
    let mut policy = Policy::empty();
    for (index, value) in only.permissions.rules.into_iter().enumerate() {
        let rule: Rule = value.try_into().map_err(|source| ConfigError::ParseRule {
            path: path.to_path_buf(),
            index,
            location: at(index),
            source: Box::new(source),
        })?;
        // NOTE: each rule is pushed onto a policy of the ones before it, so the index in
        // the error is the rule's place in the file.
        policy.push(rule).map_err(|source| ConfigError::InvalidRule {
            path: path.to_path_buf(),
            index,
            location: at(index),
            source,
        })?;
    }
    Ok(policy)
}

/// Every dotted key that `document` sets, each with [`Source::File`].
fn file_keys(document: &Document<String>) -> Vec<(String, Source)> {
    let mut found = Vec::new();
    for (name, item) in document.as_table().iter() {
        match item {
            Item::Table(table) => {
                for (key, _) in table.iter() {
                    found.push((format!("{name}.{key}"), Source::File));
                }
            }
            Item::Value(toml_edit::Value::InlineTable(table)) => {
                for (key, _) in table.iter() {
                    found.push((format!("{name}.{key}"), Source::File));
                }
            }
            _ => found.push((name.to_owned(), Source::File)),
        }
    }
    found
}

#[cfg(test)]
mod tests;

//! A reload: the settings of a file read again, laid over the settings that run.
//!
//! Two kinds of value do not come from the new file. A key of [`RESTART_KEYS`] keeps
//! its running value, because the daemon built something from it at start that it
//! cannot rebuild (the screen backend, the provider); the key is listed as needing a
//! restart. A key that an environment variable or a flag set keeps that value and its
//! source, because the variable or the flag still wins over the file, as it did at start.

use toml_edit::{DocumentMut, Item};

use crate::effective::{exact, lookup};
use crate::{ConfigError, RESTART_KEYS, Settings, Source, keys};

/// The outcome of [`Settings::reloaded`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Reloaded {
    /// The settings to use from now on.
    pub settings: Settings,
    /// The dotted keys whose new values apply only after a restart, in the order of
    /// [`keys`]; they keep their running values until then.
    pub restart_needed: Vec<String>,
}

impl Settings {
    /// `next`, the settings of the file as read now, laid over `self`, the settings
    /// that run: a key of [`RESTART_KEYS`] whose value changed keeps its running value
    /// and is listed in [`Reloaded::restart_needed`], and a key that an environment
    /// variable or a flag set keeps that value.
    pub fn reloaded(&self, next: Settings) -> Result<Reloaded, ConfigError> {
        let encode = |settings: &Settings| {
            toml_edit::ser::to_document(settings).map_err(|source| ConfigError::Encode { source })
        };
        let running = encode(self)?;
        let mut fresh = encode(&next)?;
        let mut restart_needed = Vec::new();
        let mut kept: Vec<(String, Source)> = Vec::new();
        for key in keys() {
            let source = self.source(&key);
            let overridden = matches!(source, Source::Env(_) | Source::Flag(_));
            let old = lookup(&running, &key);
            let changed = old.map(exact) != lookup(&fresh, &key).map(exact);
            if !overridden && !(changed && RESTART_KEYS.contains(&key.as_str())) {
                continue;
            }
            if !overridden {
                restart_needed.push(key.clone());
            }
            replace(&mut fresh, &key, old.cloned());
            kept.push((key, source));
        }
        if kept.is_empty() {
            return Ok(Reloaded { settings: next, restart_needed });
        }
        // The values come from two valid settings, so they read back; a failure here
        // would be a bug in the serializer, which the caller reports like any error.
        let mut settings: Settings =
            toml::from_str(&fresh.to_string()).map_err(|source| ConfigError::Parse {
                path: next.path.clone(),
                location: None,
                key: None,
                source: Box::new(source),
            })?;
        settings.path = next.path.clone();
        settings.permissions.rules = next.permissions.rules.clone();
        settings.copy_sources(&next);
        for (key, source) in kept {
            settings.set_source(&key, source);
        }
        Ok(Reloaded { settings, restart_needed })
    }
}

/// Sets the dotted `key` of `document` to `item`, or removes it for `None`.
fn replace(document: &mut DocumentMut, key: &str, item: Option<Item>) {
    let (table, name) = match key.split_once('.') {
        Some((table, name)) => match document.get_mut(table).and_then(Item::as_table_like_mut) {
            Some(table) => (table, name),
            None => return,
        },
        None => (document.as_table_mut() as &mut dyn toml_edit::TableLike, key),
    };
    match item {
        Some(item) => {
            table.insert(name, item);
        }
        None => {
            table.remove(name);
        }
    }
}

#[cfg(test)]
mod tests;

//! The format-preserving writer: `efr config set` and the settings tool change the file
//! through it.
//!
//! [`ConfigFile::open`] reads the file ([`LinkedFile`], shared with the project
//! registry). [`ConfigFile::edit`] returns an [`Edit`] of the parsed document, which
//! keeps comments and layout (`toml_edit`). [`ConfigFile::write`] checks the whole new
//! file like a load would, compares the file with what was read right before it writes,
//! and writes atomically: a temporary file in the same directory, flushed, then renamed
//! over the file. When `config.toml`
//! is a symbolic link (into a dotfiles repository, say), the file behind it is written
//! and the link stays. A missing file is created, with its directory, from
//! [`EXAMPLE`]; a link to nothing is refused, because the new file would appear
//! somewhere the user may not expect.
//!
//! [`Edit::add_table_entry`] and [`Edit::remove_table_entry`] change an array of tables
//! (`permissions.rules`, `sandbox.allow`, `network.hosts`, `dbus.allow`) in the form
//! that the file uses; [`Edit::set`] never changes one.
//!
//! The functions block. Async code calls them in `spawn_blocking`.

use std::io;
use std::path::Path;

use efr_stdx::StdxError;
use efr_stdx::fs::LinkedFile;

use efr_permissions::Rule;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, TableLike, Value};

use crate::keys::{self, Kind};
use crate::settings::value_from_text;
use crate::{ConfigError, EXAMPLE, Settings};

/// The key of the rules.
const RULES: &str = "permissions.rules";

/// The arrays of tables of the file: the keys of [`Kind::Rules`]. Only an editor and
/// the entry operations of [`Edit`] change them, never [`Edit::set`].
pub(crate) const TABLE_KEYS: &[&str] =
    &["permissions.rules", "sandbox.allow", "network.hosts", "dbus.allow"];

/// `key` as one of [`TABLE_KEYS`], with its table and its name in that table.
fn table_key(key: &str) -> Result<(&'static str, &'static str, &'static str), ConfigError> {
    let unknown = || ConfigError::UnknownKey { key: key.to_owned() };
    let known = TABLE_KEYS.iter().copied().find(|known| *known == key).ok_or_else(unknown)?;
    let (parent, name) = known.split_once('.').ok_or_else(unknown)?;
    Ok((known, parent, name))
}

/// The config file as the writer read it.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    /// `config.toml` as named and read, through a symbolic link.
    file: LinkedFile,
}

impl ConfigFile {
    /// Reads the config file at `path`. A missing file is not an error: a write creates
    /// it from the example. A symbolic link to nothing is.
    pub fn open(path: &Path) -> Result<ConfigFile, ConfigError> {
        let file = LinkedFile::open(path).map_err(|error| match error {
            StdxError::DanglingLink { path, target } => {
                ConfigError::DanglingSymlink { path, target }
            }
            StdxError::ReadFile { path, source } => ConfigError::Read { path, source },
            // NOTE: an open fails only to read; any other error is kept as the source.
            other => {
                ConfigError::Read { path: path.to_path_buf(), source: io::Error::other(other) }
            }
        })?;
        Ok(ConfigFile { file })
    }

    /// `config.toml` as named.
    pub fn path(&self) -> &Path {
        self.file.path()
    }

    /// The file a write replaces: the end of the symbolic link, or the file itself.
    pub fn target(&self) -> &Path {
        self.file.target()
    }

    /// The contents as read; `None` when the file does not exist.
    pub fn text(&self) -> Option<&str> {
        self.file.text()
    }

    /// An edit of the file as read, or of the example when the file does not exist.
    /// The file must be TOML; its values need not be valid yet, so a change can fix
    /// one.
    pub fn edit(&self) -> Result<Edit, ConfigError> {
        let text = self.text().unwrap_or(EXAMPLE);
        let document = text.parse::<DocumentMut>().map_err(|_| self.syntax_error(text))?;
        Ok(Edit { document })
    }

    /// Writes `edit` and returns the settings it holds. The whole new file is checked
    /// first, as a load would check it. Fails with [`ConfigError::Changed`] when the
    /// file changed since [`open`](Self::open), so the caller plans the change again.
    pub fn write(&self, edit: &Edit) -> Result<Settings, ConfigError> {
        let text = edit.text();
        let settings = Settings::parse(self.path(), Some(&text))?;
        self.file.write_if_unchanged(text.as_bytes()).map_err(|error| match error {
            StdxError::FileChanged { path } => ConfigError::Changed { path },
            StdxError::CreateDir { path, source } => ConfigError::CreateDir { path, source },
            StdxError::ReadFile { path, source } => ConfigError::Read { path, source },
            other => ConfigError::Write { path: self.target().to_path_buf(), source: other },
        })?;
        Ok(settings)
    }

    fn syntax_error(&self, text: &str) -> ConfigError {
        let source = match toml::from_str::<toml::Table>(text) {
            Err(source) => source,
            // NOTE: both parsers read TOML 1.1, so this arm is not reached; the error
            // still names the file.
            Ok(_) => serde::de::Error::custom("the file could not be read as a document"),
        };
        ConfigError::Parse {
            path: self.path().to_path_buf(),
            location: source.span().map(|span| crate::location::location(text, span.start)),
            key: None,
            source: Box::new(source),
        }
    }
}

/// A change to the config file, kept as a document with its comments and layout.
#[derive(Debug, Clone)]
pub struct Edit {
    document: DocumentMut,
}

impl Edit {
    /// Sets `key`, a dotted key such as `model.name`, to `value`. A comment after the
    /// old value stays. The rules are tables, not one value, so they cannot be set
    /// here.
    pub fn set(&mut self, key: &str, value: Value) -> Result<(), ConfigError> {
        writable(key)?;
        if !key.contains('.') && !self.document.contains_key(key) {
            self.insert_top_level(key, value);
            return Ok(());
        }
        let (table, name) = self.table_of(key, true)?;
        let Some(table) = table else {
            return Err(ConfigError::UnknownKey { key: key.to_owned() });
        };
        match table.get_mut(name) {
            Some(Item::Value(old)) => {
                let decor = old.decor().clone();
                *old = value;
                *old.decor_mut() = decor;
            }
            _ => {
                table.insert(name, Item::Value(value));
            }
        }
        Ok(())
    }

    /// Sets `key` to `text` read as the key's kind: a string as it is, a number,
    /// `true` or `false`, a list as a TOML array or as words separated by commas.
    pub fn set_text(&mut self, key: &str, text: &str) -> Result<(), ConfigError> {
        let kind = writable(key)?;
        let value = value_from_text(&kind, text).ok_or_else(|| ConfigError::InvalidValue {
            key: key.to_owned(),
            value: text.to_owned(),
            expected: kind.to_string(),
        })?;
        self.set(key, value)
    }

    /// Removes `key`, so its default applies again. False when the file did not set it.
    pub fn unset(&mut self, key: &str) -> Result<bool, ConfigError> {
        writable(key)?;
        let (table, name) = self.table_of(key, false)?;
        Ok(table.is_some_and(|table| table.remove(name).is_some()))
    }

    /// Appends `rule` to `permissions.rules`, after the rules the file has, so it wins
    /// over them where both match. It writes the rule as
    /// [`add_table_entry`](Self::add_table_entry) does.
    pub fn add_rule(&mut self, rule: &Rule) -> Result<(), ConfigError> {
        self.add_table_entry(RULES, rule).map(|_| ())
    }

    /// Removes `permissions.rules[index]`, counted from 0 as `efr config show` and the
    /// errors number the rules. The rules after it move up by one.
    pub fn remove_rule(&mut self, index: usize) -> Result<(), ConfigError> {
        self.remove_table_entry(RULES, index).map_err(|error| match error {
            ConfigError::NoEntry { index, count, .. } => ConfigError::NoRule { index, count },
            other => other,
        })
    }

    /// Appends `entry` to the array of tables at `key`, such as `sandbox.allow`, after
    /// the entries that the file has, and returns the new entry's index. It keeps the
    /// form that the file uses: `[[sandbox.allow]]` tables, an inline array
    /// (`allow = [{ .. }]`), an inline parent table (`sandbox = { allow = [..] }`) or a
    /// dotted key (`sandbox.allow = [..]`). A file with no entries gets a
    /// `[[sandbox.allow]]` table right after its `[sandbox]` table, and a file with no
    /// `[sandbox]` table gets one at its end. The new text is checked when the file is
    /// written, so an entry that a load refuses is never written.
    pub fn add_table_entry<T: serde::Serialize>(
        &mut self,
        key: &str,
        entry: &T,
    ) -> Result<usize, ConfigError> {
        let (key, parent, name) = table_key(key)?;
        let encoded =
            toml_edit::ser::to_document(entry).map_err(|source| ConfigError::Encode { source })?;
        let mut table = encoded.as_table().clone();
        table.decor_mut().clear();
        let inline_parent = self.document.get(parent).is_some_and(Item::is_inline_table);
        let inline_entry = |table: Table| {
            let mut inline = table.into_inline_table();
            inline.fmt();
            Value::InlineTable(inline)
        };
        let holder = self.parent_table(parent)?;
        let count = match holder.get_mut(name) {
            Some(Item::ArrayOfTables(tables)) => {
                tables.push(table);
                tables.len()
            }
            Some(Item::Value(Value::Array(array))) => {
                array.push(inline_entry(table));
                array.len()
            }
            Some(_) => return Err(ConfigError::NotATable { key }),
            None if inline_parent => {
                let array: toml_edit::Array = std::iter::once(inline_entry(table)).collect();
                holder.insert(name, Item::Value(Value::Array(array)));
                1
            }
            None => {
                let mut tables = ArrayOfTables::new();
                tables.push(table);
                holder.insert(name, Item::ArrayOfTables(tables));
                1
            }
        };
        Ok(count - 1)
    }

    /// Removes entry `index`, counted from 0, of the array of tables at `key`, such as
    /// `sandbox.allow`. The entries after it move up by one; the key goes away with its
    /// last entry.
    pub fn remove_table_entry(&mut self, key: &str, index: usize) -> Result<(), ConfigError> {
        let (key, parent, name) = table_key(key)?;
        let missing = |count| ConfigError::NoEntry { key, index, count };
        let Some(holder) =
            self.document.as_table_mut().get_mut(parent).and_then(Item::as_table_like_mut)
        else {
            return Err(missing(0));
        };
        let left = match holder.get_mut(name) {
            None => return Err(missing(0)),
            Some(Item::ArrayOfTables(tables)) => {
                let count = tables.len();
                if index >= count {
                    return Err(missing(count));
                }
                tables.remove(index);
                tables.len()
            }
            Some(Item::Value(Value::Array(array))) => {
                let count = array.len();
                if index >= count {
                    return Err(missing(count));
                }
                array.remove(index);
                array.len()
            }
            Some(_) => return Err(ConfigError::NotATable { key }),
        };
        if left == 0 {
            holder.remove(name);
        }
        Ok(())
    }

    /// The top-level table `name`, such as `[permissions]`, added at the end of the
    /// file when it is missing.
    fn parent_table(&mut self, name: &'static str) -> Result<&mut dyn TableLike, ConfigError> {
        let root = self.document.as_table_mut();
        if !root.contains_key(name) {
            root.insert(name, Item::Table(Table::new()));
        }
        root.get_mut(name)
            .and_then(Item::as_table_like_mut)
            .ok_or(ConfigError::NotATable { key: name })
    }

    /// The new contents of the file.
    pub fn text(&self) -> String {
        self.document.to_string()
    }

    /// Adds the top-level key `key`. A file with no top-level value keeps its opening
    /// comments in front of its first table; they move in front of the new key, so the
    /// key does not land above the `#:schema` line.
    fn insert_top_level(&mut self, key: &str, value: Value) {
        let root = self.document.as_table_mut();
        let has_values = root.iter().any(|(_, item)| item.is_value());
        let first_table = root
            .iter_mut()
            .filter_map(|(_, item)| item.as_table_mut())
            .min_by_key(|table| table.position().unwrap_or(isize::MAX));
        let moved = match first_table {
            Some(table) if !has_values => {
                let prefix = table.decor().prefix().and_then(|raw| raw.as_str()).map(str::to_owned);
                if prefix.is_some() {
                    table.decor_mut().set_prefix("\n");
                }
                prefix
            }
            _ => None,
        };
        root.insert(key, Item::Value(value));
        if let (Some(prefix), Some(mut inserted)) = (moved, root.key_mut(key)) {
            inserted.leaf_decor_mut().set_prefix(prefix);
        }
    }

    /// The table that holds `key` and the key's last part. With `create`, a missing
    /// table is added at the end of the file; a table below a table below the root,
    /// such as `diff` of `render.colors.diff.add`, is added as dotted keys.
    fn table_of<'a, 'k>(
        &'a mut self,
        key: &'k str,
        create: bool,
    ) -> Result<(Option<&'a mut dyn TableLike>, &'k str), ConfigError> {
        let unknown = || ConfigError::UnknownKey { key: key.to_owned() };
        let Some((path, name)) = key.rsplit_once('.') else {
            return Ok((Some(self.document.as_table_mut() as &mut dyn TableLike), key));
        };
        let mut table: &mut dyn TableLike = self.document.as_table_mut();
        for (depth, part) in path.split('.').enumerate() {
            if !table.contains_key(part) {
                if !create {
                    return Ok((None, name));
                }
                let mut new = Table::new();
                new.set_dotted(depth >= 2);
                table.insert(part, Item::Table(new));
            }
            table = table.get_mut(part).and_then(Item::as_table_like_mut).ok_or_else(unknown)?;
        }
        Ok((Some(table), name))
    }
}

/// The kind of `key` when the writer may change it.
fn writable(key: &str) -> Result<Kind, ConfigError> {
    let unknown = || ConfigError::UnknownKey { key: key.to_owned() };
    if !keys::keys().iter().any(|known| known == key) {
        return Err(unknown());
    }
    match keys::kind(key) {
        Some(Kind::Rules | Kind::Groups) | None => Err(unknown()),
        Some(kind) => Ok(kind),
    }
}

#[cfg(test)]
mod tests;

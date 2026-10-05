//! The format-preserving writer: `efr config set` and the settings tool change the file
//! through it.
//!
//! [`ConfigFile::open`] reads the file and a hash of its bytes. [`ConfigFile::edit`]
//! returns an [`Edit`] of the parsed document, which keeps comments and layout
//! (`toml_edit`). [`ConfigFile::write`] checks the whole new file like a load would,
//! compares the hash again right before it writes, and writes atomically: a temporary
//! file in the same directory, flushed, then renamed over the file. When `config.toml`
//! is a symbolic link (into a dotfiles repository, say), the file behind it is written
//! and the link stays. A missing file is created, with its directory, from
//! [`EXAMPLE`]; a link to nothing is refused, because the new file would appear
//! somewhere the user may not expect.
//!
//! The functions block. Async code calls them in `spawn_blocking`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};
use toml_edit::{DocumentMut, Item, Table, TableLike};

use crate::keys::{self, Kind};
use crate::settings::value_from_text;
use crate::{ConfigError, EXAMPLE, Settings};

/// The config file as the writer read it.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    /// `config.toml` as named, which may be a symbolic link.
    path: PathBuf,
    /// The file that is written: the end of the link, or `path` itself.
    target: PathBuf,
    /// The contents; `None` when the file does not exist.
    text: Option<String>,
    /// The hash of the contents, compared again right before a write.
    hash: Option<[u8; 32]>,
}

impl ConfigFile {
    /// Reads the config file at `path`. A missing file is not an error: a write creates
    /// it from the example. A symbolic link to nothing is.
    pub fn open(path: &Path) -> Result<ConfigFile, ConfigError> {
        let read_error = |source| ConfigError::Read { path: path.to_path_buf(), source };
        let target = match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => match fs::canonicalize(path) {
                Ok(target) => target,
                Err(source) if source.kind() == io::ErrorKind::NotFound => {
                    let target = fs::read_link(path).map_err(read_error)?;
                    return Err(ConfigError::DanglingSymlink { path: path.to_path_buf(), target });
                }
                Err(source) => return Err(read_error(source)),
            },
            Ok(_) => path.to_path_buf(),
            Err(source) if source.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
            Err(source) => return Err(read_error(source)),
        };
        let text = read(&target)?;
        let hash = text.as_deref().map(|text| hash(text.as_bytes()));
        Ok(ConfigFile { path: path.to_path_buf(), target, text, hash })
    }

    /// `config.toml` as named.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file a write replaces: the end of the symbolic link, or the file itself.
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// The contents as read; `None` when the file does not exist.
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// An edit of the file as read, or of the example when the file does not exist.
    /// The file must be TOML; its values need not be valid yet, so a change can fix
    /// one.
    pub fn edit(&self) -> Result<Edit, ConfigError> {
        let text = self.text.as_deref().unwrap_or(EXAMPLE);
        let document = text.parse::<DocumentMut>().map_err(|_| self.syntax_error(text))?;
        Ok(Edit { document })
    }

    /// Writes `edit` and returns the settings it holds. The whole new file is checked
    /// first, as a load would check it. Fails with [`ConfigError::Changed`] when the
    /// file changed since [`open`](Self::open), so the caller plans the change again.
    pub fn write(&self, edit: &Edit) -> Result<Settings, ConfigError> {
        let text = edit.text();
        let settings = Settings::parse(&self.path, Some(&text))?;
        let changed = || ConfigError::Changed { path: self.target.clone() };
        let now = read(&self.target)?;
        if now.as_deref().map(|now| hash(now.as_bytes())) != self.hash {
            return Err(changed());
        }
        if self.text.is_none() {
            // A link or file that appeared since the read is somebody else's change.
            match fs::symlink_metadata(&self.path) {
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                _ => return Err(changed()),
            }
            if let Some(dir) = self.target.parent().filter(|dir| !dir.as_os_str().is_empty()) {
                fs::create_dir_all(dir)
                    .map_err(|source| ConfigError::CreateDir { path: dir.to_path_buf(), source })?;
            }
        }
        efr_stdx::fs::write_atomic(&self.target, text.as_bytes())
            .map_err(|source| ConfigError::Write { path: self.target.clone(), source })?;
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
            path: self.path.clone(),
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
    pub fn set(&mut self, key: &str, value: toml_edit::Value) -> Result<(), ConfigError> {
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

    /// The new contents of the file.
    pub fn text(&self) -> String {
        self.document.to_string()
    }

    /// Adds the top-level key `key`. A file with no top-level value keeps its opening
    /// comments in front of its first table; they move in front of the new key, so the
    /// key does not land above the `#:schema` line.
    fn insert_top_level(&mut self, key: &str, value: toml_edit::Value) {
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
    /// table is added at the end of the file.
    fn table_of<'a, 'k>(
        &'a mut self,
        key: &'k str,
        create: bool,
    ) -> Result<(Option<&'a mut dyn TableLike>, &'k str), ConfigError> {
        let unknown = || ConfigError::UnknownKey { key: key.to_owned() };
        let Some((table, name)) = key.split_once('.') else {
            return Ok((Some(self.document.as_table_mut() as &mut dyn TableLike), key));
        };
        let root = self.document.as_table_mut();
        if !root.contains_key(table) {
            if !create {
                return Ok((None, name));
            }
            root.insert(table, Item::Table(Table::new()));
        }
        let found = root.get_mut(table).and_then(Item::as_table_like_mut).ok_or_else(unknown)?;
        Ok((Some(found), name))
    }
}

/// The kind of `key` when the writer may change it.
fn writable(key: &str) -> Result<Kind, ConfigError> {
    let unknown = || ConfigError::UnknownKey { key: key.to_owned() };
    if !keys::keys().iter().any(|known| known == key) {
        return Err(unknown());
    }
    match keys::kind(key) {
        Some(Kind::Rules) | None => Err(unknown()),
        Some(kind) => Ok(kind),
    }
}

/// The contents of `path`; `None` when it does not exist.
fn read(path: &Path) -> Result<Option<String>, ConfigError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ConfigError::Read { path: path.to_path_buf(), source }),
    }
}

fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[cfg(test)]
mod tests;

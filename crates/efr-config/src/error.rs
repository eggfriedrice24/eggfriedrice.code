//! The one public error type of the crate.

use std::fmt;
use std::io;
use std::path::PathBuf;

use efr_permissions::PermissionsError;
use efr_protocol::ConfigFileError;
use efr_stdx::StdxError;

use crate::Source;

/// A place in the config file, counted from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    /// The line.
    pub line: u32,
    /// The column, in characters.
    pub column: u32,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, column {}", self.line, self.column)
    }
}

/// Every way reading, checking or writing the config file can fail.
///
/// Variants carry the data a caller acts on; a message names what failed in one
/// sentence and leaves the source's text to the `source()` chain.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The config file exists but could not be read.
    #[error("could not read the config file {}", .path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },
    /// The config file is not valid TOML, or a key is unknown or holds a value of the
    /// wrong type.
    #[error("the config file {} is not valid", .path.display())]
    Parse {
        /// The file.
        path: PathBuf,
        /// Where the error is, when the parser says.
        location: Option<Location>,
        /// The dotted key that holds the error, when one does.
        key: Option<String>,
        /// The parser's error.
        #[source]
        source: Box<toml::de::Error>,
    },
    /// A permission rule in the config file does not have the shape of a rule: an
    /// unknown key, a missing one, or a value outside its set.
    #[error("permissions.rules[{index}] in {} is not a rule", .path.display())]
    ParseRule {
        /// The config file.
        path: PathBuf,
        /// The rule's place in `permissions.rules`, counted from 0.
        index: usize,
        /// Where the rule is.
        location: Option<Location>,
        /// The parser's error.
        #[source]
        source: Box<toml::de::Error>,
    },
    /// A permission rule in the config file names a relative path, a program, argument
    /// or forbidden word that is not one plain word, or an action its resource never
    /// matches.
    #[error("permissions.rules[{index}] in {} is invalid", .path.display())]
    InvalidRule {
        /// The config file.
        path: PathBuf,
        /// The rule's place in `permissions.rules`, counted from 0.
        index: usize,
        /// Where the rule is.
        location: Option<Location>,
        /// The error from `efr-permissions`.
        #[source]
        source: PermissionsError,
    },
    /// A value in the config file is outside its allowed set or range.
    #[error("the config value {key} = {value} in {} is not {expected}", .path.display())]
    Invalid {
        /// The config file.
        path: PathBuf,
        /// The dotted key.
        key: &'static str,
        /// The value, as TOML.
        value: String,
        /// What the value must be.
        expected: &'static str,
        /// Where the value is.
        location: Option<Location>,
    },
    /// A value from an environment variable or a flag cannot be used for its key.
    #[error("{key} = {value:?} from {from} is not {expected}")]
    InvalidOverride {
        /// The dotted key.
        key: String,
        /// The value as given.
        value: String,
        /// The variable or flag that gave it.
        from: Source,
        /// What the value must be.
        expected: String,
    },
    /// A value given as text, as `efr config set` takes it, is not of its key's kind.
    #[error("{key} = {value:?} is not {expected}")]
    InvalidValue {
        /// The dotted key.
        key: String,
        /// The value as given.
        value: String,
        /// What the value must be.
        expected: String,
    },
    /// A key that the file does not have, or that the writer cannot change.
    #[error("{key} is not a key the writer can change")]
    UnknownKey {
        /// The dotted key as given.
        key: String,
    },
    /// The config file is a symbolic link to nothing, so the writer would create a file
    /// the user may not expect.
    #[error("the config file {} is a link to {}, which does not exist", .path.display(), .target.display())]
    DanglingSymlink {
        /// The link.
        path: PathBuf,
        /// What it points to.
        target: PathBuf,
    },
    /// The config file changed between the read and the write, so the change must be
    /// planned again.
    #[error("the config file {} changed while the change was prepared", .path.display())]
    Changed {
        /// The file that changed: the target of a link.
        path: PathBuf,
    },
    /// A directory for the config file could not be created.
    #[error("could not create the directory {}", .path.display())]
    CreateDir {
        /// The directory.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },
    /// The new config file could not be written.
    #[error("could not write the config file {}", .path.display())]
    Write {
        /// The file written: the target of a link.
        path: PathBuf,
        /// The error from `efr-stdx`.
        #[source]
        source: StdxError,
    },
    /// The settings could not be turned into TOML.
    #[error("the settings could not be written as TOML")]
    Encode {
        /// The serializer's error.
        #[source]
        source: toml_edit::ser::Error,
    },
}

impl ConfigError {
    /// Where in the file the error is, when it has a place.
    pub fn location(&self) -> Option<Location> {
        match self {
            ConfigError::Parse { location, .. }
            | ConfigError::ParseRule { location, .. }
            | ConfigError::InvalidRule { location, .. }
            | ConfigError::Invalid { location, .. } => *location,
            _ => None,
        }
    }

    /// The dotted key that holds the error, such as `shell.idle_minutes` or
    /// `permissions.rules[2]`, when the error belongs to one key.
    pub fn key(&self) -> Option<String> {
        match self {
            ConfigError::Parse { key, .. } => key.clone(),
            ConfigError::ParseRule { index, .. } | ConfigError::InvalidRule { index, .. } => {
                Some(format!("permissions.rules[{index}]"))
            }
            ConfigError::Invalid { key, .. } => Some((*key).to_owned()),
            ConfigError::InvalidOverride { key, .. }
            | ConfigError::InvalidValue { key, .. }
            | ConfigError::UnknownKey { key } => Some(key.clone()),
            _ => None,
        }
    }

    /// The error as `admin.status` and `admin.config_reload` report it: the message
    /// with its causes on one line, the place and the key.
    pub fn file_error(&self) -> ConfigFileError {
        let mut message = self.to_string();
        match self {
            // NOTE: the parser's Display quotes the file with a caret under the error;
            // the place is in `line` and `column`, so only its message is kept.
            ConfigError::Parse { source, .. } | ConfigError::ParseRule { source, .. } => {
                message.push_str(": ");
                message.push_str(source.message());
            }
            _ => {
                let mut source = std::error::Error::source(self);
                while let Some(cause) = source {
                    message.push_str(": ");
                    message.push_str(&cause.to_string());
                    source = cause.source();
                }
            }
        }
        let location = self.location();
        ConfigFileError {
            message: message.split_whitespace().collect::<Vec<_>>().join(" "),
            line: location.map(|at| at.line),
            column: location.map(|at| at.column),
            key: self.key(),
        }
    }
}

//! `[diff]`: the groups of your own that `efr` sorts the changed files of a turn into,
//! for the line at the end of a turn and the table of `efr diff --stat`.
//!
//! ```toml
//! [diff.groups]
//! evals = ["evals/", "*.eval.toml"]
//! ```
//!
//! A group of the file matches before the built-in groups of `efr` (generated, tests,
//! docs, else code), in the order of the file. TOML keeps no order in a table, so the
//! load takes the order from the document ([`DiffGroups::order_as`]).

mod glob;

use std::collections::BTreeMap;
use std::fmt;

use schemars::JsonSchema;
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub(crate) use glob::{check_glob, check_name};

/// `[diff]`: how `efr` groups the changed files of a turn. Only `efr` reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct DiffSettings {
    /// Groups of your own for the changed files of a turn: a name, then a list of
    /// globs, such as `evals = ["evals/", "*.eval.toml"]`. `*` matches inside one
    /// part of a path, `**` matches any number of parts, and `?` one character. A glob
    /// without `/` matches the name of a file in any directory; a glob that ends in `/`
    /// matches every file below a directory of that name; other globs match the whole
    /// path from the project root. Your groups match first, in the order of the file,
    /// then the built-in groups: generated, tests, docs, else code. A group named
    /// `code` keeps its files in code; `total` is not a name of a group.
    #[schemars(with = "BTreeMap<String, Vec<String>>")]
    pub groups: DiffGroups,
}

/// The groups of `[diff.groups]`, in the order of the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffGroups(Vec<(String, Vec<String>)>);

impl DiffGroups {
    /// The groups `groups`, each a name and its globs, in this order.
    pub fn new(groups: Vec<(String, Vec<String>)>) -> DiffGroups {
        DiffGroups(groups)
    }

    /// Every group, a name and its globs, in the order of the file.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.0.iter().map(|(name, globs)| (name.as_str(), globs.as_slice()))
    }

    /// True when the file names no group.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The name of the first group with a glob that matches `path`, a path as
    /// `efr_protocol::FileChange::path` gives it; `None` when no group matches.
    pub fn group_of(&self, path: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(_, globs)| globs.iter().any(|pattern| glob::matches(pattern, path)))
            .map(|(name, _)| name.as_str())
    }

    /// The groups in the order of `names`, the keys of the table as the file writes
    /// them; a group that `names` leaves out goes last.
    pub(crate) fn order_as(&mut self, names: &[String]) {
        self.0.sort_by_key(|(name, _)| {
            names.iter().position(|known| known == name).unwrap_or(usize::MAX)
        });
    }
}

impl Serialize for DiffGroups {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, globs) in &self.0 {
            map.serialize_entry(name, globs)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for DiffGroups {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Groups;

        impl<'de> Visitor<'de> for Groups {
            type Value = DiffGroups;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a table of group names, each with a list of globs")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<DiffGroups, A::Error> {
                let mut groups = Vec::new();
                while let Some((name, globs)) = map.next_entry::<String, Vec<String>>()? {
                    groups.push((name, globs));
                }
                Ok(DiffGroups(groups))
            }
        }

        deserializer.deserialize_map(Groups)
    }
}

#[cfg(test)]
mod tests;

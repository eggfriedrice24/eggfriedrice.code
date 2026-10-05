//! Changes of the registry file that keep its comments and layout, for `efr project add`
//! and `efr project remove`.
//!
//! The file is read whole and checked as [`Registry::load`] checks it, so a file with an
//! error is never changed: the user fixes it by hand first. A change edits the TOML
//! document in place, so every comment, blank line and key order the user wrote stays.
//! The write goes to the file behind a symbolic link, so a registry kept in a dotfiles
//! repository stays a link, and it is refused when the file changed since it was read.

use std::io;
use std::path::{Path, PathBuf};

use efr_protocol::ProjectId;
use efr_stdx::StdxError;
use efr_stdx::fs::LinkedFile;
use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

use crate::ScopeError;
use crate::home::normalize;
use crate::registry::{Project, Registry};

/// The key of the projects in the file.
const PROJECTS: &str = "project";

/// The first lines of a registry file that a change creates.
const HEADER: &str = "\
# The projects that efr knows. A turn whose shell is in a project's root, or below it,
# runs in that project: the auto mode writes freely below the root and runs the
# project's build, test and git commands there. `efr project add`, `efr project list`
# and `efr project remove` change this file and keep your comments.
";

/// The registry file as read, and the change made to it so far.
#[derive(Debug, Clone)]
pub struct RegistryEdit {
    /// The registry file as named and read, through a symbolic link.
    file: LinkedFile,
    /// The registry with the change applied.
    registry: Registry,
    /// The file with the change applied.
    document: DocumentMut,
}

impl RegistryEdit {
    /// Reads the registry file at `path` for a change. A missing file is an empty
    /// registry, which a write creates. Fails when the file cannot be read, has an error,
    /// or is a symbolic link to nothing.
    ///
    /// This blocks on the file system; async callers run it in `spawn_blocking`.
    pub fn open(path: &Path) -> Result<Self, ScopeError> {
        let file = LinkedFile::open(path).map_err(|error| match error {
            StdxError::DanglingLink { path, target } => {
                ScopeError::DanglingRegistryLink { path, target }
            }
            StdxError::ReadFile { path, source } => ScopeError::ReadRegistry { path, source },
            // NOTE: an open fails only to read; any other error is kept as the source.
            other => ScopeError::ReadRegistry {
                path: path.to_path_buf(),
                source: io::Error::other(other),
            },
        })?;
        let registry = match file.text() {
            Some(text) => Registry::from_toml(text, path)?,
            None => Registry::empty(),
        };
        let document = file
            .text()
            .unwrap_or(HEADER)
            .parse::<DocumentMut>()
            .map_err(|_| ScopeError::RegistryShape { path: path.to_path_buf() })?;
        Ok(RegistryEdit { file, registry, document })
    }

    /// The registry file as named.
    pub fn path(&self) -> &Path {
        self.file.path()
    }

    /// The file that [`save`](Self::save) writes: the end of the symbolic link, or the
    /// file itself.
    pub fn target(&self) -> &Path {
        self.file.target()
    }

    /// The registry with the changes made so far.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The new text of the file.
    pub fn text(&self) -> String {
        self.document.to_string()
    }

    /// Adds a project after the others, as [`Registry::register`] does, and to the file
    /// as a new `[[project]]` table.
    pub fn register(
        &mut self,
        id: ProjectId,
        root: impl Into<PathBuf>,
        name: Option<String>,
    ) -> Result<Project, ScopeError> {
        let project = self.registry.register(id, root, name)?.clone();
        let root = match project.root().to_str() {
            Some(root) => root.to_owned(),
            None => unreachable!("the registry refuses a root that is not UTF-8"),
        };
        let path = self.file.path().to_path_buf();
        let shape = || ScopeError::RegistryShape { path: path.clone() };
        // NOTE: a file of comments alone keeps them after its last table, so they would
        // end up below the first project; they go before it instead.
        let leading = if self.document.as_table().is_empty() {
            self.document.trailing().as_str().map(str::to_owned).unwrap_or_default()
        } else {
            String::new()
        };
        if !leading.is_empty() {
            self.document.set_trailing("");
        }
        let item =
            self.document.entry(PROJECTS).or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
        match item {
            Item::ArrayOfTables(projects) => {
                let mut table = Table::new();
                if !leading.is_empty() {
                    table.decor_mut().set_prefix(format!("{leading}\n"));
                }
                table.insert("id", toml_edit::value(id.to_string()));
                table.insert("root", toml_edit::value(root));
                if let Some(name) = project.name() {
                    table.insert("name", toml_edit::value(name));
                }
                projects.push(table);
            }
            Item::Value(Value::Array(projects)) => {
                let mut table = InlineTable::new();
                table.insert("id", id.to_string().into());
                table.insert("root", root.into());
                if let Some(name) = project.name() {
                    table.insert("name", name.into());
                }
                projects.push(table);
            }
            _ => return Err(shape()),
        }
        Ok(project)
    }

    /// Removes the project whose root is `root`, compared in lexical normal form, from
    /// the registry and from the file, and returns it; `None` when no project has that
    /// root. The comments inside the removed table, and those right above it, go with
    /// it; comments above it that a blank line separates from it stay, such as the
    /// comment at the top of the file.
    pub fn remove_root(&mut self, root: &Path) -> Result<Option<Project>, ScopeError> {
        let Some(root) = normalize(root) else {
            return Err(ScopeError::NotAbsolute { path: root.to_path_buf() });
        };
        let Some(id) =
            self.registry.projects().iter().find(|project| project.root() == root).map(Project::id)
        else {
            return Ok(None);
        };
        let path = self.file.path().to_path_buf();
        let shape = || ScopeError::RegistryShape { path: path.clone() };
        let is_it = |entry_id: Option<&str>| {
            entry_id.and_then(|text| text.parse::<ProjectId>().ok()) == Some(id)
        };
        let mut orphaned = String::new();
        match self.document.get_mut(PROJECTS) {
            Some(Item::ArrayOfTables(projects)) => {
                let index = projects
                    .iter()
                    .position(|table| is_it(table.get("id").and_then(Item::as_str)))
                    .ok_or_else(shape)?;
                let prefix = projects
                    .get(index)
                    .and_then(|table| table.decor().prefix())
                    .and_then(|raw| raw.as_str())
                    .unwrap_or_default()
                    .to_owned();
                projects.remove(index);
                let kept = kept_comments(&prefix);
                if !kept.trim().is_empty() {
                    match projects.get_mut(index) {
                        Some(next) => {
                            let own = next
                                .decor()
                                .prefix()
                                .and_then(|raw| raw.as_str())
                                .unwrap_or_default()
                                .trim_start_matches('\n')
                                .to_owned();
                            next.decor_mut().set_prefix(format!("{kept}{own}"));
                        }
                        None => orphaned = kept,
                    }
                }
            }
            Some(Item::Value(Value::Array(projects))) => {
                let index = projects
                    .iter()
                    .position(|entry| {
                        is_it(
                            entry
                                .as_inline_table()
                                .and_then(|table| table.get("id"))
                                .and_then(Value::as_str),
                        )
                    })
                    .ok_or_else(shape)?;
                projects.remove(index);
            }
            _ => return Err(shape()),
        }
        if !orphaned.is_empty() {
            let after = self.document.trailing().as_str().unwrap_or_default().to_owned();
            self.document.set_trailing(format!("{orphaned}{after}"));
        }
        Ok(self.registry.remove(&id))
    }

    /// Writes the changed file atomically (mode 0600) to [`target`](Self::target),
    /// creating its directory when it is missing. Fails with
    /// [`ScopeError::RegistryChanged`] when the file changed since [`open`](Self::open),
    /// so the caller reads it again.
    ///
    /// This blocks on the file system; async callers run it in `spawn_blocking`.
    pub fn save(&self) -> Result<(), ScopeError> {
        let text = self.text();
        // NOTE: the text is read back as a load reads it, so a change never writes a
        // file that the daemon would refuse.
        Registry::from_toml(&text, self.path())?;
        self.file.write_if_unchanged(text.as_bytes()).map_err(|error| match error {
            StdxError::FileChanged { path } => ScopeError::RegistryChanged { path },
            StdxError::CreateDir { path, source } => ScopeError::CreateDir { path, source },
            StdxError::ReadFile { path, source } => ScopeError::ReadRegistry { path, source },
            other => ScopeError::WriteRegistry { path: self.target().to_path_buf(), source: other },
        })
    }
}

/// The part of the comments and blank lines before a table that stays when the table
/// goes: everything up to its last blank line. The lines after that blank line sit
/// right above the table and belong to it.
fn kept_comments(prefix: &str) -> String {
    let lines: Vec<&str> = prefix.split_inclusive('\n').collect();
    match lines.iter().rposition(|line| line.trim().is_empty()) {
        Some(last_blank) => lines[..=last_blank].concat(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests;

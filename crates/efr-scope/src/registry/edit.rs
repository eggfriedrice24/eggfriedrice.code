//! Changes of the registry file that keep its comments and layout, for `efr project add`
//! and `efr project remove`.
//!
//! The file is read whole and checked as [`Registry::load`] checks it, so a file with an
//! error is never changed: the user fixes it by hand first. A change edits the TOML
//! document in place, so every comment, blank line and key order the user wrote stays.
//! The write goes to the file behind a symbolic link, so a registry kept in a dotfiles
//! repository stays a link, and it is refused when the file changed since it was read.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use efr_protocol::ProjectId;
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
    /// The registry file as named, which may be a symbolic link.
    path: PathBuf,
    /// The file that is written: the end of the link, or `path` itself.
    target: PathBuf,
    /// The contents as read; `None` when the file does not exist.
    text: Option<String>,
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
        let read_error = |source| ScopeError::ReadRegistry { path: path.to_path_buf(), source };
        let target = match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => match fs::canonicalize(path) {
                Ok(target) => target,
                Err(source) if source.kind() == io::ErrorKind::NotFound => {
                    let target = fs::read_link(path).map_err(read_error)?;
                    return Err(ScopeError::DanglingRegistryLink {
                        path: path.to_path_buf(),
                        target,
                    });
                }
                Err(source) => return Err(read_error(source)),
            },
            Ok(_) => path.to_path_buf(),
            Err(source) if source.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
            Err(source) => return Err(read_error(source)),
        };
        let text = read(&target).map_err(read_error)?;
        let registry = match &text {
            Some(text) => Registry::from_toml(text, path)?,
            None => Registry::empty(),
        };
        let document = text
            .as_deref()
            .unwrap_or(HEADER)
            .parse::<DocumentMut>()
            .map_err(|_| ScopeError::RegistryShape { path: path.to_path_buf() })?;
        Ok(RegistryEdit { path: path.to_path_buf(), target, text, registry, document })
    }

    /// The registry file as named.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file that [`save`](Self::save) writes: the end of the symbolic link, or the
    /// file itself.
    pub fn target(&self) -> &Path {
        &self.target
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
        let shape = || ScopeError::RegistryShape { path: self.path.clone() };
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
    /// root. The comments inside the removed table go with it.
    pub fn remove_root(&mut self, root: &Path) -> Result<Option<Project>, ScopeError> {
        let Some(root) = normalize(root) else {
            return Err(ScopeError::NotAbsolute { path: root.to_path_buf() });
        };
        let Some(id) =
            self.registry.projects().iter().find(|project| project.root() == root).map(Project::id)
        else {
            return Ok(None);
        };
        let shape = || ScopeError::RegistryShape { path: self.path.clone() };
        let is_it = |entry_id: Option<&str>| {
            entry_id.and_then(|text| text.parse::<ProjectId>().ok()) == Some(id)
        };
        match self.document.get_mut(PROJECTS) {
            Some(Item::ArrayOfTables(projects)) => {
                let index = projects
                    .iter()
                    .position(|table| is_it(table.get("id").and_then(Item::as_str)))
                    .ok_or_else(shape)?;
                projects.remove(index);
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
        Registry::from_toml(&text, &self.path)?;
        let changed = || ScopeError::RegistryChanged { path: self.target.clone() };
        let now = read(&self.target)
            .map_err(|source| ScopeError::ReadRegistry { path: self.target.clone(), source })?;
        if now != self.text {
            return Err(changed());
        }
        if self.text.is_none() {
            // A link or a file that appeared since the read is somebody else's change.
            match fs::symlink_metadata(&self.path) {
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                _ => return Err(changed()),
            }
            if let Some(dir) = self.target.parent().filter(|dir| !dir.as_os_str().is_empty()) {
                fs::create_dir_all(dir)
                    .map_err(|source| ScopeError::CreateDir { path: dir.to_path_buf(), source })?;
            }
        }
        efr_stdx::fs::write_atomic(&self.target, text.as_bytes())
            .map_err(|source| ScopeError::WriteRegistry { path: self.target.clone(), source })
    }
}

/// The contents of `path`; `None` when it does not exist.
fn read(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(source),
    }
}

#[cfg(test)]
mod tests;

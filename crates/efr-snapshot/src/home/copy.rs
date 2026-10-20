//! One copy of home paths: the entries that the daemon reads, streamed into `git
//! fast-import` of the home store, with a manifest `efr-home.json` that names the
//! state of each path. git never reads a home path itself, and no filter runs.

use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Component, Path, PathBuf};

use efr_protocol::UndoReason;
use tokio::sync::mpsc;

use crate::SnapshotError;
use crate::home::{HomeEntry, HomeReader, HomeState};
use crate::object::{blob_id, link_id};

/// The largest file or state that a copy takes; a larger one is not covered.
pub const MAX_HOME_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// The name of the manifest in the tree of a copy.
pub(crate) const MANIFEST: &str = "efr-home.json";

/// The bytes of one part of the stream; a file goes in parts of this size, so at most
/// one file is in memory while git reads.
const PART: usize = 64 * 1024;

/// What kind of state a path has in a copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    File,
    Link,
    Missing,
    State,
    NotCovered,
}

/// One path in the manifest of a copy.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Item {
    pub(crate) path: String,
    pub(crate) kind: Kind,
    /// The mode of a file (`& 0o7777`).
    pub(crate) mode: Option<u32>,
    /// The blob of a file or a state, or of a link's target.
    pub(crate) blob: Option<String>,
    pub(crate) floor: bool,
    /// The links between a floor path and its real file, for the report.
    pub(crate) chain: Vec<String>,
    /// Why the path is not covered.
    pub(crate) reason: Option<UndoReason>,
    /// The directories above the path that were missing before the call that first
    /// named it, the deepest first ([`HomeEntry::missing_parents`]). An older copy has
    /// none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) parents: Vec<String>,
}

impl Default for Item {
    fn default() -> Self {
        Item {
            path: String::new(),
            kind: Kind::Missing,
            mode: None,
            blob: None,
            floor: false,
            chain: Vec::new(),
            reason: None,
            parents: Vec::new(),
        }
    }
}

impl Item {
    /// True when `other` is the same state of the path.
    pub(crate) fn same_state(&self, other: &Item) -> bool {
        self.kind == other.kind
            && self.kind != Kind::NotCovered
            && self.blob == other.blob
            && self.mode == other.mode
    }
}

/// The manifest of a copy: the phase (`before`, `after` or `undo-before`) and each
/// path.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Manifest {
    pub(crate) phase: String,
    pub(crate) entries: Vec<Item>,
}

impl Manifest {
    pub(crate) fn get(&self, path: &str) -> Option<&Item> {
        self.entries.iter().find(|item| item.path == path)
    }
}

/// The prefixes of the NTFS short names of `.gitmodules`, which git refuses for a link.
const SHORT_GITMODULES: [&[u8]; 2] = [b"gitmod~", b"gi7eba~"];

/// True when git refuses `name` as a part of a path in a tree, so no copy can hold a
/// path with it: `.git` in any case, and the names that mean `.git` on NTFS, which git
/// refuses on every system (`git~1`, `.git.`, `.git `, `.git::$INDEX_ALLOCATION`).
/// With `link` (the path is a symbolic link), also `.gitmodules` and its NTFS short
/// names. A home copy leaves such a path out, as a snapshot leaves out a nested
/// repository; it never escapes the name.
pub fn git_refuses(name: &OsStr, link: bool) -> bool {
    let name = name.as_bytes().to_ascii_lowercase();
    let gitmodules = || {
        means(&name, b".gitmodules") || SHORT_GITMODULES.iter().any(|short| name.starts_with(short))
    };
    means(&name, b".git") || name == b"git~1" || (link && gitmodules())
}

/// True when `name` is `base` with only dots and spaces after it, up to its end, an
/// NTFS stream (`:`) or an NTFS separator (`\`): NTFS drops what follows.
fn means(name: &[u8], base: &[u8]) -> bool {
    let Some(rest) = name.strip_prefix(base) else { return false };
    rest.iter()
        .find(|byte| !matches!(byte, b'.' | b' '))
        .is_none_or(|byte| matches!(byte, b':' | b'\\'))
}

/// The path of `path` in the tree of a copy: `files/<path without its leading />` for
/// an absolute path, `state/<name>` for `@<name>`. `Ok(None)` for a name that is not
/// UTF-8 or that git refuses ([`git_refuses`]; `link` for a link), which no copy
/// holds.
pub(crate) fn tree_path(path: &Path, link: bool) -> Result<Option<String>, SnapshotError> {
    let bad = || SnapshotError::BadHomeEntry { path: path.to_path_buf() };
    let Some(text) = path.to_str() else {
        return if path.is_absolute() { Ok(None) } else { Err(bad()) };
    };
    if let Some(name) = text.strip_prefix('@') {
        let plain = !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_lowercase());
        return if plain { Ok(Some(format!("state/{name}"))) } else { Err(bad()) };
    }
    let mut parts = path.components();
    if parts.next() != Some(Component::RootDir) {
        return Err(bad());
    }
    let mut out = String::from("files");
    let mut any = false;
    for part in parts {
        let Component::Normal(name) = part else { return Err(bad()) };
        out.push('/');
        out.push_str(name.to_str().ok_or_else(bad)?);
        any = true;
    }
    // NOTE: `components` drops a `.` and a doubled `/`; the manifest names the path as
    // it is written, so only a path in its plain form has one place.
    if !any || out.strip_prefix("files") != Some(text) {
        return Err(bad());
    }
    let refused = path.components().any(|part| match part {
        Component::Normal(name) => git_refuses(name, link),
        _ => false,
    });
    Ok((!refused).then_some(out))
}

/// `path` in the C-style quotes of `fast-import`, so any byte but NUL is safe.
fn quote(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 2);
    out.push('"');
    for ch in path.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            ch if (ch as u32) < 0x20 || ch == '\u{7f}' => {
                out.push_str(&format!("\\{:03o}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// The commit that a copy writes.
#[derive(Debug, Clone)]
pub(crate) struct CommitPlan {
    /// The ref that the commit moves.
    pub(crate) name: String,
    /// The parent, for an "after" copy.
    pub(crate) from: Option<String>,
    /// The message, with its `efr-meta:` line.
    pub(crate) message: String,
    /// git's `<seconds> +0000`.
    pub(crate) time: String,
    /// The manifest's phase.
    pub(crate) phase: String,
}

/// The `fast-import` stream of one copy, sent in parts.
struct Stream {
    sender: mpsc::Sender<Vec<u8>>,
    marks: u32,
    /// Each blob of the tree: its mark, git's mode and its path in the tree.
    files: Vec<(u32, u32, String)>,
}

impl Stream {
    fn send(&self, part: Vec<u8>) -> Result<(), SnapshotError> {
        self.sender.blocking_send(part).map_err(|_| SnapshotError::RunGit {
            program: "git".into(),
            source: std::io::Error::from(std::io::ErrorKind::BrokenPipe),
        })
    }

    /// Sends a blob of `bytes`; returns its mark.
    fn blob(&mut self, bytes: &[u8]) -> Result<u32, SnapshotError> {
        self.marks += 1;
        let mark = self.marks;
        self.send(format!("blob\nmark :{mark}\ndata {}\n", bytes.len()).into_bytes())?;
        for part in bytes.chunks(PART) {
            self.send(part.to_vec())?;
        }
        self.send(b"\n".to_vec())?;
        Ok(mark)
    }
}

/// Reads at most [`MAX_HOME_FILE_BYTES`] of `reader`; `None` when there are more.
fn read_capped(reader: &mut (dyn Read + Send)) -> Result<Option<Vec<u8>>, std::io::Error> {
    let mut bytes = Vec::new();
    reader.take(MAX_HOME_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= MAX_HOME_FILE_BYTES).then_some(bytes))
}

/// Streams the entries of `reader` and then the commit of `plan` to `sender`; returns
/// the manifest's items. Runs on the blocking pool: the reader reads files.
pub(crate) fn produce(
    reader: &mut dyn HomeReader,
    plan: &CommitPlan,
    sender: mpsc::Sender<Vec<u8>>,
) -> Result<Vec<Item>, SnapshotError> {
    let mut stream = Stream { sender, marks: 0, files: Vec::new() };
    let mut items: Vec<Item> = Vec::new();
    while let Some(next) = reader.next() {
        let (entry, bytes) = next?;
        let path = entry.path.clone();
        let item = take(&mut stream, entry, bytes)?;
        if items.iter().any(|known| known.path == item.path) {
            tracing::debug!(path = %path.display(), "a home copy named a path twice; the first stays");
            continue;
        }
        items.push(item);
    }
    let manifest = Manifest { phase: plan.phase.clone(), entries: items.clone() };
    let text = serde_json::to_vec(&manifest)
        .map_err(|_| SnapshotError::BadOutput { command: "fast-import" })?;
    let manifest_mark = stream.blob(&text)?;
    let mut commit = format!(
        "commit {}\ncommitter efr <efr@localhost> {}\ndata {}\n{}\n",
        plan.name,
        plan.time,
        plan.message.len(),
        plan.message
    );
    if let Some(from) = &plan.from {
        commit.push_str(&format!("from {from}\n"));
    }
    commit.push_str("deleteall\n");
    for (mark, mode, path) in &stream.files {
        commit.push_str(&format!("M {mode:o} :{mark} {}\n", quote(path)));
    }
    commit.push_str(&format!("M 100644 :{manifest_mark} {MANIFEST}\n\ndone\n"));
    stream.send(commit.into_bytes())?;
    Ok(items)
}

/// The manifest item of `entry`, with its blob sent when it has one.
fn take(
    stream: &mut Stream,
    entry: HomeEntry,
    bytes: Option<Box<dyn Read + Send>>,
) -> Result<Item, SnapshotError> {
    let tree = tree_path(&entry.path, matches!(entry.state, HomeState::Link { .. }))?;
    let lossy = |path: &PathBuf| path.to_string_lossy().into_owned();
    let mut item = Item {
        path: entry.path.to_string_lossy().into_owned(),
        floor: entry.floor,
        chain: entry.chain.iter().map(lossy).collect(),
        parents: entry.missing_parents.iter().map(lossy).collect(),
        ..Item::default()
    };
    let not_covered = |mut item: Item, reason: UndoReason| {
        item.kind = Kind::NotCovered;
        item.reason = Some(reason);
        item
    };
    let Some(tree) = tree else { return Ok(not_covered(item, UndoReason::LeftOut)) };
    let read = |bytes: Option<Box<dyn Read + Send>>| -> Result<Option<Vec<u8>>, SnapshotError> {
        let Some(mut bytes) = bytes else { return Ok(None) };
        read_capped(bytes.as_mut())
            .map_err(|source| SnapshotError::Io { path: entry.path.clone(), source })
    };
    match entry.state {
        HomeState::File { .. } | HomeState::State if bytes.is_none() => {
            Ok(not_covered(item, UndoReason::Unknown))
        }
        HomeState::File { mode } => {
            let Some(content) = read(bytes)? else {
                return Ok(not_covered(item, UndoReason::OverLimit));
            };
            let mark = stream.blob(&content)?;
            let git_mode = if mode & 0o111 != 0 { 0o100_755 } else { 0o100_644 };
            stream.files.push((mark, git_mode, tree));
            item.kind = Kind::File;
            item.mode = Some(mode & 0o7777);
            item.blob = Some(blob_id(&content).to_string());
            Ok(item)
        }
        HomeState::State => {
            let Some(content) = read(bytes)? else {
                return Ok(not_covered(item, UndoReason::OverLimit));
            };
            let mark = stream.blob(&content)?;
            stream.files.push((mark, 0o100_644, tree));
            item.kind = Kind::State;
            item.blob = Some(blob_id(&content).to_string());
            Ok(item)
        }
        HomeState::Link { target } => {
            let mark = stream.blob(&target)?;
            stream.files.push((mark, 0o120_000, tree));
            item.kind = Kind::Link;
            item.blob = Some(link_id(&target).to_string());
            Ok(item)
        }
        HomeState::Missing => Ok(item),
        HomeState::NotCovered(reason) => Ok(not_covered(item, reason)),
    }
}

#[cfg(test)]
mod tests;

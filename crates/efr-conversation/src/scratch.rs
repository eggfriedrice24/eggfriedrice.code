//! `$SCRATCH`: one directory per conversation for the model's own files.
//!
//! The directory lives under the scratch root (`$XDG_DATA_HOME/efr/scratch/`) as
//! `<YYYY-MM-DD>-<slug>-<idtail>`: the day the conversation began, up to five words of
//! its title, and the end of its id. It is claimed with a non-recursive `mkdir`
//! ([`efr_stdx::fs::claim_dir`]), where "already exists" means taken, and marked with a
//! small file that holds the conversation id. The name is a function of the
//! conversation, so after a restart the same names are tried in the same order and the
//! marked one is found again without a scan; a name taken by anything else falls back
//! to a longer id tail, then the whole id, then the whole id with a counter.
//!
//! The directory is kept when the conversation ends, and made again lazily, under the
//! same name, when it goes missing. Everything here blocks on the file system; the turn
//! runs it in `spawn_blocking`.

use std::fs::DirBuilder;
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};

use efr_protocol::ConversationId;
use efr_stdx::fs::{Claim, claim_dir, write_atomic};
use jiff::civil::Date;

use crate::ConversationError;

/// The file in a scratch directory that names its conversation.
const MARKER: &str = ".efr-conversation";

/// The most words of the title in a name.
const SLUG_WORDS: usize = 5;

/// The longest slug, in bytes.
const SLUG_MAX: usize = 48;

/// The id tails tried in order, in hex digits. A UUIDv7 ends in random bits, so eight
/// digits rarely collide and the whole id never does.
const TAILS: [usize; 3] = [8, 12, 32];

/// The counters tried after the whole id, for a name that something else holds.
const COUNTERS: std::ops::RangeInclusive<u32> = 2..=9;

/// The mode of the scratch root and of every scratch directory.
const DIR_MODE: u32 = 0o700;

/// The scratch directory of one conversation.
#[derive(Debug, Clone)]
pub(crate) struct Scratch {
    root: PathBuf,
    conversation_id: ConversationId,
    /// The directory once found or claimed.
    path: Option<PathBuf>,
}

impl Scratch {
    /// The scratch directory of `conversation_id` under `root`, not looked for yet.
    pub(crate) fn new(root: impl Into<PathBuf>, conversation_id: ConversationId) -> Self {
        Scratch { root: root.into(), conversation_id, path: None }
    }

    /// The directory, found, claimed, or made again when it went missing.
    ///
    /// `began` and `title` are the day the conversation began and its title; they give
    /// the name, so they must be the same on every call for one conversation.
    pub(crate) fn ensure(
        &mut self,
        began: Date,
        title: Option<&str>,
    ) -> Result<PathBuf, ConversationError> {
        if let Some(path) = self.path.clone() {
            if self.is_ours(&path) {
                return Ok(path);
            }
            if !exists(&path) {
                self.create_root()?;
                if self.claim(&path)? {
                    return Ok(path);
                }
            }
        }
        self.create_root()?;
        let slug = slug(title.unwrap_or_default());
        for name in names(began, &slug, self.conversation_id) {
            let path = self.root.join(name);
            if self.is_ours(&path) || self.claim(&path)? {
                self.path = Some(path.clone());
                return Ok(path);
            }
        }
        Err(ConversationError::ScratchNamesExhausted { root: self.root.clone() })
    }

    fn create_root(&self) -> Result<(), ConversationError> {
        DirBuilder::new().recursive(true).mode(DIR_MODE).create(&self.root).map_err(|source| {
            ConversationError::CreateScratchRoot { path: self.root.clone(), source }
        })
    }

    /// Claims `path` and marks it; false when something else is there.
    fn claim(&self, path: &Path) -> Result<bool, ConversationError> {
        let claim_error =
            |source| ConversationError::ClaimScratch { path: path.to_path_buf(), source };
        match claim_dir(path).map_err(claim_error)? {
            Claim::Taken => Ok(false),
            Claim::Claimed => {
                let marker = format!("{}\n", self.conversation_id);
                write_atomic(&path.join(MARKER), marker.as_bytes()).map_err(claim_error)?;
                Ok(true)
            }
        }
    }

    /// True when `path` is a directory, not a symbolic link, whose marker names this
    /// conversation. A link is refused because the permission engine treats every path
    /// under `$SCRATCH` as free to write, and a link would extend that to its target.
    fn is_ours(&self, path: &Path) -> bool {
        let is_dir = path.symlink_metadata().is_ok_and(|meta| meta.is_dir());
        is_dir
            && std::fs::read_to_string(path.join(MARKER))
                .is_ok_and(|marker| marker.trim() == self.conversation_id.to_string())
    }
}

/// True when anything, a dangling link included, is at `path`.
fn exists(path: &Path) -> bool {
    match path.symlink_metadata() {
        Ok(_) => true,
        Err(error) => error.kind() != io::ErrorKind::NotFound,
    }
}

/// The names the directory of `conversation_id` may take, in the order they are tried.
pub(crate) fn names(began: Date, slug: &str, conversation_id: ConversationId) -> Vec<String> {
    let hex: String = conversation_id.to_string().chars().filter(|c| *c != '-').collect();
    let base = |tail: &str| match slug {
        "" => format!("{began}-{tail}"),
        slug => format!("{began}-{slug}-{tail}"),
    };
    let tails = TAILS.iter().map(|len| base(&hex[hex.len().saturating_sub(*len)..]));
    let counted = COUNTERS.map(|counter| format!("{}-{counter}", base(&hex)));
    tails.chain(counted).collect()
}

/// Up to five words of `title`, lowercase ASCII letters and digits joined by `-`, at
/// most 48 bytes. Any other character separates words; a title without letters or
/// digits gives an empty slug.
pub(crate) fn slug(title: &str) -> String {
    let mut slug = String::new();
    let words = title.split(|c: char| !c.is_ascii_alphanumeric()).filter(|word| !word.is_empty());
    for word in words.take(SLUG_WORDS) {
        let word = word.to_ascii_lowercase();
        let separator = usize::from(!slug.is_empty());
        if slug.len() + separator + word.len() > SLUG_MAX {
            if slug.is_empty() {
                // NOTE: the word is ASCII, so any byte index is a character boundary.
                slug.push_str(&word[..SLUG_MAX]);
            }
            break;
        }
        if separator == 1 {
            slug.push('-');
        }
        slug.push_str(&word);
    }
    slug
}

#[cfg(test)]
mod tests;

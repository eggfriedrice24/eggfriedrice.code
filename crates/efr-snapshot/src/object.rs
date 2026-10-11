//! [`ObjectId`]: the id of a git object, and the ids that git gives the bytes of a file
//! and the target of a link, computed here, so a caller can compare a file with a
//! snapshot without running git.

use std::fmt;

use sha1::{Digest as _, Sha1};

/// The id of a git object: 40 lowercase hex characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(String);

impl ObjectId {
    /// `text` as an object id, when it has 40 hex characters; upper case reads as lower.
    pub fn parse(text: &str) -> Option<ObjectId> {
        let text = text.trim();
        (text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| ObjectId(text.to_ascii_lowercase()))
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The id that git gives a file with `bytes`: the SHA-1 of `blob <len>\0` and the
/// bytes, which a snapshot keeps exactly (no filter, no line-end conversion).
pub fn blob_id(bytes: &[u8]) -> ObjectId {
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", bytes.len()).as_bytes());
    hasher.update(bytes);
    let digest = hasher.finalize();
    ObjectId(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The id that git gives a symbolic link to `target`: the blob of the target's bytes.
pub fn link_id(target: &[u8]) -> ObjectId {
    blob_id(target)
}

#[cfg(test)]
mod tests;

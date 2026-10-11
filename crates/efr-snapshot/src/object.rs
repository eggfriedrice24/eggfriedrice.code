//! [`ObjectId`]: the id of a git object, and the ids that git gives the bytes of a file
//! and the target of a link, computed here, so a caller can compare a file with a
//! snapshot without running git.

use std::fmt;
use std::io::{self, Read};

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

/// The blob id of the `len` bytes that `reader` gives, read in parts, so a large file
/// never sits in memory whole. `None` when the reader gives more or fewer bytes.
pub(crate) fn blob_id_of(len: u64, reader: &mut impl Read) -> io::Result<Option<ObjectId>> {
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {len}\0").as_bytes());
    let mut chunk = vec![0_u8; 64 * 1024];
    let mut seen = 0_u64;
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        seen = seen.saturating_add(read as u64);
        if seen > len {
            return Ok(None);
        }
        hasher.update(&chunk[..read]);
    }
    if seen != len {
        return Ok(None);
    }
    let digest = hasher.finalize();
    Ok(Some(ObjectId(digest.iter().map(|byte| format!("{byte:02x}")).collect())))
}

#[cfg(test)]
mod tests;

//! The base64 text of the blobs that the turns send, by conversation and hash, within a
//! limit of bytes.
//!
//! A turn fills the bytes of each attachment once, and each request of the turn shares
//! them, so the cache serves the next turns. The oldest entry goes first when a new one
//! does not fit; an entry larger than the whole cache is never kept.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, PoisonError};

use efr_protocol::ConversationId;
use efr_provider::EncodedBytes;

/// The most bytes of base64 that the cache holds: more than three images of the
/// largest size that a model gets.
pub(crate) const CACHE_BYTES: usize = 32 * 1024 * 1024;

/// The cache of the base64 text of the blobs.
#[derive(Debug)]
pub(crate) struct Cache {
    limit: usize,
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    /// The text of each blob, by conversation and SHA-256 of the original bytes.
    entries: HashMap<ConversationId, HashMap<String, EncodedBytes>>,
    /// The keys, oldest first.
    order: VecDeque<(ConversationId, String)>,
    bytes: usize,
}

impl Cache {
    /// A cache of at most `limit` bytes.
    pub(crate) fn new(limit: usize) -> Self {
        Cache { limit, inner: Mutex::default() }
    }

    /// The text of the blob `sha256` of `conversation_id`, when the cache holds it.
    pub(crate) fn get(
        &self,
        conversation_id: ConversationId,
        sha256: &str,
    ) -> Option<EncodedBytes> {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.entries.get(&conversation_id)?.get(sha256).cloned()
    }

    /// Keeps `data` as the text of the blob `sha256` of `conversation_id`, and lets the
    /// oldest entries go until it fits.
    pub(crate) fn insert(&self, conversation_id: ConversationId, sha256: &str, data: EncodedBytes) {
        if data.len() > self.limit {
            return;
        }
        let mut guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let inner = &mut *guard;
        if inner.entries.get(&conversation_id).is_some_and(|blobs| blobs.contains_key(sha256)) {
            return;
        }
        while inner.bytes + data.len() > self.limit {
            let Some((conversation, sha)) = inner.order.pop_front() else { break };
            if let Some(blobs) = inner.entries.get_mut(&conversation) {
                if let Some(text) = blobs.remove(&sha) {
                    inner.bytes -= text.len();
                }
                if blobs.is_empty() {
                    inner.entries.remove(&conversation);
                }
            }
        }
        inner.bytes += data.len();
        inner.order.push_back((conversation_id, sha256.to_owned()));
        inner.entries.entry(conversation_id).or_default().insert(sha256.to_owned(), data);
    }
}

#[cfg(test)]
mod tests;

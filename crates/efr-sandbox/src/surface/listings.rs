//! [`ConfigListings`]: what git listed of each config content, so that the surface
//! guard runs git only for a content that it has not listed yet.
//!
//! A listing depends on the content alone: the launcher gives git the bytes that the
//! guard read, on stdin, without includes and in a fixed environment
//! ([`ConfigLister::list_content`]). So the capture after a call lists only the configs
//! that the call changed, and the listings of one call serve the next calls of the same
//! hidden shell, through a file in the shell's dir, where no call can write.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::surface::ConfigLister;

/// The file in the hidden shell's sandbox dir that keeps the listings between calls.
pub const CONFIG_LISTINGS_FILE: &str = "config-listings.json";

/// The most bytes of [`CONFIG_LISTINGS_FILE`]. Contents that do not fit are listed again
/// in the next call.
pub const MAX_CONFIG_LISTINGS_BYTES: usize = 1024 * 1024;

/// The listing of each config content that git listed, for one git.
#[derive(Debug, Clone, Default)]
pub struct ConfigListings {
    git: String,
    listings: HashMap<Vec<u8>, Listing>,
}

#[derive(Debug, Clone)]
struct Listing {
    text: String,
    used: bool,
}

/// [`CONFIG_LISTINGS_FILE`] as JSON.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    git: String,
    configs: Vec<StoredConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredConfig {
    content: String,
    listing: String,
}

impl ConfigListings {
    /// No listings yet, of the git that `git` names: its path and what identifies the
    /// file, so a listing of another git is never used.
    pub fn new(git: impl Into<String>) -> Self {
        ConfigListings { git: git.into(), listings: HashMap::new() }
    }

    /// The listings of [`CONFIG_LISTINGS_FILE`] when they are of `git`; none when the
    /// file is of another git, too large, or does not parse.
    pub fn from_json(bytes: &[u8], git: &str) -> Self {
        let mut found = ConfigListings::new(git);
        if bytes.len() > MAX_CONFIG_LISTINGS_BYTES {
            return found;
        }
        let Ok(stored) = serde_json::from_slice::<Stored>(bytes) else { return found };
        if stored.git != git {
            return found;
        }
        for config in stored.configs {
            let listing = Listing { text: config.listing, used: false };
            found.listings.insert(config.content.into_bytes(), listing);
        }
        found
    }

    /// The listings that the captures since [`from_json`](Self::from_json) used or
    /// added, as JSON of at most [`MAX_CONFIG_LISTINGS_BYTES`]: a content that is not
    /// UTF-8, or that does not fit, is left out.
    pub fn to_json(&self) -> Vec<u8> {
        let mut used: Vec<(&Vec<u8>, &Listing)> =
            self.listings.iter().filter(|(_, listing)| listing.used).collect();
        // Sorted, so the same listings always give the same file.
        used.sort_by_key(|(content, _)| *content);
        let mut stored = Stored { git: self.git.clone(), configs: Vec::new() };
        let mut size = self.git.len();
        for (content, listing) in used {
            let Ok(content) = String::from_utf8(content.clone()) else { continue };
            // NOTE: JSON can write a byte as six; a bound on that keeps the file small.
            let cost = 6 * (content.len() + listing.text.len()) + 64;
            if size + cost > MAX_CONFIG_LISTINGS_BYTES {
                continue;
            }
            size += cost;
            stored.configs.push(StoredConfig { content, listing: listing.text.clone() });
        }
        serde_json::to_vec(&stored).unwrap_or_default()
    }

    /// The listing of `content`: the known one, or the one that `lister` gives, which is
    /// kept. `None` when git could not list it; that is never kept, so the next capture
    /// asks git again.
    pub(crate) fn listing(&mut self, content: &[u8], lister: &dyn ConfigLister) -> Option<&str> {
        if !self.listings.contains_key(content) {
            let text = lister.list_content(content).ok()?;
            self.listings.insert(content.to_vec(), Listing { text, used: false });
        }
        let listing = self.listings.get_mut(content)?;
        listing.used = true;
        Some(&listing.text)
    }
}

#[cfg(test)]
mod tests;

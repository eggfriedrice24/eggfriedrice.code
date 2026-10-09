//! The cache file of the model catalog, so a start while offline offers the last list
//! that the API sent.
//!
//! The file is JSON: `version`, `base_url` (the API that sent the list), `fetched_at`
//! and `models`, the entries in the API's own form. It has a name of its own beside
//! the OpenAI catalog's file, and it is written in one step with mode 0600. A file of
//! another version or of another base URL is not used. The functions block; the daemon
//! calls them off its async workers.

use std::fs::DirBuilder;
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::Path;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{CatalogEntry, CatalogOrigin, entries_of};
use crate::{AnthropicError, Catalog};

/// The version of the file's form. A file of another version is not used.
const VERSION: u32 = 1;

/// The file's members besides the models.
#[derive(Debug, Serialize, Deserialize)]
struct Header {
    version: u32,
    base_url: String,
    fetched_at: Timestamp,
}

/// The whole file, as it is written.
#[derive(Debug, Serialize)]
struct File<'a> {
    #[serde(flatten)]
    header: Header,
    models: &'a [CatalogEntry],
}

/// The catalog in the file at `path` for the API at `base_url`, as a catalog from the
/// cache. `None` when there is no file, or when it is of another version or of another
/// base URL. Fails when the file cannot be read or is not such a file.
pub fn read_cache(path: &Path, base_url: &str) -> Result<Option<Catalog>, AnthropicError> {
    let text = match std::fs::read(path) {
        Ok(text) => text,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(AnthropicError::CacheRead { path: path.to_path_buf(), source });
        }
    };
    let parse = |source| AnthropicError::CacheParse { path: path.to_path_buf(), source };
    let value: Value = serde_json::from_slice(&text).map_err(parse)?;
    let header = Header::deserialize(&value).map_err(parse)?;
    let base_url = base_url.trim_end_matches('/');
    if header.version != VERSION || header.base_url.trim_end_matches('/') != base_url {
        return Ok(None);
    }
    let Some(list) = value.get("models") else {
        return Ok(None);
    };
    let (entries, broken) = entries_of(list);
    Ok(Some(Catalog {
        listed: entries.len() + broken,
        entries,
        origin: CatalogOrigin::Cache,
        fetched_at: header.fetched_at,
        base_url: header.base_url,
    }))
}

/// Writes `catalog` to `path` in one step with mode 0600, so a reader sees the old file
/// or the new one, never a part. The directory is made, with mode 0700, when it is
/// missing.
pub fn write_cache(path: &Path, catalog: &Catalog) -> Result<(), AnthropicError> {
    let file = File {
        header: Header {
            version: VERSION,
            base_url: catalog.base_url().to_owned(),
            fetched_at: catalog.fetched_at(),
        },
        models: catalog.entries(),
    };
    let text = serde_json::to_vec_pretty(&file)
        .map_err(|source| AnthropicError::CacheEncode { path: path.to_path_buf(), source })?;
    if let Some(dir) = path.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|source| AnthropicError::CacheDir { path: dir.to_path_buf(), source })?;
    }
    efr_stdx::fs::write_atomic(path, &text)
        .map_err(|source| AnthropicError::CacheWrite { path: path.to_path_buf(), source })
}

#[cfg(test)]
mod tests;

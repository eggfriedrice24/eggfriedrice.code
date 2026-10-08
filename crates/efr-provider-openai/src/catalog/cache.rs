//! The cache file of the model catalog, so a start while offline offers the last list
//! that the backend sent, and a fetch can ask with its tag.
//!
//! The file is JSON: `version`, `base_url` (the backend that sent the list),
//! `client_version` (the efr that asked), `fetched_at`, `etag` and `models`, the
//! entries in the backend's own form. A file of another version or of another backend
//! is not used. The functions block; the daemon calls them off its async workers.

use std::fs::DirBuilder;
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::Path;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Catalog, CatalogEntry, CatalogOrigin, entries_of};
use crate::OpenAiError;
use crate::config::Backend;

/// The version of the file's form. A file of another version is not used.
const VERSION: u32 = 1;

/// The file's members besides the models.
#[derive(Debug, Serialize, Deserialize)]
struct Header {
    version: u32,
    base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_version: Option<String>,
    fetched_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
}

/// The whole file, as it is written.
#[derive(Debug, Serialize)]
struct File<'a> {
    #[serde(flatten)]
    header: Header,
    models: &'a [CatalogEntry],
}

/// The catalog in the file at `path` for the backend at `base_url`, as a catalog from
/// the cache. `None` when there is no file, or when it is of another version or of
/// another backend. Fails when the file cannot be read or is not such a file.
pub fn read_cache(
    path: &Path,
    backend: Backend,
    base_url: &str,
) -> Result<Option<Catalog>, OpenAiError> {
    let text = match std::fs::read(path) {
        Ok(text) => text,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(OpenAiError::CacheRead { path: path.to_path_buf(), source });
        }
    };
    let parse = |source| OpenAiError::CacheParse { path: path.to_path_buf(), source };
    let value: Value = serde_json::from_slice(&text).map_err(parse)?;
    let header = Header::deserialize(&value).map_err(parse)?;
    if header.version != VERSION || header.base_url.trim_end_matches('/') != base_url {
        return Ok(None);
    }
    let Some((entries, _broken)) = entries_of(&value) else {
        return Ok(None);
    };
    Ok(Some(Catalog {
        backend,
        entries,
        origin: CatalogOrigin::Cache,
        fetched_at: Some(header.fetched_at),
        etag: header.etag,
        base_url: Some(header.base_url),
        client_version: header.client_version,
    }))
}

/// Writes `catalog` to `path` in one step, so a reader sees the old file or the new
/// one. The directory is made when it is missing. A catalog that no backend sent, such
/// as the built-in table, is not written.
pub fn write_cache(path: &Path, catalog: &Catalog) -> Result<(), OpenAiError> {
    let (Some(base_url), Some(fetched_at)) = (&catalog.base_url, catalog.fetched_at) else {
        return Ok(());
    };
    let file = File {
        header: Header {
            version: VERSION,
            base_url: base_url.clone(),
            client_version: catalog.client_version.clone(),
            fetched_at,
            etag: catalog.etag.clone(),
        },
        models: &catalog.entries,
    };
    let text = serde_json::to_vec_pretty(&file)
        .map_err(|source| OpenAiError::CacheEncode { path: path.to_path_buf(), source })?;
    if let Some(dir) = path.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|source| OpenAiError::CacheDir { path: dir.to_path_buf(), source })?;
    }
    efr_stdx::fs::write_atomic(path, &text)
        .map_err(|source| OpenAiError::CacheWrite { path: path.to_path_buf(), source })
}

#[cfg(test)]
mod tests;

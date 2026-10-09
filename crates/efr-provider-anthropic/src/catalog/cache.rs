//! The cache file of the model catalog, so a start while offline offers the last list
//! that the API sent.
//!
//! The file is JSON: `version`, `base_url` (the API that sent the list), `fetched_at`
//! and `models`, the entries in the API's own form. It has a name of its own beside
//! the OpenAI catalog's file, and it is written in one step with mode 0600. A file of
//! another version or of another base URL is not used. The functions block; the daemon
//! calls them off its async workers.

use std::path::Path;

use crate::{AnthropicError, Catalog};

/// The catalog in the file at `path` for the API at `base_url`, as a catalog from the
/// cache. `None` when there is no file, or when it is of another version or of another
/// base URL. Fails when the file cannot be read or is not such a file.
pub fn read_cache(path: &Path, base_url: &str) -> Result<Option<Catalog>, AnthropicError> {
    // NOTE: the file format is not built yet, so no file is read and every start
    // fetches the list.
    let _ = (path, base_url);
    Ok(None)
}

/// Writes `catalog` to `path` in one step with mode 0600, so a reader sees the old file
/// or the new one, never a part.
pub fn write_cache(path: &Path, catalog: &Catalog) -> Result<(), AnthropicError> {
    // NOTE: the file format is not built yet, so nothing is written.
    let _ = (path, catalog);
    Ok(())
}

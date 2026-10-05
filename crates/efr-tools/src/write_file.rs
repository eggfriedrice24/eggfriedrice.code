//! `write_file`: replace or create one file, after its original is in the journal.

use std::fs::{self, File, Permissions};
use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use efr_scope::Home;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::diff;
use crate::paths::{check_real, resolve};
use crate::tool::parse_input;
use crate::{
    FileSnapshot, JournalEntry, Original, Tool, ToolContext, ToolError, ToolOutputSink,
    ToolRequirements, ToolResult, ToolSpec,
};

/// The largest file `write_file` replaces: its original must fit in the journal.
const MAX_ORIGINAL_BYTES: u64 = 16 * 1024 * 1024;

/// The mode of a file `write_file` creates. An existing file keeps its own.
const NEW_FILE_MODE: u32 = 0o644;

/// The input of `write_file`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteFileInput {
    /// The file to write: absolute, relative to the user's working directory, or under
    /// `~`. Missing parent directories are created.
    path: String,
    /// The whole new content of the file.
    content: String,
}

/// Writes a whole file. It declares the path for writing; before it writes, it
/// records the original (path, mode, owner and content, or that there was none) in
/// the context's journal, and it writes nothing when the journal fails. The write is
/// atomic (a temporary file renamed over the path); an existing file keeps its mode,
/// and its owner and group where the system allows it.
#[derive(Debug, Clone, Default)]
pub struct WriteFileTool;

impl WriteFileTool {
    /// The tool's name.
    pub const NAME: &'static str = "write_file";

    /// The tool.
    pub fn new() -> Self {
        WriteFileTool
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::for_input::<WriteFileInput>(
            Self::NAME,
            "Write the whole content of a text file, creating it and its parent \
             directories when they are missing. Relative paths start at the user's \
             working directory; ~ is the home directory. The original is kept so the \
             change can be undone. A path through a symbolic link fails and names the \
             real path.",
        )
    }

    fn requirements(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<ToolRequirements, ToolError> {
        let input: WriteFileInput = parse_input(Self::NAME, input)?;
        Ok(ToolRequirements::none().with_write(resolve(ctx, &input.path)?))
    }

    /// A unified diff from the current file to the new content (every line added for a
    /// new file), bounded to a few hundred lines. Nothing when the write would fail
    /// anyway, such as through a symbolic link, which the call then reports.
    async fn preview(&self, ctx: &ToolContext, input: &Value) -> Option<String> {
        let input: WriteFileInput = parse_input(Self::NAME, input).ok()?;
        let path = resolve(ctx, &input.path).ok()?;
        let home = ctx.home.clone();
        let target = path.clone();
        let snapshot = blocking(&path, move || snapshot(&home, &target)).await.ok()?;
        match &snapshot.original {
            Original::Missing => Some(diff::unified_diff(&path, None, &input.content)),
            Original::File { content, .. } => match std::str::from_utf8(content) {
                Ok(old) => Some(diff::unified_diff(&path, Some(old), &input.content)),
                Err(_) => Some(format!(
                    "{} is not a text file ({} bytes); all of it would be replaced with {} bytes",
                    path.display(),
                    content.len(),
                    input.content.len()
                )),
            },
        }
    }

    async fn invoke(
        &self,
        ctx: ToolContext,
        input: Value,
        _out: &mut dyn ToolOutputSink,
    ) -> Result<ToolResult, ToolError> {
        let input: WriteFileInput = parse_input(Self::NAME, &input)?;
        let path = resolve(&ctx, &input.path)?;

        let home = ctx.home.clone();
        let target = path.clone();
        let snapshot = blocking(&path, move || snapshot(&home, &target)).await?;
        let created = snapshot.original == Original::Missing;
        let mode = match &snapshot.original {
            Original::File { mode, .. } => *mode,
            _ => NEW_FILE_MODE,
        };
        let owner = match &snapshot.original {
            Original::File { uid, gid, .. } => Some((*uid, *gid)),
            _ => None,
        };
        ctx.journal.record(JournalEntry::new(ctx.ids, snapshot)).await?;

        let bytes = input.content.len();
        let target = path.clone();
        blocking(&path, move || write(&target, input.content.as_bytes(), mode, owner)).await?;
        let verb = if created { "created" } else { "replaced" };
        Ok(ToolResult::ok(format!("{verb} {} ({bytes} bytes)", path.display())))
    }
}

/// Runs `job` on the blocking pool.
async fn blocking<T: Send + 'static>(
    path: &Path,
    job: impl FnOnce() -> Result<T, ToolError> + Send + 'static,
) -> Result<T, ToolError> {
    tokio::task::spawn_blocking(job)
        .await
        .map_err(|_| ToolError::Interrupted { path: path.to_path_buf() })?
}

/// The original at `path`, after the symlink check.
fn snapshot(home: &Home, path: &Path) -> Result<FileSnapshot, ToolError> {
    check_real(home, path)?;
    let read_error = |source| ToolError::Read { path: path.to_path_buf(), source };
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileSnapshot::new(path, Original::Missing));
        }
        Err(source) => return Err(read_error(source)),
    };
    // A dangling link passes the symlink check (nothing real is behind it), and is not
    // a file to replace either.
    if !metadata.file_type().is_file() {
        return Err(ToolError::NotAFile { path: path.to_path_buf() });
    }
    if metadata.len() > MAX_ORIGINAL_BYTES {
        return Err(ToolError::TooLarge {
            path: path.to_path_buf(),
            bytes: metadata.len(),
            limit: MAX_ORIGINAL_BYTES,
        });
    }
    let mut content = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_ORIGINAL_BYTES).read_to_end(&mut content))
        .map_err(read_error)?;
    Ok(FileSnapshot::new(
        path,
        Original::File {
            mode: metadata.mode() & 0o7777,
            uid: metadata.uid(),
            gid: metadata.gid(),
            content,
        },
    ))
}

/// Writes `content` to `path` atomically with `mode`, creating missing parents, and
/// gives the file back to `owner` when there was one.
fn write(
    path: &Path,
    content: &[u8],
    mode: u32,
    owner: Option<(u32, u32)>,
) -> Result<(), ToolError> {
    let write_error = |source| ToolError::Write { path: path.to_path_buf(), source };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| {
            write_error(efr_stdx::StdxError::CreateDir { path: PathBuf::from(parent), source })
        })?;
    }
    efr_stdx::fs::write_atomic(path, content).map_err(write_error)?;
    fs::set_permissions(path, Permissions::from_mode(mode)).map_err(|source| {
        write_error(efr_stdx::StdxError::WriteFile { path: path.to_path_buf(), source })
    })?;
    if let Some((uid, gid)) = owner
        && let Err(error) = std::os::unix::fs::chown(path, Some(uid), Some(gid))
    {
        // Only root may give a file to another user; the new file then keeps the
        // daemon's user as its owner, which the journal entry records.
        tracing::debug!(path = %path.display(), %error, "kept the new owner of a replaced file");
    }
    Ok(())
}

#[cfg(test)]
mod tests;

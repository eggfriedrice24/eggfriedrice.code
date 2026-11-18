//! `read_file`: the text of one file, whole or a range of its lines, or the bytes of
//! one image file.

use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use efr_scope::Home;
use efr_stdx::media::{MediaType, SNIFF_BYTES, sniff};
use efr_stdx::text::byte_size;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::output::{DEFAULT_OUTPUT_LIMIT, truncate_middle};
use crate::paths::{check_real, resolve};
use crate::tool::parse_input;
use crate::{
    Tool, ToolContext, ToolError, ToolMedia, ToolOutputSink, ToolRequirements, ToolResult, ToolSpec,
};

/// The largest file `read_file` reads.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// How much of the start of a file is searched for a NUL byte, the sign of a binary
/// file.
const BINARY_PROBE_BYTES: usize = 8 * 1024;

/// The input of `read_file`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadFileInput {
    /// The file to read: absolute, relative to the user's working directory, or under
    /// `~`.
    path: String,
    /// The first line to show, counted from 1. Default: 1.
    #[serde(default)]
    offset: Option<usize>,
    /// How many lines to show. Default: every line to the end.
    #[serde(default)]
    limit: Option<usize>,
}

/// Reads a text file or an image file. It declares the path for reading, refuses a path
/// that goes through a symbolic link, a file over 16 MiB and a binary file that is no
/// image, and cuts long output in the middle. An image comes back as its bytes in
/// [`ToolResult::media`] with one line of text; the daemon decides what the model sees.
#[derive(Debug, Clone)]
pub struct ReadFileTool {
    output_limit: usize,
}

impl ReadFileTool {
    /// The tool's name.
    pub const NAME: &'static str = "read_file";

    /// The tool with the default output limit.
    pub fn new() -> Self {
        ReadFileTool { output_limit: DEFAULT_OUTPUT_LIMIT }
    }

    /// Sets the most bytes of output the model sees.
    #[must_use]
    pub fn with_output_limit(mut self, output_limit: usize) -> Self {
        self.output_limit = output_limit;
        self
    }
}

impl Default for ReadFileTool {
    fn default() -> Self {
        ReadFileTool::new()
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::for_input::<ReadFileInput>(
            Self::NAME,
            "Read a text file, or an image file (PNG, JPEG, GIF or WebP), which you then see \
             as an image if your model reads images. Relative paths start at the user's \
             working directory; ~ is the home directory. Use offset and limit to read a \
             range of lines of a long text file. A path through a symbolic link fails and \
             names the real path.",
        )
    }

    fn requirements(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<ToolRequirements, ToolError> {
        let input: ReadFileInput = parse_input(Self::NAME, input)?;
        Ok(ToolRequirements::none().with_read(resolve(ctx, &input.path)?))
    }

    async fn invoke(
        &self,
        ctx: ToolContext,
        input: Value,
        _out: &mut dyn ToolOutputSink,
    ) -> Result<ToolResult, ToolError> {
        let input: ReadFileInput = parse_input(Self::NAME, &input)?;
        let path = resolve(&ctx, &input.path)?;
        let home = ctx.home.clone();
        let target = path.clone();
        let read = tokio::task::spawn_blocking(move || read(&home, &target))
            .await
            .map_err(|_| ToolError::Interrupted { path: path.clone() })??;
        match read {
            Read::Text(text) => {
                let shown = select_lines(&text, input.offset, input.limit);
                let cut = truncate_middle(&shown, self.output_limit);
                Ok(ToolResult::ok(cut.text).with_truncated(cut.truncated))
            }
            Read::Image(media, bytes) => {
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                let text =
                    format!("{name}: {} image, {}", kind(media), byte_size(bytes.len() as u64));
                Ok(ToolResult::ok(text).with_media(vec![ToolMedia { path, bytes }]))
            }
        }
    }
}

/// What [`read`] found in a file.
enum Read {
    /// A text file.
    Text(String),
    /// A PNG, JPEG, GIF or WebP image, with its bytes.
    Image(MediaType, Vec<u8>),
}

/// The name of an image type in the text of a call.
fn kind(media: MediaType) -> &'static str {
    match media {
        MediaType::Png => "PNG",
        MediaType::Jpeg => "JPEG",
        MediaType::Gif => "GIF",
        MediaType::Webp => "WebP",
        _ => "an",
    }
}

/// The file's text or image, after the symlink and type checks, the type from its first
/// bytes, then the size check, before the whole file is read.
fn read(home: &Home, path: &Path) -> Result<Read, ToolError> {
    check_real(home, path)?;
    let read_error = |source| ToolError::Read { path: path.to_path_buf(), source };
    // NOTE: the type before the open, so a FIFO or a device is never opened.
    if !std::fs::metadata(path).map_err(read_error)?.is_file() {
        return Err(ToolError::NotAFile { path: path.to_path_buf() });
    }
    let mut file = File::open(path).map_err(read_error)?;
    let size = file.metadata().map_err(read_error)?.len();
    let mut bytes = Vec::new();
    (&mut file).take(SNIFF_BYTES as u64).read_to_end(&mut bytes).map_err(read_error)?;
    let image = sniff(&bytes).filter(|media| media.is_image());
    if size > MAX_FILE_BYTES {
        return Err(ToolError::TooLarge {
            path: path.to_path_buf(),
            bytes: size,
            limit: MAX_FILE_BYTES,
        });
    }
    bytes.reserve(usize::try_from(size).unwrap_or(0).saturating_sub(bytes.len()));
    // NOTE: `take` keeps a file that grows after the size check within the limit.
    file.take(MAX_FILE_BYTES - bytes.len() as u64).read_to_end(&mut bytes).map_err(read_error)?;
    if let Some(media) = image {
        return Ok(Read::Image(media, bytes));
    }
    if bytes[..bytes.len().min(BINARY_PROBE_BYTES)].contains(&0) {
        return Err(ToolError::NotText { path: PathBuf::from(path) });
    }
    Ok(Read::Text(String::from_utf8_lossy(&bytes).into_owned()))
}

/// The lines `offset..offset + limit` (counted from 1), with a note of the range when
/// it is not the whole file.
fn select_lines(text: &str, offset: Option<usize>, limit: Option<usize>) -> String {
    if offset.is_none() && limit.is_none() {
        return text.to_owned();
    }
    let total = text.lines().count();
    let first = offset.unwrap_or(1).max(1);
    let lines: Vec<&str> =
        text.split_inclusive('\n').skip(first - 1).take(limit.unwrap_or(usize::MAX)).collect();
    if lines.is_empty() {
        return format!("[the file has {total} lines; none from line {first}]");
    }
    let last = first + lines.len() - 1;
    let mut shown: String = lines.concat();
    if !shown.ends_with('\n') {
        shown.push('\n');
    }
    shown.push_str(&format!("[lines {first} to {last} of {total}]"));
    shown
}

#[cfg(test)]
mod tests;

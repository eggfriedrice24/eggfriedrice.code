//! The records that a call writes on fd 3, and the `apply` file for the trusted shell.
//!
//! The records come from untrusted code: the model's programs can write fd 3 too. So
//! the parser is strict: a header, known record kinds, a size and count limit, and an
//! `end` record. Any failure drops every record of the call; the cwd stays and no
//! export changes. Wire format (the spec's section 6.2), NUL-separated fields:
//!
//! ```text
//! efr-records\0v1\0
//! cwd\0<absolute path>\0
//! export\0<name>\0<value>\0
//! unset\0<name>\0
//! func\0<name>\0<body>\0
//! unfunc\0<name>\0
//! alias\0<name>\0<value>\0
//! unalias\0<name>\0
//! setup-error\0<text>\0      (the inner stage only, before exec)
//! end\0<status>\0
//! ```

use std::path::{Path, PathBuf};

use crate::SandboxError;
use crate::spec::RecordLimits;

/// The first two fields of every records stream.
pub const RECORDS_HEADER: &[u8] = b"efr-records\0v1\0";

/// What one call reported about the shell state it ended with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Records {
    /// The final working directory.
    pub cwd: Option<PathBuf>,
    /// Exported variables that are new or changed, in order.
    pub exports: Vec<(String, String)>,
    /// Exported variables that the call removed.
    pub unsets: Vec<String>,
    /// Functions that are new or changed, with their bodies.
    pub functions: Vec<(String, String)>,
    /// Functions that the call removed.
    pub removed_functions: Vec<String>,
    /// Aliases that are new or changed, with their values.
    pub aliases: Vec<(String, String)>,
    /// Aliases that the call removed.
    pub removed_aliases: Vec<String>,
    /// The inner stage's setup error. It came through fd 3, which the model's code
    /// can write too, so it is text for a person and never a decision.
    pub setup_error: Option<String>,
    /// The child shell's exit status from the `end` record.
    pub status: Option<i32>,
}

/// Parses a records stream within `limits`.
///
/// A stream whose first record is `setup-error` is the inner stage's report of a
/// failed setup: it ends the stream, with no `end` record.
pub fn parse_records(bytes: &[u8], limits: &RecordLimits) -> Result<Records, SandboxError> {
    if bytes.len() > limits.max_bytes {
        return Err(SandboxError::TooLarge {
            what: "records",
            len: bytes.len(),
            max: limits.max_bytes,
        });
    }
    let body = bytes.strip_prefix(RECORDS_HEADER).ok_or(SandboxError::RecordsHeader)?;
    let mut fields = Fields { bytes: body, at: RECORDS_HEADER.len() };
    let mut records = Records::default();
    let mut count = 0_usize;
    let mut first = true;
    while let Some((offset, kind)) = fields.next() {
        count += 1;
        if count > limits.max_records {
            return Err(SandboxError::TooManyRecords { max: limits.max_records });
        }
        let bad = || SandboxError::RecordMalformed { offset };
        let mut take = || fields.next().map(|(_, field)| field).ok_or_else(bad);
        match kind {
            b"cwd" => records.cwd = Some(cwd(take()?)?),
            b"export" => {
                let name = text(take()?).ok_or_else(bad)?;
                let value = text(take()?).ok_or_else(bad)?;
                records.exports.push((name, value));
            }
            b"unset" => records.unsets.push(text(take()?).ok_or_else(bad)?),
            b"func" => {
                let name = text(take()?).ok_or_else(bad)?;
                let body = text(take()?).ok_or_else(bad)?;
                records.functions.push((name, body));
            }
            b"unfunc" => records.removed_functions.push(text(take()?).ok_or_else(bad)?),
            b"alias" => {
                let name = text(take()?).ok_or_else(bad)?;
                let value = text(take()?).ok_or_else(bad)?;
                records.aliases.push((name, value));
            }
            b"unalias" => records.removed_aliases.push(text(take()?).ok_or_else(bad)?),
            b"setup-error" if first => {
                let message = String::from_utf8_lossy(take()?).into_owned();
                return Ok(Records { setup_error: Some(message), ..Records::default() });
            }
            b"end" => {
                let status =
                    text(take()?).and_then(|status| status.parse().ok()).ok_or_else(bad)?;
                records.status = Some(status);
                return Ok(records);
            }
            _ => return Err(bad()),
        }
        first = false;
    }
    Err(SandboxError::RecordsMissingEnd)
}

/// NUL-terminated fields with the offset of each.
struct Fields<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Fields<'a> {
    fn next(&mut self) -> Option<(usize, &'a [u8])> {
        let end = self.bytes.iter().position(|byte| *byte == 0)?;
        let (field, rest) = self.bytes.split_at(end);
        let offset = self.at;
        self.bytes = rest.get(1..).unwrap_or_default();
        self.at += end + 1;
        Some((offset, field))
    }
}

fn text(bytes: &[u8]) -> Option<String> {
    String::from_utf8(bytes.to_vec()).ok()
}

fn cwd(bytes: &[u8]) -> Result<PathBuf, SandboxError> {
    let path = text(bytes).ok_or(SandboxError::RecordCwd)?;
    if !path.starts_with('/') || path.chars().any(char::is_control) {
        return Err(SandboxError::RecordCwd);
    }
    Ok(PathBuf::from(path))
}

/// The records as their wire form, for tests and the fake bwrap of CI.
pub fn encode_records(records: &Records) -> Vec<u8> {
    let mut out = RECORDS_HEADER.to_vec();
    let mut field = |bytes: &[u8]| {
        out.extend_from_slice(bytes);
        out.push(0);
    };
    if let Some(error) = &records.setup_error {
        field(b"setup-error");
        field(error.as_bytes());
        return out;
    }
    if let Some(cwd) = &records.cwd {
        field(b"cwd");
        field(cwd.as_os_str().as_encoded_bytes());
    }
    let pairs = [
        (b"export".as_slice(), records.exports.as_slice()),
        (b"func".as_slice(), records.functions.as_slice()),
        (b"alias".as_slice(), records.aliases.as_slice()),
    ];
    for (kind, list) in pairs {
        for (name, value) in list {
            field(kind);
            field(name.as_bytes());
            field(value.as_bytes());
        }
    }
    let singles: [(&[u8], &[String]); 3] = [
        (b"unset", &records.unsets),
        (b"unfunc", &records.removed_functions),
        (b"unalias", &records.removed_aliases),
    ];
    for (kind, list) in singles {
        for name in list {
            field(kind);
            field(name.as_bytes());
        }
    }
    field(b"end");
    field(records.status.unwrap_or(0).to_string().as_bytes());
    out
}

/// The `$CALL/apply` file that the trusted shell's `_efr_hs_sbx_apply` reads:
/// `cd\0<dir>\0`, then `export\0<name>\0<value>\0` and `unset\0<name>\0`. Only filtered
/// values go in; the shell checks names and directories again.
pub fn encode_apply(
    cwd: Option<&Path>,
    exports: &[(String, String)],
    unsets: &[String],
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut field = |bytes: &[u8]| {
        out.extend_from_slice(bytes);
        out.push(0);
    };
    if let Some(cwd) = cwd {
        field(b"cd");
        field(cwd.as_os_str().as_encoded_bytes());
    }
    for (name, value) in exports {
        field(b"export");
        field(name.as_bytes());
        field(value.as_bytes());
    }
    for name in unsets {
        field(b"unset");
        field(name.as_bytes());
    }
    out
}

#[cfg(test)]
mod tests;

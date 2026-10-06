//! The fixture format and its loader.
//!
//! A fixture is one JSON object per line, each with a `kind`. Any record may carry an
//! `about` string, which is ignored and serves as a comment. Blank lines are skipped.
//!
//! The first record is the header:
//!
//! ```text
//! {"kind":"case","size":{"cols":20,"rows":3},"differs":["vt100"]}
//! ```
//!
//! `differs` names the backends that legitimately diverge on this case (vt100 answers
//! no DA or DSR, for example); for them the rows, cursor, size, title, bell and reply
//! checks are skipped. Marks are always checked, because the scanner, not the
//! backend, produces them.
//!
//! Then, in order:
//!
//! - `{"kind":"feed","text":"..."}` or `{"kind":"feed","hex":"1b5d..."}`: PTY output.
//!   `"chunk":N` feeds it in chunks of at most N bytes, `"split_at":[i,...]` cuts it at
//!   those byte offsets, and `"seq":N` sets the recording offset of its first byte
//!   (otherwise it continues from the previous feed, starting at 0).
//! - `{"kind":"resize","size":{"cols":C,"rows":R}}`.
//! - `{"kind":"expect_rows","rows":["...",...]}`: the text of every visible row, as
//!   [`row_text`](crate::row_text) renders it.
//! - `{"kind":"expect_cursor","row":R,"col":C}` and
//!   `{"kind":"expect_size","cols":C,"rows":R}`.
//! - `{"kind":"expect_title","title":"..."}` (or `null`): the snapshot's title, and
//!   with `"changes":["...",...]` also the title changes reported since the previous
//!   `expect_title`.
//! - `{"kind":"expect_bells","count":N}`: bells since the previous `expect_bells`.
//! - `{"kind":"expect_replies","text":"..."}` or `"hex"`: the PTY reply bytes since
//!   the previous `expect_replies`.
//! - `{"kind":"expect_marks","marks":[...]}`: the marks since the previous
//!   `expect_marks`, each as `{"start":S,"end":E,"<kind>":{...}}` where `<kind>` is
//!   `prompt_start`, `input_start`, `output_start`, `command_end`, `cwd_changed` or
//!   `sandbox_end` (see `mark_json` in the runner).
//!
//! At the end of a case every mark must have been expected, and every reply and bell
//! too unless the backend is in `differs`.

use std::path::{Path, PathBuf};

use efr_protocol::Size;
use serde_json::{Map, Value};

/// One fixture file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Case {
    /// The file name without `.ndjson`.
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) size: Size,
    pub(crate) differs: Vec<String>,
    pub(crate) steps: Vec<Step>,
}

/// One record after the header, with the line it came from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Step {
    pub(crate) line: usize,
    pub(crate) action: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Action {
    Feed { chunks: Vec<Vec<u8>>, seq: Option<u64> },
    Resize(Size),
    ExpectRows(Vec<String>),
    ExpectCursor { row: u16, col: u16 },
    ExpectSize(Size),
    ExpectTitle { title: Option<String>, changes: Option<Vec<String>> },
    ExpectBells(usize),
    ExpectReplies(Vec<u8>),
    ExpectMarks(Vec<Value>),
}

/// A fixture that cannot be read, with the line at fault (0 for the whole file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureError {
    pub(crate) path: PathBuf,
    pub(crate) line: usize,
    pub(crate) problem: String,
}

impl std::fmt::Display for FixtureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.path.display(), self.line, self.problem)
    }
}

/// Every `*.ndjson` file in `dir`, sorted by name.
pub(crate) fn load_dir(dir: &Path) -> Result<Vec<Case>, FixtureError> {
    let error = |problem: String| FixtureError { path: dir.to_owned(), line: 0, problem };
    let entries = std::fs::read_dir(dir).map_err(|err| error(format!("cannot list: {err}")))?;
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry.map_err(|err| error(format!("cannot list: {err}")))?.path();
        if path.extension().is_some_and(|ext| ext == "ndjson") {
            paths.push(path);
        }
    }
    paths.sort();
    if paths.is_empty() {
        return Err(error("holds no .ndjson fixtures".to_owned()));
    }
    paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).map_err(|err| FixtureError {
                path: path.clone(),
                line: 0,
                problem: format!("cannot read: {err}"),
            })?;
            parse(&path, &text)
        })
        .collect()
}

/// Parses one fixture.
pub(crate) fn parse(path: &Path, text: &str) -> Result<Case, FixtureError> {
    let error =
        |line: usize, problem: String| FixtureError { path: path.to_owned(), line, problem };
    let mut records = text
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty());
    let (header_line, header) =
        records.next().ok_or_else(|| error(0, "the file is empty".to_owned()))?;
    let header = record(header).map_err(|problem| error(header_line, problem))?;
    let (size, differs) = parse_header(&header).map_err(|problem| error(header_line, problem))?;
    let steps = records
        .map(|(line, text)| {
            let action = record(text).and_then(|record| parse_action(&record));
            action.map(|action| Step { line, action }).map_err(|problem| error(line, problem))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let name = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Case { name, path: path.to_owned(), size, differs, steps })
}

type Record = Map<String, Value>;

fn record(text: &str) -> Result<Record, String> {
    match serde_json::from_str(text) {
        Ok(Value::Object(record)) => Ok(record),
        Ok(_) => Err("a record must be a JSON object".to_owned()),
        Err(err) => Err(format!("not JSON: {err}")),
    }
}

fn parse_header(header: &Record) -> Result<(Size, Vec<String>), String> {
    if kind(header)? != "case" {
        return Err("the first record must be the case header".to_owned());
    }
    allow_keys(header, &["size", "differs"])?;
    let size = size(field(header, "size")?)?;
    let differs = match header.get("differs") {
        None => Vec::new(),
        Some(value) => strings(value, "differs")?,
    };
    Ok((size, differs))
}

fn parse_action(record: &Record) -> Result<Action, String> {
    let kind = kind(record)?;
    let action = match kind {
        "feed" => {
            allow_keys(record, &["text", "hex", "chunk", "split_at", "seq"])?;
            let data = bytes(record)?;
            let seq = record.get("seq").map(|value| number(value, "seq")).transpose()?;
            Action::Feed { chunks: chunks(record, data)?, seq }
        }
        "resize" => {
            allow_keys(record, &["size"])?;
            Action::Resize(size(field(record, "size")?)?)
        }
        "expect_rows" => {
            allow_keys(record, &["rows"])?;
            Action::ExpectRows(strings(field(record, "rows")?, "rows")?)
        }
        "expect_cursor" => {
            allow_keys(record, &["row", "col"])?;
            Action::ExpectCursor { row: small(record, "row")?, col: small(record, "col")? }
        }
        "expect_size" => {
            allow_keys(record, &["cols", "rows"])?;
            Action::ExpectSize(Size { cols: small(record, "cols")?, rows: small(record, "rows")? })
        }
        "expect_title" => {
            allow_keys(record, &["title", "changes"])?;
            let title = match field(record, "title")? {
                Value::Null => None,
                Value::String(title) => Some(title.clone()),
                _ => return Err("title must be a string or null".to_owned()),
            };
            let changes =
                record.get("changes").map(|value| strings(value, "changes")).transpose()?;
            Action::ExpectTitle { title, changes }
        }
        "expect_bells" => {
            allow_keys(record, &["count"])?;
            let count = number(field(record, "count")?, "count")?;
            Action::ExpectBells(usize::try_from(count).map_err(|_| "count is too large")?)
        }
        "expect_replies" => {
            allow_keys(record, &["text", "hex"])?;
            Action::ExpectReplies(bytes(record)?)
        }
        "expect_marks" => {
            allow_keys(record, &["marks"])?;
            match field(record, "marks")? {
                Value::Array(marks) => Action::ExpectMarks(marks.clone()),
                _ => return Err("marks must be an array".to_owned()),
            }
        }
        "case" => return Err("only the first record may be the case header".to_owned()),
        other => return Err(format!("unknown kind {other:?}")),
    };
    Ok(action)
}

fn kind(record: &Record) -> Result<&str, String> {
    match field(record, "kind")? {
        Value::String(kind) => Ok(kind),
        _ => Err("kind must be a string".to_owned()),
    }
}

/// Rejects keys a record of this kind does not have, so a typo fails loudly instead
/// of silently weakening the case.
fn allow_keys(record: &Record, allowed: &[&str]) -> Result<(), String> {
    match record
        .keys()
        .find(|key| !matches!(key.as_str(), "kind" | "about") && !allowed.contains(&key.as_str()))
    {
        Some(key) => Err(format!("unknown key {key:?}")),
        None => Ok(()),
    }
}

fn field<'a>(record: &'a Record, key: &str) -> Result<&'a Value, String> {
    record.get(key).ok_or_else(|| format!("missing {key:?}"))
}

fn number(value: &Value, what: &str) -> Result<u64, String> {
    value.as_u64().ok_or_else(|| format!("{what} must be a non-negative integer"))
}

fn small(record: &Record, key: &str) -> Result<u16, String> {
    let value = number(field(record, key)?, key)?;
    u16::try_from(value).map_err(|_| format!("{key} does not fit a terminal dimension"))
}

fn size(value: &Value) -> Result<Size, String> {
    let Value::Object(size) = value else {
        return Err("size must be an object with cols and rows".to_owned());
    };
    allow_keys(size, &["cols", "rows"])?;
    Ok(Size { cols: small(size, "cols")?, rows: small(size, "rows")? })
}

fn strings(value: &Value, what: &str) -> Result<Vec<String>, String> {
    let Value::Array(items) = value else {
        return Err(format!("{what} must be an array of strings"));
    };
    items
        .iter()
        .map(|item| {
            item.as_str().map(str::to_owned).ok_or_else(|| format!("{what} must hold strings"))
        })
        .collect()
}

/// The bytes of a record from exactly one of `text` and `hex`.
fn bytes(record: &Record) -> Result<Vec<u8>, String> {
    match (record.get("text"), record.get("hex")) {
        (Some(Value::String(text)), None) => Ok(text.as_bytes().to_vec()),
        (None, Some(Value::String(hex))) => decode_hex(hex),
        (Some(_), Some(_)) => Err("give text or hex, not both".to_owned()),
        (None, None) => Err("missing \"text\" or \"hex\"".to_owned()),
        _ => Err("text and hex must be strings".to_owned()),
    }
}

/// Hex pairs; whitespace between pairs is allowed for readability.
fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    let digits: Vec<u8> = hex.bytes().filter(|byte| !byte.is_ascii_whitespace()).collect();
    if !digits.len().is_multiple_of(2) {
        return Err("hex must hold whole bytes".to_owned());
    }
    digits
        .chunks(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| "hex holds a character that is not a hex digit".to_owned())
        })
        .collect()
}

fn chunks(record: &Record, data: Vec<u8>) -> Result<Vec<Vec<u8>>, String> {
    match (record.get("chunk"), record.get("split_at")) {
        (None, None) => Ok(vec![data]),
        (Some(chunk), None) => {
            let size =
                usize::try_from(number(chunk, "chunk")?).map_err(|_| "chunk is too large")?;
            if size == 0 {
                return Err("chunk must be at least 1".to_owned());
            }
            Ok(data.chunks(size).map(<[u8]>::to_vec).collect())
        }
        (None, Some(Value::Array(points))) => {
            let mut chunks = Vec::new();
            let mut from = 0;
            for point in points {
                let point = usize::try_from(number(point, "split_at")?)
                    .map_err(|_| "split_at is too large")?;
                if point <= from || point >= data.len() {
                    return Err("split_at must increase and stay inside the data".to_owned());
                }
                chunks.push(data[from..point].to_vec());
                from = point;
            }
            chunks.push(data[from..].to_vec());
            Ok(chunks)
        }
        (None, Some(_)) => Err("split_at must be an array".to_owned()),
        (Some(_), Some(_)) => Err("give chunk or split_at, not both".to_owned()),
    }
}

#[cfg(test)]
mod tests;

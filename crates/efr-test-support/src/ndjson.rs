//! NDJSON transcripts: the record format of the replay harness.
//!
//! A transcript is one JSON object per line, in the order things happen. The shape is
//! t3code's `expect_outbound` and `emit_inbound`:
//!
//! ```text
//! {"dir":"expect_outbound","kind":"provider_request","body":{...}}
//! {"dir":"emit_inbound","kind":"provider_sse","body":"data: {...}\n\n"}
//! {"dir":"emit_inbound","kind":"pty_bytes","pty":"shell","b64":"..."}
//! {"dir":"emit_inbound","kind":"client_frame","body":{...}}
//! {"dir":"expect_outbound","kind":"event","body":{...}}
//! {"kind":"clock_advance","ms":30000}
//! ```
//!
//! `expect_outbound` is something the code under test must send next; `emit_inbound`
//! is something the harness feeds into it. Which direction a kind may take is fixed:
//! `provider_request` and `event` go out, `provider_sse` and `client_frame` come in,
//! and `pty_bytes` goes either way (bytes the PTY produces come in, bytes written to
//! it go out). `clock_advance` has no direction: the harness moves its `TestClock`.
//!
//! The reader is strict, because a fixture with a typo would otherwise test nothing:
//! an unknown member, a wrong direction, a body of the wrong JSON type or bad base64
//! is an error naming the line. Blank lines are skipped. What a body means is checked
//! by its consumer: [`ReplayProvider`](crate::ReplayProvider) parses the provider
//! records, and the scenario driver of `efr-test-daemon` the others.

use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use serde_json::Value;

use crate::TestSupportError;

const EXPECT_OUTBOUND: &str = "expect_outbound";
const EMIT_INBOUND: &str = "emit_inbound";

/// One record of a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Record {
    /// `"dir": "expect_outbound"`: the code under test must send this next.
    ExpectOutbound(Outbound),
    /// `"dir": "emit_inbound"`: the harness feeds this into the code under test.
    EmitInbound(Inbound),
    /// `"kind": "clock_advance"`: the harness moves its clock forward. The transcript
    /// holds whole milliseconds, so a smaller part is dropped when it is written.
    ClockAdvance(Duration),
}

/// What the code under test sends.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outbound {
    /// `provider_request`: a request to a model. For the replay provider, the JSON form
    /// of an `efr_provider::Request`.
    ProviderRequest(Value),
    /// `event`: an event the daemon sends to its subscribers.
    Event(Value),
    /// `pty_bytes`: bytes written to a PTY.
    PtyBytes {
        /// The name of the PTY in the scenario, such as `shell`.
        pty: String,
        /// The bytes, `b64` in the transcript.
        bytes: Vec<u8>,
    },
}

/// What the harness feeds in.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Inbound {
    /// `provider_sse`: a piece of a model's streamed answer, as server-sent events text.
    ProviderSse(String),
    /// `pty_bytes`: bytes a PTY produces.
    PtyBytes {
        /// The name of the PTY in the scenario, such as `shell`.
        pty: String,
        /// The bytes, `b64` in the transcript.
        bytes: Vec<u8>,
    },
    /// `client_frame`: a frame a client sends to the daemon.
    ClientFrame(Value),
}

impl Record {
    /// The record's `kind` member.
    pub fn kind(&self) -> &'static str {
        match self {
            Record::ExpectOutbound(Outbound::ProviderRequest(_)) => "provider_request",
            Record::ExpectOutbound(Outbound::Event(_)) => "event",
            Record::ExpectOutbound(Outbound::PtyBytes { .. })
            | Record::EmitInbound(Inbound::PtyBytes { .. }) => "pty_bytes",
            Record::EmitInbound(Inbound::ProviderSse(_)) => "provider_sse",
            Record::EmitInbound(Inbound::ClientFrame(_)) => "client_frame",
            Record::ClockAdvance(_) => "clock_advance",
        }
    }

    /// True for the records a provider handles: `provider_request` and `provider_sse`.
    pub fn is_provider(&self) -> bool {
        matches!(
            self,
            Record::ExpectOutbound(Outbound::ProviderRequest(_))
                | Record::EmitInbound(Inbound::ProviderSse(_))
        )
    }

    /// The record as one line of NDJSON, without the newline, with its members in the
    /// order `dir`, `kind`, then the rest.
    pub fn to_json_line(&self) -> String {
        let text = |value: &str| Value::from(value).to_string();
        let mut members = Vec::with_capacity(4);
        match self {
            Record::ExpectOutbound(_) => members.push(("dir", text(EXPECT_OUTBOUND))),
            Record::EmitInbound(_) => members.push(("dir", text(EMIT_INBOUND))),
            Record::ClockAdvance(_) => {}
        }
        members.push(("kind", text(self.kind())));
        match self {
            Record::ExpectOutbound(Outbound::ProviderRequest(body) | Outbound::Event(body))
            | Record::EmitInbound(Inbound::ClientFrame(body)) => {
                members.push(("body", body.to_string()));
            }
            Record::EmitInbound(Inbound::ProviderSse(body)) => members.push(("body", text(body))),
            Record::ExpectOutbound(Outbound::PtyBytes { pty, bytes })
            | Record::EmitInbound(Inbound::PtyBytes { pty, bytes }) => {
                members.push(("pty", text(pty)));
                members.push(("b64", text(&STANDARD.encode(bytes))));
            }
            Record::ClockAdvance(by) => {
                let ms = u64::try_from(by.as_millis()).unwrap_or(u64::MAX);
                members.push(("ms", ms.to_string()));
            }
        }
        let members: Vec<String> =
            members.iter().map(|(name, value)| format!("{}:{value}", text(name))).collect();
        format!("{{{}}}", members.join(","))
    }
}

/// A record with the line it was read from, for error messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The line number in the transcript, from 1. Blank lines count.
    pub line: usize,
    /// The record.
    pub record: Record,
}

/// A validated transcript: its records in order, each with its line number.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transcript {
    entries: Vec<Entry>,
}

impl Transcript {
    /// Parses and validates NDJSON text. The first invalid line is the error.
    pub fn parse(text: &str) -> Result<Self, TestSupportError> {
        let mut entries = Vec::new();
        for (index, text) in text.lines().enumerate() {
            let line = index + 1;
            if text.trim().is_empty() {
                continue;
            }
            let raw: RawRecord = serde_json::from_str(text)
                .map_err(|source| TestSupportError::RecordSyntax { line, source })?;
            let record = raw
                .into_record()
                .map_err(|problem| TestSupportError::InvalidRecord { line, problem })?;
            entries.push(Entry { line, record });
        }
        Ok(Transcript { entries })
    }

    /// Reads and validates the transcript file at `path`.
    pub fn read(path: impl AsRef<Path>) -> Result<Self, TestSupportError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| {
            TestSupportError::ReadTranscript { path: path.to_path_buf(), source }
        })?;
        Transcript::parse(&text).map_err(|source| TestSupportError::InvalidTranscript {
            path: path.to_path_buf(),
            source: Box::new(source),
        })
    }

    /// A transcript of `records`, numbered from line 1 as [`to_ndjson`](Self::to_ndjson)
    /// writes them.
    pub fn from_records(records: impl IntoIterator<Item = Record>) -> Self {
        let entries = records
            .into_iter()
            .enumerate()
            .map(|(index, record)| Entry { line: index + 1, record })
            .collect();
        Transcript { entries }
    }

    /// The records with their line numbers, in order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The records, in order.
    pub fn records(&self) -> impl Iterator<Item = &Record> {
        self.entries.iter().map(|entry| &entry.record)
    }

    /// The number of records.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the transcript has no records.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The transcript as NDJSON text, one record per line, each line ending in a
    /// newline.
    pub fn to_ndjson(&self) -> String {
        self.records().map(|record| record.to_json_line() + "\n").collect()
    }
}

/// A line as JSON gives it, before the rules of its kind are checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRecord {
    #[serde(default)]
    dir: Option<String>,
    kind: String,
    #[serde(default)]
    body: Option<Value>,
    #[serde(default)]
    pty: Option<String>,
    #[serde(default)]
    b64: Option<String>,
    #[serde(default)]
    ms: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Out,
    In,
}

impl RawRecord {
    fn into_record(self) -> Result<Record, &'static str> {
        let dir = match self.dir.as_deref() {
            None => None,
            Some(EXPECT_OUTBOUND) => Some(Dir::Out),
            Some(EMIT_INBOUND) => Some(Dir::In),
            Some(_) => return Err("`dir` must be expect_outbound or emit_inbound"),
        };
        match self.kind.as_str() {
            "provider_request" | "event" => {
                self.only(&["body"])?;
                if dir != Some(Dir::Out) {
                    return Err("provider_request and event records must have dir expect_outbound");
                }
                let body = object(self.body)?;
                Ok(Record::ExpectOutbound(if self.kind == "event" {
                    Outbound::Event(body)
                } else {
                    Outbound::ProviderRequest(body)
                }))
            }
            "provider_sse" => {
                self.only(&["body"])?;
                if dir != Some(Dir::In) {
                    return Err("provider_sse and client_frame records must have dir emit_inbound");
                }
                match self.body {
                    Some(Value::String(body)) => {
                        Ok(Record::EmitInbound(Inbound::ProviderSse(body)))
                    }
                    _ => Err("`body` must be a string"),
                }
            }
            "client_frame" => {
                self.only(&["body"])?;
                if dir != Some(Dir::In) {
                    return Err("provider_sse and client_frame records must have dir emit_inbound");
                }
                Ok(Record::EmitInbound(Inbound::ClientFrame(object(self.body)?)))
            }
            "pty_bytes" => {
                self.only(&["pty", "b64"])?;
                let pty =
                    self.pty.filter(|pty| !pty.is_empty()).ok_or("`pty` must name the PTY")?;
                let bytes = self
                    .b64
                    .and_then(|b64| STANDARD.decode(b64).ok())
                    .ok_or("`b64` must be standard base64")?;
                match dir {
                    Some(Dir::Out) => Ok(Record::ExpectOutbound(Outbound::PtyBytes { pty, bytes })),
                    Some(Dir::In) => Ok(Record::EmitInbound(Inbound::PtyBytes { pty, bytes })),
                    None => Err("a pty_bytes record must have a dir"),
                }
            }
            "clock_advance" => {
                self.only(&["ms"])?;
                if dir.is_some() {
                    return Err("a clock_advance record must not have a dir");
                }
                let ms = self.ms.ok_or("a clock_advance record needs `ms`")?;
                Ok(Record::ClockAdvance(Duration::from_millis(ms)))
            }
            _ => Err("`kind` must be provider_request, provider_sse, pty_bytes, client_frame, \
                      event or clock_advance"),
        }
    }

    /// Fails when a member other than `allowed` (and `dir` and `kind`) is present.
    fn only(&self, allowed: &[&str]) -> Result<(), &'static str> {
        let present = [
            ("body", self.body.is_some()),
            ("pty", self.pty.is_some()),
            ("b64", self.b64.is_some()),
            ("ms", self.ms.is_some()),
        ];
        if present.iter().any(|(name, is_present)| *is_present && !allowed.contains(name)) {
            Err("the record has a member that its kind does not take")
        } else {
            Ok(())
        }
    }
}

fn object(body: Option<Value>) -> Result<Value, &'static str> {
    match body {
        Some(body @ Value::Object(_)) => Ok(body),
        _ => Err("`body` must be a JSON object"),
    }
}

#[cfg(test)]
mod tests;

//! `efr clip`: the read of a paste on a `,` line (OSC 5522, ADR 0027). The zsh plugin
//! catches the paste event of Ghostty 1.4 and runs it with the event in
//! `EFR_CLIP_EVENT`; it is hidden from the help.
//!
//! The command opens the terminal itself (the standard input of a widget's child is
//! `/dev/null`), asks for the best image type and `text/plain` in one read with the
//! one-time password of the event, and reads the reply to its end with three
//! deadlines. It never leaves in the middle of a reply, because the bytes that it
//! leaves in the terminal would be typed into the line: over the size limit, after
//! the whole deadline or at Ctrl+C it reads on and drops the rest (the drain). An image,
//! and a text over [`TEXT_IN_LINE_MAX`], go to a clip file in the state root, which
//! `efr` removes once the prompt went (`attach.rs`).
//!
//! It prints `<kind> NUL <text> NUL`, then the keys that came during the read, which the
//! plugin gives back to the line: `file` and a path, `text` and the text, `error`,
//! `failed` or `off` and a message. `efr clip --drain` reads a reply that came too late
//! to its end, drops it and prints an empty kind.

use std::io::Write as _;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use efr_protocol::Base64Bytes;
use efr_stdx::env::Var;
use efr_stdx::media::MediaType;
use efr_stdx::text::{MIB, byte_size};
use efr_stdx::time::Clock;
use jiff::Timestamp;
use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use rustix::termios::{
    InputModes, LocalModes, OptionalActions, SpecialCodeIndex, Termios, tcgetattr, tcsetattr,
};

use crate::attach::{CLIPS_DIR, ClipName};
use crate::cli::ClipArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format::attach::file_limit;
use crate::output::Output;

/// The start of an OSC 5522 packet.
const PREFIX: &[u8] = b"\x1b]5522;";

/// `Paste event` in base64: a read with the password of a paste event must name it.
const PASTE_NAME: &str = "UGFzdGUgZXZlbnQ=";

/// The image types that a paste takes, best first.
const IMAGE_TYPES: [MediaType; 4] =
    [MediaType::Png, MediaType::Jpeg, MediaType::Webp, MediaType::Gif];

/// The text type that a paste takes; Ghostty gives the text for each alias of it.
const TEXT_TYPE: &str = "text/plain";

/// The longest text that goes into the line itself: one environment variable of zsh
/// takes at most 128 KiB, and a longer prompt would not go. A longer text becomes a file.
const TEXT_IN_LINE_MAX: usize = 16 * 1024;

/// The time from the request to the first byte of the reply.
const FIRST_BYTE: Duration = Duration::from_secs(3);

/// The longest gap between two bytes of a reply: Ghostty writes a reply in one go, so
/// a longer gap means that it stopped.
const GAP: Duration = Duration::from_secs(1);

/// The longest read of a whole reply.
const TOTAL: Duration = Duration::from_secs(30);

/// The quiet time after which the drain ends.
const QUIET: Duration = Duration::from_millis(500);

/// The most bytes of one read of the terminal.
const READ_BYTES: usize = 64 * 1024;

/// The longest packet: a data packet holds at most 4096 bytes as base64, with its keys.
const PACKET_MAX: usize = 8 * 1024;

/// The most bytes of keys typed during a read that go back to the line.
const TYPED_MAX: usize = 4 * 1024;

/// The most clip files of one shell; the oldest goes first.
const CLIPS_PER_SHELL: usize = 10;

/// The age after which a clip file goes, whatever its shell.
const CLIP_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Ctrl+C, as a byte with the terminal's signals off.
const CTRL_C: u8 = 0x03;

/// The texts of the paste (spec 6.3). The zsh plugin shows `STOPPED` itself too
/// (`_efr_clip_stopped`), when `efr clip` gives no answer.
const STOPPED: &str = "efr: the clipboard read stopped. Nothing was added.";
const DENIED: &str = "efr: the terminal does not allow clipboard reads (clipboard-read). \
                      Pastes on , lines are text again in this shell.";
const NO_PRIMARY: &str = "efr: this clipboard is not available (primary selection).";
const NOTHING: &str = "efr: the clipboard has no image and no text.";

pub(crate) async fn run(ctx: &Context, out: &mut Output, args: &ClipArgs) -> Result<(), CliError> {
    let event = if args.drain {
        None
    } else {
        Some(
            ctx.env
                .var(Var::ClipEvent)
                .map_err(|source| CliError::Environment { source })?
                .unwrap_or_default(),
        )
    };
    let limit = u64::from(ctx.settings.attachments.max_file_mib) * MIB;
    let clips = ctx.dirs.state().join(CLIPS_DIR);
    let tty = args.tty.clone();
    let shell = args.shell.unwrap_or_default();
    let clock = Arc::clone(&ctx.clock);
    let answer = tokio::task::spawn_blocking(move || match event {
        Some(event) => read(&tty, &event, limit, &*clock)
            .map(|got| answer(got, &clips, shell, SystemTime::from(clock.now()))),
        None => drain(&tty, &*clock).map(|typed| Answer {
            kind: Kind::Drained,
            text: Vec::new(),
            typed,
        }),
    })
    .await;
    let answer = match answer {
        Ok(Ok(answer)) => answer,
        Ok(Err(error)) => {
            tracing::debug!(error = %error, "the clipboard read failed");
            Answer::failed(Failure::Stopped)
        }
        Err(error) => {
            tracing::debug!(error = %error, "the clipboard read stopped");
            Answer::failed(Failure::Stopped)
        }
    };
    out.out_bytes(&answer.bytes())
}

/// What the plugin does with an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Puts `@` and the path of a clip file into the line.
    File,
    /// Puts the text into the line.
    Text,
    /// Shows the message.
    Error,
    /// Shows the message; a second failed read turns the paste off in the shell.
    Failed,
    /// Shows the message and turns the paste off in the shell.
    Off,
    /// Nothing: the drain of a late reply ended.
    Drained,
}

impl Kind {
    /// The word of the kind in the answer.
    fn as_str(self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Text => "text",
            Kind::Error => "error",
            Kind::Failed => "failed",
            Kind::Off => "off",
            Kind::Drained => "",
        }
    }
}

/// Why a read failed, with the text that the plugin shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    /// The read stopped: Ctrl+C, an error of the terminal or a broken reply.
    Stopped,
    /// No byte came within [`FIRST_BYTE`].
    NoFirstByte,
    /// The reply took more than [`TOTAL`].
    TooLong,
}

impl Failure {
    fn text(self) -> String {
        match self {
            Failure::Stopped => STOPPED.to_owned(),
            Failure::NoFirstByte => format!(
                "efr: the terminal sent no clipboard data in {} s. Nothing was added.",
                FIRST_BYTE.as_secs()
            ),
            Failure::TooLong => format!(
                "efr: the clipboard read took more than {} s. Nothing was added.",
                TOTAL.as_secs()
            ),
        }
    }
}

/// What the plugin gets.
#[derive(Debug, PartialEq, Eq)]
struct Answer {
    kind: Kind,
    text: Vec<u8>,
    typed: Vec<u8>,
}

impl Answer {
    fn failed(failure: Failure) -> Answer {
        Answer { kind: Kind::Failed, text: failure.text().into_bytes(), typed: Vec::new() }
    }

    fn message(kind: Kind, message: &str) -> Answer {
        Answer { kind, text: message.into(), typed: Vec::new() }
    }

    /// `<kind> NUL <text> NUL <typed>`, with no NUL in the text.
    fn bytes(&self) -> Vec<u8> {
        let kind = self.kind.as_str();
        let mut bytes = Vec::with_capacity(kind.len() + self.text.len() + self.typed.len() + 2);
        bytes.extend_from_slice(kind.as_bytes());
        bytes.push(0);
        bytes.extend(self.text.iter().filter(|&&byte| byte != 0));
        bytes.push(0);
        bytes.extend_from_slice(&self.typed);
        bytes
    }
}

/// The plugin's answer for what a read `got`: a clip file below `clips` for an image or
/// a long text of the shell `shell`, written at `now`.
fn answer(got: Got, clips: &Path, shell: u32, now: SystemTime) -> Answer {
    let Got { end, typed } = got;
    let file = |extension: &str, bytes: &[u8]| match write_clip(clips, shell, extension, bytes, now)
    {
        Ok(path) => Answer {
            kind: Kind::File,
            text: path.into_os_string().into_encoded_bytes(),
            typed: Vec::new(),
        },
        Err(error) => {
            tracing::debug!(error = %error, "the clip file could not be written");
            Answer::failed(Failure::Stopped)
        }
    };
    let answer = match end {
        End::Image { media, bytes } => file(media.extension(), &bytes),
        End::Text(text) if text.len() > TEXT_IN_LINE_MAX => file("txt", &text),
        End::Text(text) => Answer { kind: Kind::Text, text, typed: Vec::new() },
        End::Nothing => Answer::message(Kind::Error, NOTHING),
        End::TooLarge { bytes, limit } => {
            let message = format!(
                "efr: the clipboard image is {}. The limit is {} (attachments.max_file_mib). \
                 Nothing was added.",
                byte_size(bytes),
                file_limit(limit),
            );
            Answer::message(Kind::Error, &message)
        }
        End::Denied => Answer::message(Kind::Off, DENIED),
        End::NoPrimary => Answer::message(Kind::Error, NO_PRIMARY),
        End::Failed(failure) => Answer::failed(failure),
    };
    Answer { typed, ..answer }
}

/// How a read ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum End {
    /// An image of the type `media`.
    Image { media: MediaType, bytes: Vec<u8> },
    /// Text and no image.
    Text(Vec<u8>),
    /// The clipboard had neither.
    Nothing,
    /// The image or the text had `bytes`, more than `limit`.
    TooLarge { bytes: u64, limit: u64 },
    /// The terminal does not allow the read (`EPERM`).
    Denied,
    /// The clipboard is not there (`ENOSYS`), such as a primary selection.
    NoPrimary,
    /// The read failed.
    Failed(Failure),
}

/// What a read gave: how it ended, and the keys typed meanwhile.
#[derive(Debug, PartialEq, Eq)]
struct Got {
    end: End,
    typed: Vec<u8>,
}

/// The facts of a paste event: its password, its location and the types of the
/// clipboard, or the error of the event itself.
#[derive(Debug, Default, PartialEq, Eq)]
struct Event {
    password: String,
    location: Option<String>,
    types: Vec<String>,
    error: Option<String>,
}

impl Event {
    fn parse(text: &str) -> Event {
        let mut event = Event::default();
        let mut packets = Packets::default();
        let mut other = Vec::new();
        for packet in packets.feed(text.as_bytes(), &mut other).unwrap_or_default() {
            if let Some(password) = packet.key("pw") {
                password.clone_into(&mut event.password);
            }
            if let Some(location) = packet.key("loc") {
                event.location = Some(location.to_owned());
            }
            match packet.key("status") {
                Some("DATA") if packet.mime().as_deref() == Some(".") => {
                    let listing = packet.data().unwrap_or_default();
                    event.types = String::from_utf8_lossy(&listing)
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect();
                }
                Some(status) if status.starts_with('E') => event.error = Some(status.to_owned()),
                _ => {}
            }
        }
        event
    }

    /// The image type to ask for, the best one that the clipboard has.
    fn image(&self) -> Option<MediaType> {
        IMAGE_TYPES.into_iter().find(|media| self.types.iter().any(|kind| kind == media.as_str()))
    }

    fn text(&self) -> bool {
        self.types.iter().any(|kind| kind == TEXT_TYPE)
    }

    /// The read of the types `wanted`, with the password and the name of a paste.
    fn request(&self, wanted: &[&str]) -> Vec<u8> {
        let mut request = PREFIX.to_vec();
        request.extend_from_slice(b"type=read");
        if let Some(location) = &self.location {
            request.extend_from_slice(b":loc=");
            request.extend_from_slice(location.as_bytes());
        }
        request.extend_from_slice(b":pw=");
        request.extend_from_slice(self.password.as_bytes());
        request.extend_from_slice(b":name=");
        request.extend_from_slice(PASTE_NAME.as_bytes());
        request.push(b';');
        request.extend_from_slice(Base64Bytes::encode(wanted.join(" ").as_bytes()).as_bytes());
        request.extend_from_slice(b"\x1b\\");
        request
    }
}

/// One OSC 5522 packet: its keys and its payload, still in base64.
#[derive(Debug, PartialEq, Eq)]
struct Packet {
    keys: Vec<(String, String)>,
    payload: Vec<u8>,
}

impl Packet {
    fn key(&self, name: &str) -> Option<&str> {
        self.keys.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }

    /// The type of a data packet, decoded.
    fn mime(&self) -> Option<String> {
        let mime = Base64Bytes::decode(self.key("mime")?)?;
        String::from_utf8(mime).ok()
    }

    /// The payload, decoded; `None` when it is not strict base64.
    fn data(&self) -> Option<Vec<u8>> {
        Base64Bytes::decode(std::str::from_utf8(&self.payload).ok()?)
    }
}

/// The packets of a stream of terminal bytes, piece by piece; each other byte is a key.
#[derive(Debug, Default)]
struct Packets {
    buf: Vec<u8>,
}

/// A packet that did not end within [`PACKET_MAX`] bytes.
#[derive(Debug, PartialEq, Eq)]
struct Broken;

impl Packets {
    /// The packets that `chunk` ends; the bytes outside packets go to `other`.
    fn feed(&mut self, chunk: &[u8], other: &mut Vec<u8>) -> Result<Vec<Packet>, Broken> {
        self.buf.extend_from_slice(chunk);
        let mut packets = Vec::new();
        loop {
            let Some(start) = find(&self.buf, PREFIX) else {
                // NOTE: a tail that can be the start of a prefix waits for more bytes.
                let keep = (1..PREFIX.len().min(self.buf.len() + 1))
                    .rev()
                    .find(|&size| self.buf.ends_with(&PREFIX[..size]))
                    .unwrap_or(0);
                other.extend(self.buf.drain(..self.buf.len() - keep));
                return Ok(packets);
            };
            other.extend(self.buf.drain(..start));
            let body = &self.buf[PREFIX.len()..];
            let end =
                [find(body, b"\x1b\\").map(|at| (at, 2)), find(body, b"\x07").map(|at| (at, 1))]
                    .into_iter()
                    .flatten()
                    .min();
            let Some((at, width)) = end else {
                if self.buf.len() > PACKET_MAX {
                    return Err(Broken);
                }
                return Ok(packets);
            };
            let (meta, payload) = match body[..at].iter().position(|&byte| byte == b';') {
                Some(split) => (&body[..split], &body[split + 1..at]),
                None => (&body[..at], &body[at..at]),
            };
            let keys = meta
                .split(|&byte| byte == b':')
                .map(|part| {
                    let text = String::from_utf8_lossy(part);
                    let (key, value) = text.split_once('=').unwrap_or((&text, ""));
                    (key.to_owned(), value.to_owned())
                })
                .collect();
            packets.push(Packet { keys, payload: payload.to_vec() });
            self.buf.drain(..PREFIX.len() + at + width);
        }
    }

    /// The bytes that wait for the rest of a packet: at the end of a read they were
    /// keys.
    fn rest(self) -> Vec<u8> {
        self.buf
    }
}

/// The first place of `needle` in `bytes`.
fn find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes.windows(needle.len()).position(|window| window == needle)
}

/// The state of one reply as its packets come.
#[derive(Debug)]
struct Reply {
    image: Option<MediaType>,
    limit: u64,
    image_bytes: Vec<u8>,
    text: Vec<u8>,
    /// The bytes of the image so far, also those that the drain dropped.
    image_count: u64,
    /// The bytes of the text so far, also those that the drain dropped.
    text_count: u64,
    /// Why the rest of the reply is dropped.
    dropping: Option<Dropping>,
    typed: Vec<u8>,
}

/// Why the rest of a reply is dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dropping {
    /// The image or the text is over the size limit.
    TooLarge,
    /// The deadline passed, Ctrl+C came or a packet broke.
    Failed(Failure),
}

/// What the read does after a packet or a key.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    Read,
    Ended(End),
}

impl Reply {
    fn new(image: Option<MediaType>, limit: u64) -> Reply {
        Reply {
            image,
            limit,
            image_bytes: Vec::new(),
            text: Vec::new(),
            image_count: 0,
            text_count: 0,
            dropping: None,
            typed: Vec::new(),
        }
    }

    /// Takes one packet of the reply.
    fn packet(&mut self, packet: &Packet) -> Next {
        match packet.key("status") {
            Some("DATA") => {
                let Some(data) = packet.data() else {
                    return self.stop(End::Failed(Failure::Stopped));
                };
                let mime = packet.mime();
                let image = self.image.is_some_and(|media| mime.as_deref() == Some(media.as_str()));
                let (count, buffer) = match mime.as_deref() {
                    _ if image => (&mut self.image_count, &mut self.image_bytes),
                    Some(TEXT_TYPE) => (&mut self.text_count, &mut self.text),
                    _ => return Next::Read,
                };
                *count += data.len() as u64;
                if self.dropping.is_some() {
                    return Next::Read;
                }
                if *count > self.limit {
                    self.image_bytes = Vec::new();
                    self.text = Vec::new();
                    self.dropping = Some(Dropping::TooLarge);
                } else {
                    buffer.extend_from_slice(&data);
                }
                Next::Read
            }
            Some("DONE") => Next::Ended(self.finish()),
            Some("EPERM") => self.stop(End::Denied),
            Some("ENOSYS") => self.stop(End::NoPrimary),
            Some(status) if status.starts_with('E') => self.stop(End::Failed(Failure::Stopped)),
            _ => Next::Read,
        }
    }

    /// Takes the keys `typed` during the read. Ctrl+C drops the rest of the reply, and
    /// a second one ends the drain.
    fn keys(&mut self, typed: &[u8]) -> Next {
        for &byte in typed {
            if byte == CTRL_C {
                if self.dropping.is_some() {
                    return Next::Ended(self.finish());
                }
                self.dropping = Some(Dropping::Failed(Failure::Stopped));
                continue;
            }
            if self.typed.len() < TYPED_MAX {
                self.typed.push(byte);
            }
        }
        Next::Read
    }

    /// The whole deadline passed: the rest of the reply is dropped.
    fn too_long(&mut self) {
        self.dropping.get_or_insert(Dropping::Failed(Failure::TooLong));
    }

    /// A packet that ends the reply with `end`, unless it is dropped already.
    fn stop(&mut self, end: End) -> Next {
        Next::Ended(self.dropping.take().map_or(end, |dropping| self.dropped(dropping)))
    }

    /// How the reply ends at its last packet or at the end of the drain.
    fn finish(&mut self) -> End {
        if let Some(dropping) = self.dropping.take() {
            return self.dropped(dropping);
        }
        match self.image {
            Some(media) if !self.image_bytes.is_empty() => {
                End::Image { media, bytes: std::mem::take(&mut self.image_bytes) }
            }
            _ if !self.text.is_empty() => End::Text(std::mem::take(&mut self.text)),
            _ => End::Nothing,
        }
    }

    /// How a reply whose rest the read dropped for `dropping` ends.
    fn dropped(&self, dropping: Dropping) -> End {
        match dropping {
            Dropping::TooLarge => {
                End::TooLarge { bytes: self.image_count.max(self.text_count), limit: self.limit }
            }
            Dropping::Failed(failure) => End::Failed(failure),
        }
    }

    /// How long the read waits for the next byte, after `elapsed` in all and with
    /// `started` true once a byte came.
    fn wait(&self, elapsed: Duration, started: bool) -> Duration {
        if self.dropping.is_some() {
            return QUIET;
        }
        let gap = if started { GAP } else { FIRST_BYTE };
        gap.min(TOTAL.saturating_sub(elapsed))
    }

    /// How the read ends when no byte came in its wait, after `elapsed` in all.
    fn quiet(&mut self, elapsed: Duration, started: bool) -> End {
        if self.dropping.is_some() {
            return self.finish();
        }
        if elapsed >= TOTAL {
            self.too_long();
            return self.finish();
        }
        End::Failed(if started { Failure::Stopped } else { Failure::NoFirstByte })
    }

    /// Keeps the keys `typed` after the end of the reply.
    fn typed_after(&mut self, typed: &[u8]) {
        let room = TYPED_MAX.saturating_sub(self.typed.len());
        self.typed.extend(typed.iter().take(room));
    }
}

/// The time that passed on `clock` since `start`.
fn since(clock: &dyn Clock, start: Timestamp) -> Duration {
    Duration::try_from(clock.now().duration_since(start)).unwrap_or_default()
}

/// Reads the clipboard on the terminal `tty` for the paste event `event`, with images
/// and texts of at most `limit` bytes, and the deadlines on `clock`.
fn read(tty: &Path, event: &str, limit: u64, clock: &dyn Clock) -> std::io::Result<Got> {
    let event = Event::parse(event);
    let ended = |end| Ok(Got { end, typed: Vec::new() });
    match event.error.as_deref() {
        Some("EPERM") => return ended(End::Denied),
        Some("ENOSYS") => return ended(End::NoPrimary),
        Some(_) => return ended(End::Failed(Failure::Stopped)),
        None => {}
    }
    if event.password.is_empty() {
        return ended(End::Failed(Failure::Stopped));
    }
    let image = event.image();
    let mut wanted: Vec<&str> = image.iter().map(|media| media.as_str()).collect();
    if event.text() {
        wanted.push(TEXT_TYPE);
    }
    if wanted.is_empty() {
        return ended(End::Nothing);
    }
    let terminal = Terminal::open(tty)?;
    terminal.write(&event.request(&wanted))?;
    let mut reply = Reply::new(image, limit);
    let mut packets = Packets::default();
    let start = clock.now();
    let mut started = false;
    let mut chunk = vec![0_u8; READ_BYTES];
    loop {
        let elapsed = since(clock, start);
        if elapsed >= TOTAL {
            reply.too_long();
        }
        let wait = reply.wait(elapsed, started);
        let Some(read) = terminal.read(&mut chunk, wait)? else {
            let end = reply.quiet(since(clock, start), started);
            return Ok(Got { end, typed: reply.typed });
        };
        started = true;
        let mut other = Vec::new();
        let found = match packets.feed(&chunk[..read], &mut other) {
            Ok(found) => found,
            Err(Broken) => {
                reply.dropping.get_or_insert(Dropping::Failed(Failure::Stopped));
                packets = Packets::default();
                Vec::new()
            }
        };
        for packet in &found {
            if let Next::Ended(end) = reply.packet(packet) {
                // NOTE: the bytes around the last packet are keys typed during the read.
                reply.typed_after(&other);
                reply.typed_after(&packets.rest());
                return Ok(Got { end, typed: reply.typed });
            }
        }
        if let Next::Ended(end) = reply.keys(&other) {
            return Ok(Got { end, typed: reply.typed });
        }
    }
}

/// Reads the terminal `tty` to the end of a reply that came too late, or to
/// [`QUIET`], and drops it. The keys typed meanwhile come back.
fn drain(tty: &Path, clock: &dyn Clock) -> std::io::Result<Vec<u8>> {
    let terminal = Terminal::open(tty)?;
    let mut packets = Packets::default();
    let mut typed = Vec::new();
    let mut chunk = vec![0_u8; READ_BYTES];
    let start = clock.now();
    while let Some(read) = terminal.read(&mut chunk, QUIET)? {
        let mut other = Vec::new();
        let ended = match packets.feed(&chunk[..read], &mut other) {
            Ok(found) => found.iter().any(|packet| {
                packet
                    .key("status")
                    .is_some_and(|status| status == "DONE" || status.starts_with('E'))
            }),
            Err(Broken) => {
                packets = Packets::default();
                false
            }
        };
        let room = TYPED_MAX.saturating_sub(typed.len());
        typed.extend(other.iter().copied().filter(|&byte| byte != CTRL_C).take(room));
        if ended || other.contains(&CTRL_C) || since(clock, start) >= TOTAL {
            break;
        }
    }
    Ok(typed)
}

/// The terminal of the read, with no echo, no line editing and no signal keys while
/// it lasts, and its settings back when it drops.
struct Terminal {
    fd: OwnedFd,
    saved: Termios,
}

impl Terminal {
    fn open(tty: &Path) -> std::io::Result<Terminal> {
        let fd =
            rustix::fs::open(tty, OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC, Mode::empty())?;
        let saved = tcgetattr(&fd)?;
        let mut raw = saved.clone();
        raw.local_modes.remove(LocalModes::ICANON | LocalModes::ECHO | LocalModes::ISIG);
        raw.input_modes.remove(InputModes::ICRNL | InputModes::IXON);
        // NOTE: a read never waits; `poll` waits, with the deadline.
        raw.special_codes[SpecialCodeIndex::VMIN] = 0;
        raw.special_codes[SpecialCodeIndex::VTIME] = 0;
        tcsetattr(&fd, OptionalActions::Now, &raw)?;
        Ok(Terminal { fd, saved })
    }

    fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = std::fs::File::from(self.fd.try_clone()?);
        file.write_all(bytes)
    }

    /// The bytes that come within `wait`; `None` when none came.
    fn read(&self, chunk: &mut [u8], wait: Duration) -> std::io::Result<Option<usize>> {
        let timeout = Timespec::try_from(wait).map_err(std::io::Error::other)?;
        let mut fds = [PollFd::new(&self.fd, PollFlags::IN)];
        match rustix::event::poll(&mut fds, Some(&timeout)) {
            Ok(0) | Err(Errno::INTR) => return Ok(None),
            Ok(_) => {}
            Err(errno) => return Err(errno.into()),
        }
        match rustix::io::read(&self.fd, chunk) {
            Ok(0) | Err(Errno::INTR | Errno::AGAIN) => Ok(None),
            Ok(read) => Ok(Some(read)),
            Err(errno) => Err(errno.into()),
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Err(errno) = tcsetattr(&self.fd, OptionalActions::Now, &self.saved) {
            tracing::debug!(%errno, "the settings of the terminal could not be put back");
        }
    }
}

/// Writes `bytes` as the next clip file of the shell `shell` in `dir`,
/// `<shell>.<n>.<extension>` (0600, the dir 0700), and gives its path. It first
/// removes each clip file older than [`CLIP_MAX_AGE`] and the oldest files of the shell
/// over [`CLIPS_PER_SHELL`], at `now`.
fn write_clip(
    dir: &Path,
    shell: u32,
    extension: &str,
    bytes: &[u8],
    now: SystemTime,
) -> std::io::Result<PathBuf> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let dir = std::fs::canonicalize(dir)?;
    let mut mine: Vec<(u64, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| now.duration_since(modified).unwrap_or_default() > CLIP_MAX_AGE);
        if old {
            remove(&entry.path());
            continue;
        }
        let name = entry.file_name();
        let clip = name.to_str().and_then(ClipName::parse).filter(|clip| clip.shell == shell);
        if let Some(clip) = clip {
            mine.push((clip.number, entry.path()));
        }
    }
    mine.sort_unstable();
    let over = (mine.len() + 1).saturating_sub(CLIPS_PER_SHELL);
    for (_, path) in &mine[..over] {
        remove(path);
    }
    let number = mine.last().map_or(1, |(last, _)| last + 1);
    let path = dir.join(ClipName { shell, number, extension }.to_string());
    let mut file =
        std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
    file.write_all(bytes)?;
    Ok(path)
}

fn remove(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        tracing::debug!(path = %path.display(), error = %error, "a clip file could not be removed");
    }
}

#[cfg(test)]
mod tests;

//! The calls that a filtered bus proxy refused, read from its `--log` lines (DE11).
//!
//! `xdg-dbus-proxy --log` prints one header line per message (check C4-2, version 0.1.9;
//! the wrapper sends its stdout to the stderr pipe that efrd reads):
//!
//! - `C<serial>: -> <destination> call <interface>.<member> at <path>` for a method call
//!   of a client, `signal`, `fake` or another word for the other kinds;
//! - `B<serial>: <- ...` for a message from the bus to a client;
//! - `*DENIED* (ping)` or `*HIDDEN* (ping)` right after the header of a message that the
//!   filter refused, and `*SKIPPED*` or `*REWRITTEN*` after the replies it changed;
//! - `Filtering message due to arg0 <name>, policy: N (required M)` before the verdict
//!   of a bus driver call (such as `StartServiceByName`) whose name the rules hide.
//!
//! The serial of a `C` line is the client's own message serial, not a client id, and
//! each client starts at `C1`. So a verdict pairs only with the call line just before
//! it that is still open: any other header line closes it, and a verdict after a signal
//! line or a `B` line pairs with nothing. A bus driver refusal counts as a refusal of
//! the name that the call asked for (arg0), with no member and no path; the verdict
//! that follows it pairs with nothing, so it counts once. Each name, member and path
//! must match the D-Bus grammar, else it becomes [`INVALID`]: the text comes from the
//! command's own messages. A line that is not whole (the end of a log that efrd cut) is
//! skipped, and so is every line that this module does not know.

use efr_protocol::{BusKind, BusRefusal, SandboxSummary};

/// The text that stands for a name, a member or a path that is not valid D-Bus grammar.
pub(crate) const INVALID: &str = "<invalid>";

/// The longest name, interface, member or path that a refusal keeps; a longer one is
/// [`INVALID`]. The D-Bus grammar caps names at 255 bytes.
const MAX_PART: usize = 255;

/// The refusals of the proxies of one call or job: at most
/// [`SandboxSummary::MAX_BUS_REFUSED`] entries, one per bus, name, member and path,
/// each with its count, in the order they first came, and the count of the rest.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Refusals {
    entries: Vec<BusRefusal>,
    more: u32,
}

impl Refusals {
    /// Adds the refusals in `log`, the stderr of the proxy of `bus`.
    pub(crate) fn read(&mut self, bus: BusKind, log: &[u8]) {
        let mut open: Option<Call> = None;
        // NOTE: a last part with no newline is a line that efrd cut at its limit.
        let whole = whole_lines(log);
        for raw in whole.split(|byte| *byte == b'\n') {
            let line = String::from_utf8_lossy(raw);
            let line = line.trim_end_matches('\r');
            if let Some(rest) = line.strip_prefix('C').filter(|rest| rest.contains(": -> ")) {
                open = client_call(rest);
            } else if line.starts_with('B') && line.contains(": <- ") {
                open = None;
            } else if line.starts_with("*DENIED*") || line.starts_with("*HIDDEN*") {
                if let Some(call) = open.take() {
                    self.add(bus, call.name, call.member, call.path);
                }
            } else if let Some(rest) = line.strip_prefix("Filtering message due to arg0 ") {
                let name = rest.split([',', ' ']).next().unwrap_or_default();
                self.add(bus, checked_name(name), String::new(), String::new());
                open = None;
            }
        }
    }

    /// The entries and the count of the refusals that they leave out.
    pub(crate) fn into_parts(self) -> (Vec<BusRefusal>, u32) {
        (self.entries, self.more)
    }

    fn add(&mut self, bus: BusKind, name: String, member: String, path: String) {
        let same = |entry: &&mut BusRefusal| {
            entry.bus == bus && entry.name == name && entry.member == member && entry.path == path
        };
        if let Some(entry) = self.entries.iter_mut().find(same) {
            entry.count = entry.count.saturating_add(1);
        } else if self.entries.len() < SandboxSummary::MAX_BUS_REFUSED {
            self.entries.push(BusRefusal { bus, name, member, path, count: 1 });
        } else {
            self.more = self.more.saturating_add(1);
        }
    }
}

/// A method call line, checked.
#[derive(Debug)]
struct Call {
    name: String,
    member: String,
    path: String,
}

/// The method call of a client header line, from the rest after its `C`; `None` for
/// any other message of a client and for a line that does not parse.
fn client_call(rest: &str) -> Option<Call> {
    let (serial, rest) = rest.split_once(": -> ")?;
    serial.parse::<i64>().ok()?;
    // NOTE: a call with no destination logs `(no dest)`, two words.
    let (name, rest) = match rest.strip_prefix("(no dest) ") {
        Some(rest) => (INVALID.to_owned(), rest),
        None => {
            let (name, rest) = rest.split_once(' ')?;
            (checked_name(name), rest)
        }
    };
    let call = rest.strip_prefix("call ")?;
    let (member, path) = call.rsplit_once(" at ").unwrap_or((call, ""));
    Some(Call { name, member: checked_member(member), path: checked_path(path) })
}

/// `name` when it is a well-known or a unique bus name, else [`INVALID`].
fn checked_name(name: &str) -> String {
    let well_known = |name: &str| {
        name.contains('.')
            && name.split('.').all(|element| {
                element.bytes().next().is_some_and(|first| !first.is_ascii_digit())
                    && element
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
            })
    };
    let unique = |name: &str| {
        name.strip_prefix(':').is_some_and(|rest| {
            rest.contains('.')
                && rest.split('.').all(|element| {
                    !element.is_empty()
                        && element
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
                })
        })
    };
    if name.len() <= MAX_PART && (well_known(name) || unique(name)) {
        name.to_owned()
    } else {
        INVALID.to_owned()
    }
}

/// `interface.member`, or the member alone (a call with no interface logs `.member`),
/// when each part matches the grammar, else [`INVALID`].
fn checked_member(text: &str) -> String {
    let Some((interface, member)) = text.rsplit_once('.') else {
        return INVALID.to_owned();
    };
    let valid = is_member(member)
        && (interface.is_empty()
            || (interface.len() <= MAX_PART
                && interface.contains('.')
                && interface.split('.').all(is_member)));
    match (valid, interface.is_empty()) {
        (false, _) => INVALID.to_owned(),
        (true, true) => member.to_owned(),
        (true, false) => text.to_owned(),
    }
}

/// A member name, or one element of an interface name: `[A-Za-z_][A-Za-z0-9_]*`.
fn is_member(name: &str) -> bool {
    name.len() <= MAX_PART
        && name.bytes().next().is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// `path` when it is an object path of at most [`MAX_PART`] bytes, else [`INVALID`].
fn checked_path(path: &str) -> String {
    let element = |element: &str| {
        !element.is_empty()
            && element.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    };
    let valid =
        path == "/" || path.strip_prefix('/').is_some_and(|rest| rest.split('/').all(element));
    if valid && path.len() <= MAX_PART { path.to_owned() } else { INVALID.to_owned() }
}

/// The part of `log` up to its last newline: its whole lines.
fn whole_lines(log: &[u8]) -> &[u8] {
    match log.iter().rposition(|byte| *byte == b'\n') {
        Some(end) => &log[..end],
        None => &[],
    }
}

#[cfg(test)]
mod tests;

//! The server-sent events of a `provider_sse` body.
//!
//! A replay body is a whole string, not a stream of chunks, so this reader is much
//! smaller than `efr_http::SseDecoder`, which this crate may not depend on. It follows
//! the same field rules (`event`, `data`, comments, one optional space after the
//! colon) and differs in one place on purpose: an event that is not closed by a blank
//! line is an error instead of being dropped, because a fixture that loses its last
//! event would test less than it says.

/// The event type of an event without an `event` field.
const DEFAULT_EVENT: &str = "message";

/// One event: its type and its `data` lines joined with `\n`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub(crate) event: String,
    pub(crate) data: String,
}

/// The events of `body`, in order. An event with no `data` field is skipped, as the
/// standard says; `id`, `retry` and unknown fields mean nothing to a replay.
pub(crate) fn parse(body: &str) -> Result<Vec<SseEvent>, &'static str> {
    let mut events = Vec::new();
    let mut event = String::new();
    let mut data: Option<String> = None;
    let mut open = false;
    for line in body.lines() {
        if line.is_empty() {
            if let Some(data) = data.take() {
                let event = if event.is_empty() { DEFAULT_EVENT } else { event.as_str() };
                events.push(SseEvent { event: event.to_owned(), data });
            }
            event.clear();
            open = false;
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        open = true;
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => value.clone_into(&mut event),
            "data" => match &mut data {
                Some(data) => {
                    data.push('\n');
                    data.push_str(value);
                }
                None => data = Some(value.to_owned()),
            },
            _ => {}
        }
    }
    if open {
        return Err(
            "the provider_sse body ends inside an event; end every event with a blank line",
        );
    }
    Ok(events)
}

#[cfg(test)]
mod tests;

//! `pty.attach`: watch a hidden shell's screen and output. Sequence numbers are byte
//! offsets in the PTY's recording.
//!
//! The client is registered for live steps first, then gets either the output after
//! its `since_seq` from the recording (when the recording holds it and the gap is at
//! most 1 MiB) or a snapshot of the daemon's screen followed by the output recorded
//! after the snapshot's offset. The recording also holds each new size of the PTY, and
//! a replay sends it as `resized` at its place between the output. The replay stops at
//! the mark of the registration: every step after it reaches the client live, so
//! nothing is lost or repeated, and the order stays the recording's. The stream ends
//! when the shell exits; a client that falls behind gets `overflow` with the offset to
//! resume from. Attaching never changes the PTY.

use efr_protocol::{Base64Bytes, PtyAttach, PtyAttachItem, PtyId, Seq};
use efr_screen::ScreenHandle;
use efr_store::recording::RecordedRange;
use efr_transport::{Responder, TransportError};

use crate::DaemonError;
use crate::ptys::{AttachMark, PtyDelivery, PtyLive};
use crate::state::State;

/// Scrollback rows in a snapshot when the client names none.
const DEFAULT_SCROLLBACK: u32 = 200;
/// The most scrollback rows a snapshot carries.
pub(crate) const MAX_SCROLLBACK: u32 = 5000;
/// The largest gap a resume replays; a larger one starts with a snapshot.
pub(crate) const MAX_REPLAY: u64 = 1024 * 1024;

pub(crate) async fn handle(
    state: &State,
    params: PtyAttach,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let pty_id = params.pty_id;
    let live = state.ptys.attach(pty_id);
    let screen =
        state.ptys.conversation(pty_id).and_then(|conversation| state.shells.screen(conversation));
    let (Some(mut live), Some(screen)) = (live, screen) else {
        return replay_only(state, params, responder).await;
    };
    let mark = live.mark();
    let scrollback = params.scrollback_rows.unwrap_or(DEFAULT_SCROLLBACK).min(MAX_SCROLLBACK);
    let resume =
        params.since_seq.filter(|since| mark.end.saturating_sub(since.get()) <= MAX_REPLAY);
    let mut next = match resume {
        Some(since) => {
            let range = read(state, pty_id, since).await?;
            if range.start == since {
                send_range(responder, &range, Some(mark)).await?;
                mark.end.max(since.get())
            } else {
                snapshot(state, &screen, scrollback, pty_id, mark, responder).await?
            }
        }
        None => snapshot(state, &screen, scrollback, pty_id, mark, responder).await?,
    };
    while let Some(delivery) = live.recv().await {
        match delivery {
            PtyDelivery::Live(PtyLive::Output { start, data }) => {
                let len = u64::try_from(data.len()).unwrap_or(u64::MAX);
                let stop = start.saturating_add(len);
                if stop <= next {
                    continue;
                }
                let skip = usize::try_from(next.saturating_sub(start)).unwrap_or(usize::MAX);
                let fresh = data.slice(skip.min(data.len())..);
                let seq = Seq::new(start.max(next));
                responder
                    .item(&PtyAttachItem::Output { seq, data: Base64Bytes::new(fresh.to_vec()) })
                    .await?;
                next = stop;
            }
            PtyDelivery::Live(PtyLive::Resized { at, size }) => {
                responder.item(&PtyAttachItem::Resized { seq: Seq::new(at), size }).await?;
            }
            PtyDelivery::Overflowed => {
                let last_seq = Seq::new(next);
                return Err(DaemonError::Respond { source: TransportError::Overflow { last_seq } });
            }
        }
    }
    Ok(())
}

/// Sends a snapshot of `screen`, then what the recording holds after its offset up to
/// `mark`, and returns the offset the live output continues from.
async fn snapshot(
    state: &State,
    screen: &ScreenHandle,
    scrollback: u32,
    pty_id: PtyId,
    mark: AttachMark,
    responder: &Responder,
) -> Result<u64, DaemonError> {
    let rows = usize::try_from(scrollback).unwrap_or(usize::MAX);
    let capture = screen.snapshot(rows).await.map_err(|source| DaemonError::Screen { source })?;
    responder
        .item(&PtyAttachItem::Snapshot { seq: capture.at, snapshot: capture.snapshot })
        .await?;
    // Output stored after the screen's offset but offered before this client attached
    // reaches it from the recording.
    let range = read(state, pty_id, capture.at).await?;
    send_range(responder, &range, Some(mark)).await?;
    Ok(mark.end.max(capture.at.get()))
}

/// A PTY whose shell no longer runs: a resume replays what the recording holds and the
/// stream ends; without `since_seq` there is no screen to show.
async fn replay_only(
    state: &State,
    params: PtyAttach,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let pty_id = params.pty_id;
    let known = state.readers.with(move |conn| efr_store::shells::get(conn, pty_id)).await?;
    let (Some(_), Some(since)) = (known, params.since_seq) else {
        return Err(DaemonError::PtyNotFound { pty_id });
    };
    let range = read(state, pty_id, since).await?;
    send_range(responder, &range, None).await
}

async fn read(state: &State, pty_id: PtyId, from: Seq) -> Result<RecordedRange, DaemonError> {
    Ok(state.recordings.read_range(pty_id, from, Seq::new(u64::MAX)).await?)
}

/// Sends the output and the sizes of `range` in the order of the recording: a size at
/// offset `n` after the output before `n`. With a `mark`, only what was offered before
/// the client registered; the client gets the rest live.
async fn send_range(
    responder: &Responder,
    range: &RecordedRange,
    mark: Option<AttachMark>,
) -> Result<(), DaemonError> {
    let end = mark.map_or(u64::MAX, |mark| mark.end);
    let mut sizes_at_end = mark.map_or(u32::MAX, |mark| mark.sizes_at_end);
    let mut resizes = range.resizes.iter().peekable();
    for chunk in &range.chunks {
        let seq = chunk.seq.get();
        while let Some(resize) = resizes.next_if(|resize| resize.seq.get() <= seq) {
            if !offered(resize.seq.get(), end, &mut sizes_at_end) {
                return Ok(());
            }
            let item = PtyAttachItem::Resized { seq: resize.seq, size: resize.size };
            responder.item(&item).await?;
        }
        if seq >= end {
            return Ok(());
        }
        let len = usize::try_from(end - seq).unwrap_or(usize::MAX).min(chunk.data.len());
        let data = Base64Bytes::new(chunk.data[..len].to_vec());
        responder.item(&PtyAttachItem::Output { seq: chunk.seq, data }).await?;
    }
    for resize in resizes {
        if !offered(resize.seq.get(), end, &mut sizes_at_end) {
            return Ok(());
        }
        responder.item(&PtyAttachItem::Resized { seq: resize.seq, size: resize.size }).await?;
    }
    Ok(())
}

/// True when the size recorded at `seq` was offered before the mark of `end` and
/// `sizes_at_end`, which counts down the sizes at `end` that are left.
fn offered(seq: u64, end: u64, sizes_at_end: &mut u32) -> bool {
    match seq.cmp(&end) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Equal if *sizes_at_end > 0 => {
            *sizes_at_end -= 1;
            true
        }
        _ => false,
    }
}

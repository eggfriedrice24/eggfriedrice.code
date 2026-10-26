//! The live connections of each device, and the revokes that close them.
//!
//! `device.prove` adds its connection here before it reads the device's row, and the
//! feed of revokes was subscribed when the daemon started. So a revoke that commits
//! before the read shows in the row, and one that commits after it reaches [`follow`],
//! which closes the device's connections at once: the transport then drops the frames
//! still queued for them and cancels their requests. A follower that falls behind the
//! feed reads the row of every live device again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use efr_protocol::DeviceId;
use efr_store::Readers;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio_util::sync::CancellationToken;

/// The live connections of each device that proved, or is proving, its key.
#[derive(Debug, Default)]
pub(crate) struct LiveDevices {
    inner: Mutex<HashMap<DeviceId, Vec<(u64, CancellationToken)>>>,
}

impl LiveDevices {
    /// Adds the connection `key` of `device`, which `close` closes.
    pub(crate) fn add(&self, device: DeviceId, key: u64, close: CancellationToken) {
        self.lock().entry(device).or_default().push((key, close));
    }

    /// Takes out one entry of the connection `key` for `device`: the one of a proof that
    /// was refused.
    pub(crate) fn remove(&self, device: DeviceId, key: u64) {
        let mut inner = self.lock();
        if let Some(conns) = inner.get_mut(&device) {
            if let Some(at) = conns.iter().position(|(held, _)| *held == key) {
                conns.remove(at);
            }
            if conns.is_empty() {
                inner.remove(&device);
            }
        }
    }

    /// Forgets the connection `key`, for every device.
    pub(crate) fn forget(&self, key: u64) {
        self.lock().retain(|_, conns| {
            conns.retain(|(held, _)| *held != key);
            !conns.is_empty()
        });
    }

    /// Closes every connection of `device` and forgets them; how many there were.
    pub(crate) fn close(&self, device: DeviceId) -> usize {
        let conns = self.lock().remove(&device).unwrap_or_default();
        for (_, close) in &conns {
            close.cancel();
        }
        conns.len()
    }

    /// The devices with a live connection.
    pub(crate) fn devices(&self) -> Vec<DeviceId> {
        self.lock().keys().copied().collect()
    }

    /// How many connections of `device` are live.
    #[cfg(test)]
    pub(crate) fn count(&self, device: DeviceId) -> usize {
        self.lock().get(&device).map_or(0, Vec::len)
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<DeviceId, Vec<(u64, CancellationToken)>>> {
        // NOTE: each critical section leaves the map whole, so a poisoned lock is good.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Closes the connections of each device that `feed` names as revoked, until `stop`.
pub(crate) async fn follow(
    live: Arc<LiveDevices>,
    readers: Readers,
    mut feed: broadcast::Receiver<DeviceId>,
    stop: CancellationToken,
) {
    loop {
        let revoked = tokio::select! {
            () = stop.cancelled() => return,
            revoked = feed.recv() => revoked,
        };
        match revoked {
            Ok(device) => {
                let closed = live.close(device);
                tracing::info!(device_id = %device, closed, "a revoked device was closed");
            }
            Err(RecvError::Lagged(missed)) => {
                tracing::warn!(missed, "the revoke feed fell behind; checking every live device");
                recheck(&live, &readers).await;
            }
            Err(RecvError::Closed) => return,
        }
    }
}

/// Closes each live device whose row is gone, revoked or cannot be read.
async fn recheck(live: &LiveDevices, readers: &Readers) {
    for device in live.devices() {
        let row = readers.with(move |conn| efr_store::devices::get(conn, device)).await;
        let active = matches!(&row, Ok(Some(row)) if row.is_active());
        if !active {
            let closed = live.close(device);
            tracing::info!(device_id = %device, closed, "a device that is not active was closed");
        }
    }
}

#[cfg(test)]
mod tests;

//! `FakeProcView`: the process facts that a test daemon's routing reads, set by the
//! test, so no routing test depends on a real process or a real terminal.
//!
//! Every pid is alive until the test marks it dead. Every socket peer runs on the
//! terminal device the test sets with [`FakeProcView::peer_on`] (none at first, so no
//! pick-up happens unless a test asks for one), and each terminal path has the device
//! the test gives it with [`FakeProcView::device`].

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use efr_daemon::ProcView;

/// Process facts that a test sets. Cheap to clone: clones share the facts.
#[derive(Debug, Clone, Default)]
pub struct FakeProcView {
    inner: Arc<Mutex<Facts>>,
}

#[derive(Debug, Default)]
struct Facts {
    dead: HashSet<u32>,
    peer: Option<u64>,
    devices: HashMap<String, u64>,
}

impl FakeProcView {
    /// Facts where every pid runs, no peer runs on a terminal and no terminal is a
    /// device.
    pub fn new() -> Self {
        FakeProcView::default()
    }

    /// Marks the process `pid` as exited.
    pub fn kill(&self, pid: u32) {
        self.lock().dead.insert(pid);
    }

    /// Gives the terminal `tty` the device number `device`.
    pub fn device(&self, tty: &str, device: u64) {
        self.lock().devices.insert(tty.to_owned(), device);
    }

    /// Makes every socket peer run on the terminal with the device number `device`;
    /// `None` on none.
    pub fn peer_on(&self, device: Option<u64>) {
        self.lock().peer = device;
    }

    /// Gives the terminal `tty` a device of its own and makes every peer run on it, so
    /// a prompt from `tty` passes the peer check of a pick-up.
    pub fn peer_on_tty(&self, tty: &str) {
        let mut facts = self.lock();
        let next = u64::try_from(facts.devices.len()).unwrap_or(u64::MAX).saturating_add(1);
        let device = *facts.devices.entry(tty.to_owned()).or_insert(next);
        facts.peer = Some(device);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Facts> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ProcView for FakeProcView {
    fn alive(&self, pid: u32) -> bool {
        !self.lock().dead.contains(&pid)
    }

    fn tty_of(&self, _pid: u32) -> Option<u64> {
        self.lock().peer
    }

    fn device_of(&self, tty: &str) -> Option<u64> {
        self.lock().devices.get(tty).copied()
    }
}

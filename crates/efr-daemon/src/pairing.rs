//! The open pairings of new devices, such as a phone. They live in memory only: a
//! restart of efrd ends every pairing, and the token of a QR code is then good for
//! nothing.
//!
//! The steps, one method each:
//!
//! 1. `device.pair_start` (`efr device add`, Unix socket, scope `admin`) opens a
//!    pairing with [`Pairings::open`]: a one-time token of [`TOKEN_BYTES`] random
//!    bytes that expires after [`PAIRING_TTL`]. efrd keeps only the SHA-256 of the
//!    token; the QR code holds the token. The pairing lives as long as the request:
//!    [`Opened`] ends it when it is dropped.
//! 2. `device.enroll` (the tailnet listener only, before the proof) sends the token,
//!    the phone's name and its key. [`Pairings::check`] finds the pairing; a wrong
//!    token counts as a failed attempt, and after [`MAX_FAILED_ENROLLS`] of them every
//!    open pairing ends (`attempts`). [`Pairings::enroll`] then spends the token (it
//!    is single use), builds the device row from the whois facts of the connection
//!    ([`EnrollPeer`]) and sends the facts to the terminal. Nothing is written yet.
//! 3. `device.pair_confirm` (the terminal) answers with [`Pairings::answer`]. A no
//!    ends the pairing (`refused`). A yes writes the row ([`Accepted::complete`]), and
//!    only then does the phone get its device id.
//!
//! A pairing that waits for the terminal ends at the expiry of its token, but never
//! less than [`CONFIRM_WINDOW`] after the phone enrolled. A phone that leaves before
//! the answer ends the pairing too: a device that no phone knows must not exist, so
//! efrd revokes a row whose phone left while it was written.

use std::collections::HashMap;
use std::fmt;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use efr_protocol::{DeviceId, PairEnd, PairingId, ScopeName, SecretText};
use efr_stdx::id::uuid_v7;
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;
use efr_store::WriterHandle;
use efr_store::devices::{DevicePeer, DevicePublicKey, NewDevice};
use efr_transport::ConnectionContext;
use jiff::{SignedDuration, Timestamp};
use sha2::{Digest as _, Sha256};
use tokio::sync::{mpsc, oneshot};

use crate::DaemonError;
use crate::error::PairingProblem;

/// How long the token of a QR code is good for.
pub(crate) const PAIRING_TTL: Duration = Duration::from_secs(5 * 60);

/// The least time that the terminal has to answer after a phone enrolled, also when
/// the token expires sooner.
pub(crate) const CONFIRM_WINDOW: Duration = Duration::from_secs(2 * 60);

/// The random bytes of a token: 144 bits, 24 characters of base64url.
pub(crate) const TOKEN_BYTES: usize = 18;

/// The most pairings that can be open at one time.
pub(crate) const MAX_OPEN: usize = 4;

/// The wrong tokens that `device.enroll` takes while a pairing is open. The next one
/// ends every open pairing.
pub(crate) const MAX_FAILED_ENROLLS: u32 = 5;

/// The scopes of an enrolled device: those of the phone origin. `terminal` is off for
/// a phone, and `admin` is never granted to a device.
pub(crate) const DEVICE_SCOPES: [ScopeName; 3] =
    [ScopeName::Read, ScopeName::Operate, ScopeName::Approve];

/// The updates that one pairing sends: a `pending` and its end.
const UPDATES: usize = 4;

/// The hex digits of a key fingerprint: the first 8 bytes of the key's SHA-256.
const FINGERPRINT_BYTES: usize = 8;

/// What Tailscale's whois said about the peer of a remote connection, and efrd's own
/// tailnet: the facts that a device row pins and that the terminal shows.
///
/// [`enroll_peer`] reads it from the admission of the connection that the tailnet
/// listener made ([`crate::remote::Admission`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnrollPeer {
    /// `Node.StableID` of the phone's node.
    pub(crate) node_stable_id: String,
    /// `Node.Name`, the MagicDNS name of the phone's node.
    pub(crate) node_name: String,
    /// `UserProfile.ID` of the node's user.
    pub(crate) user_id: i64,
    /// `UserProfile.LoginName`, for display only.
    pub(crate) user_login: String,
    /// efrd's own `CurrentTailnet.StableID`.
    pub(crate) tailnet_stable_id: String,
    /// The operating system of the phone's node, when Tailscale knows it.
    pub(crate) os: Option<String>,
}

/// The whois facts of the connection of `context`: `None` for a connection through the
/// Unix socket, or a remote one that the listener did not admit with them.
pub(crate) fn enroll_peer(context: &ConnectionContext) -> Option<EnrollPeer> {
    let admission = crate::remote::admission(context)?;
    let peer = admission.peer();
    Some(EnrollPeer {
        node_stable_id: peer.node_stable_id.clone(),
        node_name: peer.node_name.clone(),
        user_id: peer.user_id,
        user_login: peer.login_name.clone(),
        tailnet_stable_id: admission.tailnet().to_owned(),
        // NOTE: the whois of efr-http has no operating system.
        os: None,
    })
}

/// The fingerprint of a device key that the terminal and the phone show:
/// `SHA256:` and the first 8 bytes of the SHA-256 of the 65 bytes of the key, in
/// lowercase hex, such as `SHA256:9f86d081884c7d65`.
pub(crate) fn key_fingerprint(key: &DevicePublicKey) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let mut out = String::from("SHA256:");
    for byte in &digest[..FINGERPRINT_BYTES] {
        // NOTE: writing to a String cannot fail.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Refuses a device name that the store would not keep, before a terminal or a phone
/// waits for it. `device_id` is any id; only the name is checked.
fn check_name(device_id: DeviceId, name: &str) -> Result<(), PairingProblem> {
    // NOTE: the store owns the rules of a name; a device with this name and stand-in
    // values elsewhere asks it, so the two never differ.
    let mut key = [0x04; DevicePublicKey::LEN];
    key[1] = 0;
    let key = DevicePublicKey::from_x963(&key).map_err(|_| PairingProblem::BadKey)?;
    let peer = DevicePeer {
        node_stable_id: "n".to_owned(),
        user_id: 0,
        tailnet_stable_id: "t".to_owned(),
    };
    NewDevice::new(device_id, name, key, "f", peer, &[])
        .map(|_| ())
        .map_err(|_| PairingProblem::BadField { field: "name" })
}

/// The open pairings.
pub(crate) struct Pairings {
    table: Arc<Mutex<Table>>,
    clock: Arc<dyn Clock>,
    rng: Arc<dyn Rng>,
}

impl fmt::Debug for Pairings {
    // NOTE: the table holds the hashes of live tokens, which no log line needs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pairings").field("open", &lock(&self.table).open.len()).finish()
    }
}

impl Pairings {
    pub(crate) fn new(clock: Arc<dyn Clock>, rng: Arc<dyn Rng>) -> Self {
        Pairings {
            table: Arc::new(Mutex::new(Table { open: HashMap::new(), failed: 0 })),
            clock,
            rng,
        }
    }

    /// Opens a pairing that gives the device `name`, or the name that the phone sends.
    pub(crate) fn open(&self, name: Option<String>) -> Result<Opened, PairingProblem> {
        let pairing_id = PairingId::from_uuid(uuid_v7(&*self.clock, &*self.rng));
        if let Some(name) = &name {
            check_name(DeviceId::from_uuid(*pairing_id.as_uuid()), name)?;
        }
        let mut bytes = [0; TOKEN_BYTES];
        self.rng.fill_bytes(&mut bytes);
        let token = SecretText::new(URL_SAFE_NO_PAD.encode(bytes));
        bytes.fill(0);
        let hash = token_hash(&token);
        let expires_at = after(self.clock.now(), PAIRING_TTL);
        let (updates, receiver) = mpsc::channel(UPDATES);
        let mut table = lock(&self.table);
        if table.open.len() >= MAX_OPEN {
            return Err(PairingProblem::TooMany);
        }
        if table.open.is_empty() {
            // NOTE: the count of wrong tokens protects the pairings that are open; a
            // new one after they all ended starts from zero.
            table.failed = 0;
        }
        let entry =
            Entry { hash, expires_at, deadline: expires_at, name, updates, phase: Phase::Open };
        table.open.insert(pairing_id, entry);
        drop(table);
        Ok(Opened {
            pairing_id,
            token,
            expires_at,
            updates: receiver,
            table: Arc::clone(&self.table),
        })
    }

    /// The open pairing whose token is `token`, before anything is spent. A token that
    /// matches no pairing counts as a failed attempt while a pairing is open; one that
    /// matches a used or expired pairing does not, but it is refused the same way.
    pub(crate) fn check(&self, token: &SecretText) -> Result<PairingId, PairingProblem> {
        let hash = token_hash(token);
        let now = self.clock.now();
        let mut table = lock(&self.table);
        let found = table.open.iter().find(|(_, entry)| same(&entry.hash, &hash));
        match found {
            Some((id, entry)) if entry.is_open_at(now) => Ok(*id),
            Some(_) => Err(PairingProblem::BadToken),
            None => {
                table.failed(now);
                Err(PairingProblem::BadToken)
            }
        }
    }

    /// Spends the token of `pairing_id` for the device that the phone of `peer`
    /// describes, and sends the facts to the terminal. The phone waits on the returned
    /// [`EnrollWait`] for the answer.
    pub(crate) fn enroll(
        &self,
        pairing_id: PairingId,
        token: &SecretText,
        phone_name: &str,
        key: DevicePublicKey,
        peer: EnrollPeer,
    ) -> Result<EnrollWait, PairingProblem> {
        let device_id = DeviceId::from_uuid(uuid_v7(&*self.clock, &*self.rng));
        check_name(device_id, phone_name)?;
        let hash = token_hash(token);
        let now = self.clock.now();
        let fingerprint = key_fingerprint(&key);
        let mut table = lock(&self.table);
        let Some(entry) = table.open.get_mut(&pairing_id) else {
            return Err(PairingProblem::BadToken);
        };
        if !same(&entry.hash, &hash) || !entry.is_open_at(now) {
            return Err(PairingProblem::BadToken);
        }
        let name = entry.name.clone().unwrap_or_else(|| phone_name.to_owned());
        let row = DevicePeer {
            node_stable_id: peer.node_stable_id.clone(),
            user_id: peer.user_id,
            tailnet_stable_id: peer.tailnet_stable_id.clone(),
        };
        let device = NewDevice::new(device_id, name, key, &fingerprint, row, &DEVICE_SCOPES)
            .map_err(|error| PairingProblem::BadField { field: device_field(&error) })?;
        let (answer, waiting) = oneshot::channel();
        entry.phase = Phase::Pending { device: Box::new(device), answer };
        entry.deadline = entry.deadline.max(after(now, CONFIRM_WINDOW));
        let facts = PendingFacts {
            device_name: phone_name.to_owned(),
            node_name: peer.node_name.trim_end_matches('.').to_owned(),
            user_login: peer.user_login,
            key_fingerprint: fingerprint,
            os: peer.os,
        };
        // NOTE: the channel has room for every update of a pairing, so a full one
        // means that its request is gone, and the drop of `Opened` ends the pairing.
        let _ = entry.updates.try_send(Update::Pending(facts));
        drop(table);
        Ok(EnrollWait { pairing_id, answer: Some(waiting), table: Arc::clone(&self.table) })
    }

    /// The terminal's answer for `pairing_id`: a no ends the pairing at once, a yes
    /// hands over the device to write.
    pub(crate) fn answer(
        &self,
        pairing_id: PairingId,
        accept: bool,
    ) -> Result<Decision, PairingProblem> {
        let mut table = lock(&self.table);
        let pending = table
            .open
            .get(&pairing_id)
            .is_some_and(|entry| matches!(entry.phase, Phase::Pending { .. }));
        if !pending {
            return Err(PairingProblem::NotPending { pairing_id });
        }
        let Some(entry) = table.open.remove(&pairing_id) else {
            return Err(PairingProblem::NotPending { pairing_id });
        };
        drop(table);
        let Entry { updates, phase, .. } = entry;
        let Phase::Pending { device, answer } = phase else {
            unreachable!("the entry was checked to be pending under the same lock");
        };
        if accept {
            Ok(Decision::Accepted(Box::new(Accepted { device: *device, answer, updates })))
        } else {
            let _ = answer.send(Answer::Ended(PairEnd::Refused));
            let _ = updates.try_send(Update::Ended(PairEnd::Refused));
            Ok(Decision::Refused)
        }
    }

    /// How many pairings are open.
    #[cfg(test)]
    pub(crate) fn open_count(&self) -> usize {
        lock(&self.table).open.len()
    }
}

/// The open pairings, and the wrong tokens since the first of them opened.
struct Table {
    open: HashMap<PairingId, Entry>,
    failed: u32,
}

impl fmt::Debug for Table {
    // NOTE: the entries hold the hashes of live tokens, which no log line needs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Table")
            .field("open", &self.open.len())
            .field("failed", &self.failed)
            .finish()
    }
}

impl Table {
    /// Counts a wrong token. The one after [`MAX_FAILED_ENROLLS`] ends every pairing
    /// that waits for a phone; one that waits for the terminal has its phone already.
    fn failed(&mut self, now: Timestamp) {
        if self.open.is_empty() {
            return;
        }
        self.failed += 1;
        if self.failed <= MAX_FAILED_ENROLLS {
            return;
        }
        let ended: Vec<PairingId> = self
            .open
            .iter()
            .filter(|(_, entry)| matches!(entry.phase, Phase::Open))
            .map(|(id, _)| *id)
            .collect();
        tracing::warn!(
            pairings = ended.len(),
            at = %now,
            "ended the open pairings after too many wrong tokens"
        );
        for id in ended {
            if let Some(entry) = self.open.remove(&id) {
                let _ = entry.updates.try_send(Update::Ended(PairEnd::Attempts));
            }
        }
    }
}

struct Entry {
    /// The SHA-256 of the token.
    hash: [u8; 32],
    /// When the token expires.
    expires_at: Timestamp,
    /// When the pairing ends: the expiry, or later while the terminal answers.
    deadline: Timestamp,
    /// The name that `device.pair_start` gave.
    name: Option<String>,
    updates: mpsc::Sender<Update>,
    phase: Phase,
}

impl Entry {
    /// True while the token can still be spent at `now`.
    fn is_open_at(&self, now: Timestamp) -> bool {
        matches!(self.phase, Phase::Open) && now < self.expires_at
    }
}

enum Phase {
    /// The QR code is out; no phone has sent the token.
    Open,
    /// A phone sent the token; the terminal answers.
    Pending { device: Box<NewDevice>, answer: oneshot::Sender<Answer> },
}

/// What a pairing tells its `device.pair_start` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Update {
    /// A phone sent the token; the terminal must answer.
    Pending(PendingFacts),
    /// The device is enrolled.
    Paired { device_id: DeviceId },
    /// The pairing ended without a device.
    Ended(PairEnd),
    /// The store did not take the device.
    Failed,
}

/// The facts of a phone that the terminal shows before it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingFacts {
    pub(crate) device_name: String,
    pub(crate) node_name: String,
    pub(crate) user_login: String,
    pub(crate) key_fingerprint: String,
    pub(crate) os: Option<String>,
}

/// What the phone's `device.enroll` gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answer {
    /// The device is enrolled.
    Paired { device_id: DeviceId, scopes: Vec<ScopeName> },
    /// The pairing ended without a device.
    Ended(PairEnd),
    /// The store did not take the device.
    Failed,
}

/// The terminal's answer, from [`Pairings::answer`].
#[derive(Debug)]
pub(crate) enum Decision {
    /// No: the pairing ended.
    Refused,
    /// Yes: write the device.
    Accepted(Box<Accepted>),
}

/// A device that the terminal accepted and that is not written yet.
#[derive(Debug)]
pub(crate) struct Accepted {
    device: NewDevice,
    answer: oneshot::Sender<Answer>,
    updates: mpsc::Sender<Update>,
}

impl Accepted {
    /// Writes the device and tells the phone and the terminal. A phone that left
    /// before gets no row; one that leaves while the row is written gets it revoked
    /// at once, so no device stays that no phone knows.
    ///
    /// The caller runs it in a task of its own: a client that goes away must not stop
    /// it between the write and the answer.
    pub(crate) async fn complete(
        self: Box<Self>,
        writer: WriterHandle,
    ) -> Result<DeviceId, DaemonError> {
        let Accepted { device, answer, updates } = *self;
        let device_id = device.device_id();
        if answer.is_closed() {
            let _ = updates.try_send(Update::Ended(PairEnd::Expired));
            return Err(DaemonError::Pairing { problem: PairingProblem::PhoneLeft });
        }
        let stored = match writer.device_insert(device).await {
            Ok(stored) => stored,
            Err(error) => {
                let _ = answer.send(Answer::Failed);
                let _ = updates.try_send(Update::Failed);
                return Err(error.into());
            }
        };
        let scopes = stored.scopes.clone();
        if answer.send(Answer::Paired { device_id, scopes }).is_err() {
            if let Err(error) = writer.device_revoke(device_id).await {
                tracing::error!(%device_id, %error, "a device whose phone left was not revoked");
            }
            let _ = updates.try_send(Update::Ended(PairEnd::Expired));
            return Err(DaemonError::Pairing { problem: PairingProblem::PhoneLeft });
        }
        tracing::info!(%device_id, "paired a device");
        let _ = updates.try_send(Update::Paired { device_id });
        Ok(device_id)
    }
}

/// A pairing that `device.pair_start` holds open. Dropping it ends the pairing: its
/// token is then good for nothing, and a phone that waits for the terminal hears
/// `refused`.
#[derive(Debug)]
pub(crate) struct Opened {
    pub(crate) pairing_id: PairingId,
    /// The token for the QR code. efrd keeps only its hash.
    pub(crate) token: SecretText,
    pub(crate) expires_at: Timestamp,
    updates: mpsc::Receiver<Update>,
    table: Arc<Mutex<Table>>,
}

impl Opened {
    /// The next update; `None` when nothing can send one any more.
    pub(crate) async fn next(&mut self) -> Option<Update> {
        self.updates.recv().await
    }

    /// When the pairing ends unless something comes first; `None` once the terminal
    /// accepted, while the device is written.
    pub(crate) fn deadline(&self) -> Option<Timestamp> {
        lock(&self.table).open.get(&self.pairing_id).map(|entry| entry.deadline)
    }

    /// Ends the pairing when its deadline passed at `now`: true when it ended. A phone
    /// that waits for the terminal hears `expired`.
    pub(crate) fn expire(&self, now: Timestamp) -> bool {
        let mut table = lock(&self.table);
        let due = table.open.get(&self.pairing_id).is_some_and(|entry| now >= entry.deadline);
        if !due {
            return false;
        }
        if let Some(Entry { phase: Phase::Pending { answer, .. }, .. }) =
            table.open.remove(&self.pairing_id)
        {
            let _ = answer.send(Answer::Ended(PairEnd::Expired));
        }
        true
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        let removed = lock(&self.table).open.remove(&self.pairing_id);
        if let Some(Entry { phase: Phase::Pending { answer, .. }, .. }) = removed {
            let _ = answer.send(Answer::Ended(PairEnd::Refused));
        }
    }
}

/// The phone's wait for the terminal's answer. Dropping it before the answer ends the
/// pairing, because the phone left.
#[derive(Debug)]
pub(crate) struct EnrollWait {
    pairing_id: PairingId,
    answer: Option<oneshot::Receiver<Answer>>,
    table: Arc<Mutex<Table>>,
}

impl EnrollWait {
    /// The answer; `ended` for a pairing that ended in a way that sent none.
    pub(crate) async fn outcome(mut self) -> Answer {
        let Some(answer) = self.answer.as_mut() else {
            return Answer::Ended(PairEnd::Unknown);
        };
        let outcome = answer.await.unwrap_or(Answer::Ended(PairEnd::Unknown));
        self.answer = None;
        outcome
    }
}

impl Drop for EnrollWait {
    fn drop(&mut self) {
        if self.answer.is_none() {
            return;
        }
        let mut table = lock(&self.table);
        let waits = table
            .open
            .get(&self.pairing_id)
            .is_some_and(|entry| matches!(entry.phase, Phase::Pending { .. }));
        if waits && let Some(entry) = table.open.remove(&self.pairing_id) {
            // NOTE: the phone left, so the token gives no device any more; the protocol
            // names that end `expired`.
            let _ = entry.updates.try_send(Update::Ended(PairEnd::Expired));
        }
    }
}

/// The field of a device that the store refused.
fn device_field(error: &efr_store::StoreError) -> &'static str {
    match error {
        efr_store::StoreError::DeviceFieldInvalid { field } => field,
        _ => "device",
    }
}

fn lock(table: &Mutex<Table>) -> MutexGuard<'_, Table> {
    table.lock().unwrap_or_else(PoisonError::into_inner)
}

fn token_hash(token: &SecretText) -> [u8; 32] {
    Sha256::digest(token.expose_secret().as_bytes()).into()
}

/// Compares two hashes in a time that does not depend on where they differ.
fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// `at` plus `duration`, or the latest time that jiff knows.
fn after(at: Timestamp, duration: Duration) -> Timestamp {
    SignedDuration::try_from(duration)
        .ok()
        .and_then(|duration| at.checked_add(duration).ok())
        .unwrap_or(Timestamp::MAX)
}

#[cfg(test)]
mod tests;

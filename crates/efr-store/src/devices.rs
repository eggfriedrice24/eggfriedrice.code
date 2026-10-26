//! The enrolled devices (migration 11), such as a phone.
//!
//! A row holds what efrd checks on each connection of a device: its P-256 public key,
//! the Tailscale node and user that paired it, the tailnet of efrd at that time, and
//! the scopes of its connections. efrd writes a row only after the user confirms the
//! pairing in the terminal
//! ([`WriterHandle::device_insert`](crate::WriterHandle::device_insert)). A revoke
//! keeps the row and sets its time
//! ([`WriterHandle::device_revoke`](crate::WriterHandle::device_revoke)); a revoked
//! device never comes back. The writer announces each revoke after its commit
//! ([`WriterHandle::subscribe_revocations`](crate::WriterHandle::subscribe_revocations)),
//! so efrd can close the device's live connections. The table is not a projection: no
//! event holds a device, and a rebuild leaves it alone.
//!
//! The store checks the shape of each value and nothing more: it does not parse the
//! key as a point of the curve, and it does not derive the fingerprint. efrd does both.

use std::collections::BTreeSet;
use std::fmt;

use efr_protocol::{DeviceId, ScopeName};
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension as _, Row, params};

use crate::{StoreError, sql};

const TABLE: &str = "devices";

const COLUMNS: &str = "device_id, name, public_key, key_fingerprint, node_stable_id, user_id, \
                       tailnet_stable_id, scopes, created_at, last_seen_at, revoked_at";

/// The most characters (Unicode code points) of a device name.
pub const DEVICE_NAME_MAX_CHARS: usize = 64;

/// The most bytes of a key fingerprint and of a Tailscale StableID.
pub const DEVICE_TEXT_MAX_BYTES: usize = 128;

/// A P-256 public key in the uncompressed X9.63 form: `0x04`, then the 32 bytes of X
/// and the 32 bytes of Y.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DevicePublicKey([u8; DevicePublicKey::LEN]);

impl DevicePublicKey {
    /// The length of the key.
    pub const LEN: usize = 65;

    /// The key in `bytes`. Fails with [`StoreError::DeviceFieldInvalid`] unless it has
    /// 65 bytes and starts with `0x04`. It does not check that the point is on the
    /// curve; the verifier of the signatures does.
    pub fn from_x963(bytes: &[u8]) -> Result<Self, StoreError> {
        match <[u8; Self::LEN]>::try_from(bytes) {
            Ok(key) if key[0] == 0x04 => Ok(DevicePublicKey(key)),
            _ => Err(StoreError::DeviceFieldInvalid { field: "public_key" }),
        }
    }

    /// The 65 bytes of the key.
    pub const fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }
}

impl fmt::Debug for DevicePublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: a public key is no secret, but 130 hex digits make every log line of a
        // device unreadable. The first bytes are enough to tell two keys apart.
        write!(f, "DevicePublicKey(")?;
        for byte in &self.0[..9] {
            write!(f, "{byte:02x}")?;
        }
        write!(f, "..)")
    }
}

/// A device to insert after the user confirmed its pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDevice {
    device_id: DeviceId,
    name: String,
    public_key: DevicePublicKey,
    key_fingerprint: String,
    node_stable_id: String,
    user_id: i64,
    tailnet_stable_id: String,
    scopes: Vec<ScopeName>,
}

/// Where a pairing came from: the facts of Tailscale's whois of its connection, and the
/// tailnet of efrd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePeer {
    /// The `Node.StableID` of the phone.
    pub node_stable_id: String,
    /// The `UserProfile.ID` of the node's user (a Tailscale `UserID`, an int64).
    pub user_id: i64,
    /// efrd's own `CurrentTailnet.StableID`.
    pub tailnet_stable_id: String,
}

impl NewDevice {
    /// A device with `scopes`, which the store keeps sorted and without repeats.
    ///
    /// Fails with [`StoreError::DeviceFieldInvalid`] for a value of the wrong shape: a
    /// name that is empty, only white space, longer than [`DEVICE_NAME_MAX_CHARS`], or
    /// that holds a control character, a bidirectional control or a zero-width
    /// character (the terminal shows the name to the user, who confirms by it); a
    /// fingerprint or a StableID that is empty, longer than [`DEVICE_TEXT_MAX_BYTES`] or
    /// not printable ASCII; or a scope list that holds `admin`, which only the Unix
    /// socket has.
    pub fn new(
        device_id: DeviceId,
        name: impl Into<String>,
        public_key: DevicePublicKey,
        key_fingerprint: impl Into<String>,
        peer: DevicePeer,
        scopes: &[ScopeName],
    ) -> Result<Self, StoreError> {
        let name = name.into();
        let key_fingerprint = key_fingerprint.into();
        if !name_is_valid(&name) {
            return Err(StoreError::DeviceFieldInvalid { field: "name" });
        }
        if !text_is_valid(&key_fingerprint) {
            return Err(StoreError::DeviceFieldInvalid { field: "key_fingerprint" });
        }
        if !text_is_valid(&peer.node_stable_id) {
            return Err(StoreError::DeviceFieldInvalid { field: "node_stable_id" });
        }
        if !text_is_valid(&peer.tailnet_stable_id) {
            return Err(StoreError::DeviceFieldInvalid { field: "tailnet_stable_id" });
        }
        if scopes.contains(&ScopeName::Admin) {
            return Err(StoreError::DeviceFieldInvalid { field: "scopes" });
        }
        let scopes = scopes.iter().copied().collect::<BTreeSet<_>>().into_iter().collect();
        Ok(NewDevice {
            device_id,
            name,
            public_key,
            key_fingerprint,
            node_stable_id: peer.node_stable_id,
            user_id: peer.user_id,
            tailnet_stable_id: peer.tailnet_stable_id,
            scopes,
        })
    }

    /// The device's id.
    pub fn device_id(&self) -> DeviceId {
        self.device_id
    }
}

/// An enrolled device as its row holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Device {
    /// The device.
    pub device_id: DeviceId,
    /// The name that the phone sent.
    pub name: String,
    /// Its P-256 public key.
    pub public_key: DevicePublicKey,
    /// The fingerprint of the key that the terminal showed.
    pub key_fingerprint: String,
    /// The `Node.StableID` of the phone at the pairing.
    pub node_stable_id: String,
    /// The `UserProfile.ID` of the node's user at the pairing.
    pub user_id: i64,
    /// efrd's `CurrentTailnet.StableID` at the pairing.
    pub tailnet_stable_id: String,
    /// The scopes of its connections, sorted, without repeats.
    pub scopes: Vec<ScopeName>,
    /// When the user confirmed the pairing.
    pub created_at: Timestamp,
    /// When a connection of the device last proved its key; `None` before the first.
    pub last_seen_at: Option<Timestamp>,
    /// When the device was revoked; `None` while it may connect.
    pub revoked_at: Option<Timestamp>,
}

impl Device {
    /// True while the device is not revoked.
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

/// What [`WriterHandle::device_revoke`](crate::WriterHandle::device_revoke) did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RevokeOutcome {
    /// This call revoked the device, at this time.
    Revoked {
        /// The time of the revoke.
        at: Timestamp,
    },
    /// The device was revoked before, at this time; nothing changed.
    AlreadyRevoked {
        /// The time of the earlier revoke.
        at: Timestamp,
    },
    /// No device has this id.
    Unknown,
}

/// The device `device_id`, also when it is revoked.
pub fn get(conn: &Connection, device_id: DeviceId) -> Result<Option<Device>, StoreError> {
    let raw = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM devices WHERE device_id = ?1"),
            [device_id.to_string()],
            RawDevice::from_row,
        )
        .optional()?;
    raw.map(RawDevice::decode).transpose()
}

/// Every device, the revoked ones too, oldest pairing first.
pub fn list(conn: &Connection) -> Result<Vec<Device>, StoreError> {
    let mut stmt =
        conn.prepare(&format!("SELECT {COLUMNS} FROM devices ORDER BY created_at, device_id"))?;
    let raw = stmt.query_map([], RawDevice::from_row)?.collect::<Result<Vec<_>, _>>()?;
    raw.into_iter().map(RawDevice::decode).collect()
}

/// Inserts `device` with the time `at`, inside the writer's transaction. Fails with
/// [`StoreError::DeviceExists`] for an id that has a row, and with
/// [`StoreError::DeviceKeyInUse`] for a key of a device that is not revoked.
pub(crate) fn insert(
    conn: &Connection,
    device: &NewDevice,
    at: Timestamp,
) -> Result<Device, StoreError> {
    if get(conn, device.device_id)?.is_some() {
        return Err(StoreError::DeviceExists { device_id: device.device_id });
    }
    let holder: Option<String> = conn
        .query_row(
            "SELECT device_id FROM devices WHERE public_key = ?1 AND revoked_at IS NULL",
            [&device.public_key.0[..]],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(holder) = holder {
        let device_id = sql::parse(&holder, TABLE, "device_id")?;
        return Err(StoreError::DeviceKeyInUse { device_id });
    }
    conn.execute(
        &format!(
            "INSERT INTO devices ({COLUMNS}) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL)"
        ),
        params![
            device.device_id.to_string(),
            device.name,
            &device.public_key.0[..],
            device.key_fingerprint,
            device.node_stable_id,
            device.user_id,
            device.tailnet_stable_id,
            sql::to_json(&device.scopes, "device scopes")?,
            sql::micros(at),
        ],
    )?;
    Ok(Device {
        device_id: device.device_id,
        name: device.name.clone(),
        public_key: device.public_key,
        key_fingerprint: device.key_fingerprint.clone(),
        node_stable_id: device.node_stable_id.clone(),
        user_id: device.user_id,
        tailnet_stable_id: device.tailnet_stable_id.clone(),
        scopes: device.scopes.clone(),
        created_at: at,
        last_seen_at: None,
        revoked_at: None,
    })
}

/// Revokes `device_id` at `at`, inside the writer's transaction.
pub(crate) fn revoke(
    conn: &Connection,
    device_id: DeviceId,
    at: Timestamp,
) -> Result<RevokeOutcome, StoreError> {
    let Some(device) = get(conn, device_id)? else {
        return Ok(RevokeOutcome::Unknown);
    };
    if let Some(before) = device.revoked_at {
        return Ok(RevokeOutcome::AlreadyRevoked { at: before });
    }
    conn.execute(
        "UPDATE devices SET revoked_at = ?2 WHERE device_id = ?1",
        params![device_id.to_string(), sql::micros(at)],
    )?;
    Ok(RevokeOutcome::Revoked { at })
}

/// Sets the time that `device_id` was last seen to `at`, inside the writer's
/// transaction; false, with no change, for a device that is unknown or revoked.
pub(crate) fn touch(
    conn: &Connection,
    device_id: DeviceId,
    at: Timestamp,
) -> Result<bool, StoreError> {
    let changed = conn.execute(
        "UPDATE devices SET last_seen_at = ?2 WHERE device_id = ?1 AND revoked_at IS NULL",
        params![device_id.to_string(), sql::micros(at)],
    )?;
    Ok(changed > 0)
}

/// A name that the terminal can show as it is: no control character, no
/// bidirectional control and no zero-width character, which could make a name look
/// like another.
fn name_is_valid(name: &str) -> bool {
    let chars = name.chars().count();
    (1..=DEVICE_NAME_MAX_CHARS).contains(&chars)
        && !name.trim().is_empty()
        && !name.chars().any(|c| c.is_control() || is_invisible_format(c))
}

/// The format characters that change how text around them reads: the bidirectional
/// marks, embeddings, overrides and isolates, the zero-width characters and the byte
/// order mark.
fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2069}'
            | '\u{feff}'
    )
}

fn text_is_valid(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= DEVICE_TEXT_MAX_BYTES
        && text.bytes().all(|byte| byte.is_ascii_graphic())
}

struct RawDevice {
    device_id: String,
    name: String,
    public_key: Vec<u8>,
    key_fingerprint: String,
    node_stable_id: String,
    user_id: i64,
    tailnet_stable_id: String,
    scopes: String,
    created_at: i64,
    last_seen_at: Option<i64>,
    revoked_at: Option<i64>,
}

impl RawDevice {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(RawDevice {
            device_id: row.get(0)?,
            name: row.get(1)?,
            public_key: row.get(2)?,
            key_fingerprint: row.get(3)?,
            node_stable_id: row.get(4)?,
            user_id: row.get(5)?,
            tailnet_stable_id: row.get(6)?,
            scopes: row.get(7)?,
            created_at: row.get(8)?,
            last_seen_at: row.get(9)?,
            revoked_at: row.get(10)?,
        })
    }

    fn decode(self) -> Result<Device, StoreError> {
        let time = |value: Option<i64>, column| {
            value.map(|value| sql::to_timestamp(value, TABLE, column)).transpose()
        };
        Ok(Device {
            device_id: sql::parse(&self.device_id, TABLE, "device_id")?,
            name: self.name,
            public_key: DevicePublicKey::from_x963(&self.public_key)
                .map_err(|source| sql::decode_error(TABLE, "public_key", source))?,
            key_fingerprint: self.key_fingerprint,
            node_stable_id: self.node_stable_id,
            user_id: self.user_id,
            tailnet_stable_id: self.tailnet_stable_id,
            scopes: sql::from_json(&self.scopes, TABLE, "scopes")?,
            created_at: sql::to_timestamp(self.created_at, TABLE, "created_at")?,
            last_seen_at: time(self.last_seen_at, "last_seen_at")?,
            revoked_at: time(self.revoked_at, "revoked_at")?,
        })
    }
}

#[cfg(test)]
mod tests;

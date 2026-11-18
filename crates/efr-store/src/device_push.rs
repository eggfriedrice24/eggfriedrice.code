//! The table `device_push`: the Expo push token of each enrolled device
//! (`0016_device_push.sql`).
//!
//! A row exists only for a device that is not revoked: a revoke deletes it in its own
//! transaction (`devices::revoke`), and a set refuses a revoked device. The token is a
//! secret, so it leaves this module only as [`SecretText`], whose `Debug` shows no
//! text. The store checks its form ([`is_expo_push_token`]) and nothing more.

use std::collections::HashSet;

use efr_protocol::{DeviceId, SecretText, is_expo_push_token};
use jiff::Timestamp;
use rusqlite::{Connection, params};

use crate::{StoreError, sql};

const TABLE: &str = "device_push";

/// A device that has push on: what a push needs of it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PushTarget {
    /// The device.
    pub device_id: DeviceId,
    /// The name that the phone sent at its pairing.
    pub name: String,
    /// Its Expo push token.
    pub token: SecretText,
}

/// Every device that is not revoked and has a token, oldest pairing first. With
/// `seen_since`, only the devices that proved their key at or after it.
pub fn targets(
    conn: &Connection,
    seen_since: Option<Timestamp>,
) -> Result<Vec<PushTarget>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT d.device_id, d.name, p.token FROM device_push p \
         JOIN devices d ON d.device_id = p.device_id \
         WHERE d.revoked_at IS NULL AND (?1 IS NULL OR d.last_seen_at >= ?1) \
         ORDER BY d.created_at, d.device_id",
    )?;
    let rows = stmt.query_map([seen_since.map(sql::micros)], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })?;
    rows.map(|row| {
        let (device_id, name, token) = row?;
        let device_id = sql::parse(&device_id, TABLE, "device_id")?;
        Ok(PushTarget { device_id, name, token: SecretText::new(token) })
    })
    .collect()
}

/// The devices that have a token.
pub fn devices(conn: &Connection) -> Result<HashSet<DeviceId>, StoreError> {
    let mut stmt = conn.prepare("SELECT device_id FROM device_push")?;
    let ids = stmt.query_map([], |row| row.get::<_, String>(0))?;
    ids.map(|id| sql::parse(&id?, TABLE, "device_id")).collect()
}

/// Sets the token of `device_id` to `token` at `at`, inside the writer's transaction;
/// false, with no change, for a device that is unknown or revoked. `None` deletes the
/// token and answers true. Fails with [`StoreError::DeviceFieldInvalid`] for a token
/// that is not an Expo push token.
pub(crate) fn set(
    conn: &Connection,
    device_id: DeviceId,
    token: Option<&SecretText>,
    at: Timestamp,
) -> Result<bool, StoreError> {
    let Some(token) = token else {
        remove(conn, device_id)?;
        return Ok(true);
    };
    if !is_expo_push_token(token.expose_secret()) {
        return Err(StoreError::DeviceFieldInvalid { field: "token" });
    }
    let changed = conn.execute(
        "INSERT INTO device_push (device_id, token, updated_at) \
         SELECT device_id, ?2, ?3 FROM devices WHERE device_id = ?1 AND revoked_at IS NULL \
         ON CONFLICT (device_id) DO UPDATE SET token = excluded.token, \
         updated_at = excluded.updated_at",
        params![device_id.to_string(), token.expose_secret(), sql::micros(at)],
    )?;
    Ok(changed > 0)
}

/// Deletes the token of `device_id` when it is still `token`, inside the writer's
/// transaction; false when the row holds another token or none. A newer token that the
/// app sent after the push stays.
pub(crate) fn drop_token(
    conn: &Connection,
    device_id: DeviceId,
    token: &SecretText,
) -> Result<bool, StoreError> {
    let deleted = conn.execute(
        "DELETE FROM device_push WHERE device_id = ?1 AND token = ?2",
        params![device_id.to_string(), token.expose_secret()],
    )?;
    Ok(deleted > 0)
}

/// Deletes the token of `device_id`, inside the writer's transaction.
pub(crate) fn remove(conn: &Connection, device_id: DeviceId) -> Result<(), StoreError> {
    conn.execute("DELETE FROM device_push WHERE device_id = ?1", [device_id.to_string()])?;
    Ok(())
}

#[cfg(test)]
mod tests;

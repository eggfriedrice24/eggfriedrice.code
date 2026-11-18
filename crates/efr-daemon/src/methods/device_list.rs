//! `device.list`: every enrolled device, the revoked ones too, oldest first, with
//! whether it has push on, for `efr device list`.

use std::collections::HashSet;

use efr_protocol::{DeviceId, DeviceInfo, DeviceList, DeviceListResult};
use efr_store::devices::Device;
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    _params: DeviceList,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let (devices, push) = state
        .readers
        .with(|conn| Ok((efr_store::devices::list(conn)?, efr_store::device_push::devices(conn)?)))
        .await?;
    let devices = devices.into_iter().map(|device| info(device, &push)).collect();
    responder.item(&DeviceListResult { devices }).await?;
    Ok(())
}

/// The wire form of a row; `push` holds the devices that have a token. The key itself,
/// the user id and the token stay in efrd.
fn info(device: Device, push: &HashSet<DeviceId>) -> DeviceInfo {
    DeviceInfo {
        push: push.contains(&device.device_id),
        device_id: device.device_id,
        name: device.name,
        key_fingerprint: device.key_fingerprint,
        node: device.node_stable_id,
        scopes: device.scopes,
        created_at: device.created_at,
        last_seen_at: device.last_seen_at,
        revoked_at: device.revoked_at,
    }
}

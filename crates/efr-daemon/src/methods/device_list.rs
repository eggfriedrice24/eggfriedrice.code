//! `device.list`: every enrolled device, the revoked ones too, oldest first, for
//! `efr device list`.

use efr_protocol::{DeviceInfo, DeviceList, DeviceListResult};
use efr_store::devices::Device;
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    _params: DeviceList,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let devices = state.readers.with(efr_store::devices::list).await?;
    let devices = devices.into_iter().map(info).collect();
    responder.item(&DeviceListResult { devices }).await?;
    Ok(())
}

/// The wire form of a row. The key itself and the user id stay in efrd.
fn info(device: Device) -> DeviceInfo {
    DeviceInfo {
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

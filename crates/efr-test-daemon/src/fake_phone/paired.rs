//! A phone that a terminal paired and that proved its key: the start of each test of
//! what a phone does after the pairing.

use std::net::SocketAddrV4;

use efr_protocol::{
    DeviceEnroll, DeviceEnrollResult, DeviceId, DevicePairConfirm, DevicePairConfirmResult,
    DevicePairStart, DevicePairStartItem, Method,
};
use futures::StreamExt as _;

use super::{FakePhone, PhoneKey};
use crate::{TTY, TestDaemon, TestDaemonError};

/// The name that a paired phone enrolls with.
const PAIRED_PHONE_NAME: &str = "paired phone";

/// A paired phone on a connection whose device proved its key.
#[derive(Debug)]
pub struct PairedPhone {
    /// The connection.
    pub phone: FakePhone,
    /// The software key of the device.
    pub key: PhoneKey,
    /// The device.
    pub device_id: DeviceId,
}

impl PairedPhone {
    /// Pairs a new phone with `daemon`, whose remote listener is on: a terminal opens the
    /// pairing and says yes, and the phone enrolls and proves its key.
    ///
    /// # Errors
    ///
    /// The errors of the client and the phone, and
    /// [`TestDaemonError::PhoneUnexpected`] for a pairing that does not go as the app
    /// sees it.
    pub async fn pair(daemon: &TestDaemon) -> Result<PairedPhone, TestDaemonError> {
        let terminal = daemon.client_for_tty(TTY).await?;
        let mut items = terminal
            .stream::<DevicePairStartItem>(Method::DevicePairStart(DevicePairStart::default()))
            .await?;
        let Some(Ok(DevicePairStartItem::Started { pairing_id, qr, .. })) = items.next().await
        else {
            return Err(TestDaemonError::PhoneUnexpected { what: "no started pairing" });
        };
        let mut phone = FakePhone::connect(SocketAddrV4::new(qr.host, qr.port)).await?;
        let hello = phone.hello(None).await?;
        let key = PhoneKey::generate()?;
        let enroll = DeviceEnroll {
            token: qr.token.clone(),
            name: PAIRED_PHONE_NAME.to_owned(),
            public_key: key.public_key(),
        };
        let enroll = phone.send(Method::DeviceEnroll(enroll)).await?;
        let Some(Ok(DevicePairStartItem::Pending { .. })) = items.next().await else {
            return Err(TestDaemonError::PhoneUnexpected { what: "no pending pairing" });
        };
        let confirm = DevicePairConfirm { pairing_id, accept: true };
        let confirmed: DevicePairConfirmResult =
            terminal.call(Method::DevicePairConfirm(confirm)).await?;
        let Some(device_id) = confirmed.device_id else {
            return Err(TestDaemonError::PhoneUnexpected { what: "a yes with no device" });
        };
        let _: DeviceEnrollResult = phone.result(enroll).await?;
        phone.prove(&key, &hello, device_id).await?;
        Ok(PairedPhone { phone, key, device_id })
    }

    /// A new connection of the same device to `daemon`, proved, as the app opens one
    /// after a lost link.
    ///
    /// # Errors
    ///
    /// The errors of the phone.
    pub async fn reconnect(&self, daemon: &TestDaemon) -> Result<FakePhone, TestDaemonError> {
        let mut phone = FakePhone::connect(daemon.remote_addr().await?).await?;
        let hello = phone.hello(None).await?;
        phone.prove(&self.key, &hello, self.device_id).await?;
        Ok(phone)
    }
}

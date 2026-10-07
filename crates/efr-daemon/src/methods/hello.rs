//! `hello`: the first request on every connection.
//!
//! The transport has already checked the protocol version. The daemon answers with its
//! identity, its directories, what the connection may do and a fresh challenge, and
//! keeps the hello's tty and client for the life of the connection.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use efr_protocol::{Capabilities, DaemonPaths, Hello, HelloResult, PROTOCOL_VERSION, ScopeName};
use efr_transport::ConnectionContext;

use crate::DaemonError;
use crate::connections::HelloInfo;
use crate::methods::granted;
use crate::sandbox::peers::PeerSide;
use crate::state::{SCRATCH_DIR, State};

/// The challenge's size: 256 bits, more than any signature scheme needs.
const CHALLENGE_BYTES: usize = 32;

pub(crate) fn handle(
    state: &State,
    context: &ConnectionContext,
    peer: PeerSide,
    hello: &Hello,
) -> Result<HelloResult, DaemonError> {
    let mut challenge = [0; CHALLENGE_BYTES];
    state.rng.fill_bytes(&mut challenge);
    let capabilities = Capabilities {
        admin: Some(granted(context.surface(), peer).contains(&ScopeName::Admin)),
        screen_snapshots: Some(true),
        ..Capabilities::default()
    };
    let result = HelloResult {
        daemon_id: state.daemon_id,
        protocol: PROTOCOL_VERSION,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        capabilities,
        paths: DaemonPaths {
            scratch_root: state.dirs.data().join(SCRATCH_DIR),
            data_dir: state.dirs.data().to_path_buf(),
        },
        challenge: URL_SAFE_NO_PAD.encode(challenge),
    };
    // NOTE: recorded last, after anything that could fail, so a hello that is refused
    // leaves nothing behind for `closed` to clean up.
    state.connections.opened(
        context.conn_id(),
        HelloInfo {
            surface: context.surface(),
            tty: hello.tty.clone(),
            client: hello.client.clone(),
        },
    );
    Ok(result)
}

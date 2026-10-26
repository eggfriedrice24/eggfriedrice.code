//! `hello`: the first request on every connection.
//!
//! The transport has already checked the protocol version. The daemon answers with its
//! identity, its directories, what the connection may do and a fresh challenge, and
//! keeps the hello's tty and client for the life of the connection. A connection of the
//! tailnet listener also gets the Tailscale facts that its device signs for
//! `device.prove`, and the connection keeps the challenge for that proof.
//!
//! A remote device has no terminal and no use for local paths. So the daemon keeps no
//! tty for a connection of the tailnet listener, whatever its hello names, and sends it
//! empty paths: before the proof, any node of the user can send `hello`, and the paths
//! show the user's name and the layout of the home directory.

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
    let challenge = URL_SAFE_NO_PAD.encode(challenge);
    let prove = crate::remote::hello(context, hello, &challenge)?;
    let remote = context.remote().is_some();
    let capabilities = Capabilities {
        admin: Some(granted(context.surface(), peer).contains(&ScopeName::Admin)),
        screen_snapshots: Some(true),
        search: Some(true),
        device_prove: prove.is_some().then_some(true),
        // NOTE: the streaming features of `conversation.subscribe` and the approval
        // events, on every connection; the phone app hides what an older efrd lacks.
        drafts_coalesced: Some(true),
        snapshot_live: Some(true),
        approval_local_only: Some(true),
        // NOTE: the lists of the phone follow the watch instead of asking again.
        conversations_watch: Some(true),
        ..Capabilities::default()
    };
    let result = HelloResult {
        daemon_id: state.daemon_id,
        protocol: PROTOCOL_VERSION,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        capabilities,
        paths: if remote {
            DaemonPaths::default()
        } else {
            DaemonPaths {
                scratch_root: state.dirs.data().join(SCRATCH_DIR),
                data_dir: state.dirs.data().to_path_buf(),
            }
        },
        challenge,
        prove,
    };
    // NOTE: recorded last, after anything that could fail, so a hello that is refused
    // leaves nothing behind for `closed` to clean up.
    state.connections.opened(
        context.conn_id(),
        HelloInfo {
            surface: context.surface(),
            // NOTE: a tty from a remote device would make it count as a client on
            // that terminal: its follows would hold back the terminal's notices, and
            // its prompts would route into the terminal's slot.
            tty: if remote { None } else { hello.tty.clone() },
            client: hello.client.clone(),
        },
    );
    Ok(result)
}

//! The host's side of the job port forwards on this machine: the bind of
//! `127.0.0.1:<port>` and the host's tcp tables.

use std::io;
use std::net::{Ipv4Addr, TcpListener};

use efr_sandbox::{TcpRow, parse_tcp_table};

use crate::sandbox::seams::HostPorts;

/// The host's tcp table of IPv4.
const TCP: &str = "/proc/net/tcp";

/// The host's tcp table of IPv6; missing when the kernel has IPv6 off.
const TCP6: &str = "/proc/net/tcp6";

/// The real ports and tables of the machine.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SystemHostPorts;

impl HostPorts for SystemHostPorts {
    fn bind(&self, port: u16) -> io::Result<TcpListener> {
        // NOTE: std sets `SO_REUSEADDR` and never `SO_REUSEPORT`, so no other process can
        // share the port (DL8); a listener of another program on the port refuses it.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        listener.set_nonblocking(true)?;
        Ok(listener)
    }

    fn rows(&self) -> io::Result<(Vec<TcpRow>, Vec<TcpRow>)> {
        let v4 = std::fs::read_to_string(TCP)?;
        let v6 = match std::fs::read_to_string(TCP6) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        Ok((parse_tcp_table(&v4, false), parse_tcp_table(&v6, true)))
    }

    fn own_uid(&self) -> u32 {
        rustix::process::geteuid().as_raw()
    }
}

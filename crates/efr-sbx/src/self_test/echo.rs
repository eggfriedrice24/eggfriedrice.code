//! `self-test --echo ADDR`: a small TCP server in a job for the forwarder's tests. The
//! tests run no `python3`, and zsh's `ztcp` has no IPv6, so the launcher serves.
//!
//! It binds the UDP socket of `--echo-udp` first, when one is named, then listens on
//! `ADDR` (port 0 takes a free port), prints `udp <address>` and `echo <address>`, and
//! sends back the bytes of each connection, each on a thread of its own. At the end of
//! a connection's bytes it ends its own side too. It runs until it is killed, for
//! `--echo-ms` milliseconds, or, with `--echo-once`, until its first connection ends.

use std::io::{self, Write};
use std::net::{Shutdown, TcpListener, TcpStream, UdpSocket};
use std::process::ExitCode;
use std::time::Duration;

use super::SelfTestArgs;
use crate::os;

/// Runs the server.
pub(super) fn main(args: &SelfTestArgs) -> ExitCode {
    match serve(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr(), "efr-sbx self-test --echo: {error}");
            ExitCode::FAILURE
        }
    }
}

fn serve(args: &SelfTestArgs) -> io::Result<()> {
    let Some(address) = args.echo else { return Err(io::ErrorKind::InvalidInput.into()) };
    let udp = args.echo_udp.map(UdpSocket::bind).transpose()?;
    let listener = TcpListener::bind(address)?;
    {
        let mut out = io::stdout().lock();
        if let Some(udp) = &udp {
            writeln!(out, "udp {}", udp.local_addr()?)?;
        }
        writeln!(out, "echo {}", listener.local_addr()?)?;
        out.flush()?;
    }
    if let Some(ms) = args.echo_ms {
        std::thread::spawn(move || {
            os::sleep(Duration::from_millis(ms));
            std::process::exit(0);
        });
    }
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if args.echo_once {
            echo(stream);
            return Ok(());
        }
        std::thread::spawn(move || echo(stream));
    }
    drop(udp);
    Ok(())
}

/// Sends back every byte of `stream`; at its end, ends the way back too.
fn echo(mut stream: TcpStream) {
    let Ok(mut back) = stream.try_clone() else { return };
    let _ = io::copy(&mut stream, &mut back);
    let _ = back.shutdown(Shutdown::Write);
}

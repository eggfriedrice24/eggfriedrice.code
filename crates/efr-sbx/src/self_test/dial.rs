//! `self-test --dial ADDR`: a small TCP client in a call for the reach helper's tests.
//! The tests run no `curl` or `python3`, and zsh's `ztcp` has no half-close.
//!
//! It connects to `ADDR`, sends `--dial-bytes` bytes (`n % 251` for the byte `n`) on a
//! thread of its own and then ends the way there, reads the answer to its end, and
//! prints one `dial` line: it passes when the answer is the bytes it sent. The detail
//! names the error of each step that failed, such as `ConnectionReset`, so a test can
//! tell a reset from an empty answer. The exit status is 0 when the line passed.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::process::ExitCode;
use std::time::Duration;

use super::{CheckLine, SelfTestArgs};

/// How long the connect may take.
const CONNECT_WAIT: Duration = Duration::from_secs(5);

/// Runs the client.
pub(super) fn main(args: &SelfTestArgs) -> ExitCode {
    let (ok, detail) = match args.dial {
        Some(address) => dial(address, args.dial_bytes),
        None => (false, "no address".to_owned()),
    };
    let line = CheckLine { name: "dial".to_owned(), ok, detail };
    let text = serde_json::to_string(&line).unwrap_or_default();
    let _ = writeln!(io::stdout(), "{text}");
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// The bytes that the client sends.
fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|n| u8::try_from(n % 251).unwrap_or_default()).collect()
}

fn dial(address: std::net::SocketAddr, len: usize) -> (bool, String) {
    let stream = match TcpStream::connect_timeout(&address, CONNECT_WAIT) {
        Ok(stream) => stream,
        Err(error) => return (false, format!("connect: {error:?}")),
    };
    let sent = pattern(len);
    let writer = stream.try_clone().map(|mut writer| {
        let bytes = sent.clone();
        std::thread::spawn(move || {
            let done = writer.write_all(&bytes);
            let _ = writer.shutdown(Shutdown::Write);
            done
        })
    });
    let mut back = Vec::new();
    let mut reader = stream;
    let read = reader.read_to_end(&mut back);
    let wrote = match writer {
        Ok(writer) => writer.join().unwrap_or_else(|_| Err(io::ErrorKind::Other.into())),
        Err(error) => Err(error),
    };
    let mut detail = format!("sent {}, got {} back", sent.len(), back.len());
    if let Err(error) = &wrote {
        detail.push_str(&format!("; write: {error:?}"));
    }
    if let Err(error) = &read {
        detail.push_str(&format!("; read: {error:?}"));
    }
    (wrote.is_ok() && read.is_ok() && back == sent, detail)
}

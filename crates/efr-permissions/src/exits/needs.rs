//! The model's `needs`, normalized into exits.

use std::path::{Path, PathBuf};

use efr_protocol::{BusKind, ExitKind, ExitSource, Grant, Needs};

use super::envelope::{Envelope, resolve};
use super::{ExitNeed, write_need};
use crate::path_class::normalize;
use crate::{AutoSupport, CallFacts, Egress};

/// The exits that `needs` asks for, each list cut at its limit. A write that the
/// envelope already allows is dropped; a read of a secret is a floor.
pub(super) fn exits(
    needs: &Needs,
    envelope: &Envelope<'_>,
    dir: Option<&Path>,
    facts: Option<&CallFacts>,
    made: &[(PathBuf, bool)],
    support: &AutoSupport,
) -> Vec<ExitNeed> {
    let source = ExitSource::Needs;
    let locations = envelope.locations();
    let mut found = Vec::new();
    for word in needs.write.iter().take(Needs::MAX_WRITE) {
        let Some(path) = resolve(word, dir, locations) else { continue };
        let exit = envelope.write_exit(&path, facts, made);
        found.extend(write_need(exit, &path, &format!("needs.write {}", path.display()), source));
    }
    for word in needs.hosts.iter().take(Needs::MAX_HOSTS) {
        let Some((host, port)) = host_and_port(word) else { continue };
        let mut need = ExitNeed::new(ExitKind::Host, format!("needs.hosts {host}"), source);
        need.grants = match support.egress {
            Egress::None => vec![Grant::OpenNetwork],
            Egress::Proxy => vec![Grant::Host { host, port }],
        };
        found.push(need);
    }
    for word in needs.sockets.iter().take(Needs::MAX_SOCKETS) {
        let Some(path) = resolve(word, dir, locations) else { continue };
        found.push(socket(path, source));
    }
    if let Some(bus) = needs.bus {
        let mut need = ExitNeed::new(ExitKind::Bus, "needs.bus", source);
        need.grants = vec![Grant::Bus { bus }];
        found.push(need);
    }
    if let Some(device) = &needs.device
        && let Some(path) = normalize(Path::new(device)).filter(|path| path.starts_with("/dev"))
    {
        let mut need = ExitNeed::new(ExitKind::Device, format!("needs.device {device}"), source);
        need.grants = vec![Grant::Device { path: path.clone() }];
        need.target = Some(path);
        found.push(need);
    }
    for word in needs.unmask.iter().take(Needs::MAX_UNMASK) {
        let Some(path) = resolve(word, dir, locations) else { continue };
        let part = format!("needs.unmask {}", path.display());
        let need = if envelope.is_secret(&path) {
            let mut need = ExitNeed::new(ExitKind::Secret, part, source);
            need.target = Some(path);
            need
        } else {
            // NOTE: a path that no built-in mask hides may still be one of the user's
            // mask globs, which only the daemon knows, so it asks all the same.
            envelope.masked_read(&path, source).unwrap_or_else(|| {
                let mut need = ExitNeed::new(ExitKind::MaskedRead, part, source);
                need.grants = vec![Grant::Unmask { path: path.clone() }];
                need.target = Some(path);
                need
            })
        };
        found.push(need);
    }
    if needs.outside {
        found.push(ExitNeed::new(ExitKind::Outside, "needs.outside", source));
    }
    found
}

/// The exit of a socket that the model names: a container engine's socket gives root,
/// a bus socket is the bus, a display socket the desktop.
fn socket(path: PathBuf, source: ExitSource) -> ExitNeed {
    let part = format!("needs.sockets {}", path.display());
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let text = path.to_string_lossy();
    let kind = if matches!(name, "docker.sock" | "podman.sock" | "containerd.sock") {
        ExitKind::Privilege
    } else if path == Path::new("/run/dbus/system_bus_socket")
        || path == Path::new("/var/run/dbus/system_bus_socket")
    {
        let mut need = ExitNeed::new(ExitKind::Bus, part, source);
        need.grants = vec![Grant::Bus { bus: BusKind::System }];
        need.target = Some(path);
        return need;
    } else if name == "bus" && text.starts_with("/run/user/") {
        let mut need = ExitNeed::new(ExitKind::Bus, part, source);
        need.grants = vec![Grant::Bus { bus: BusKind::Session }];
        need.target = Some(path);
        return need;
    } else if text.starts_with("/tmp/.X11-unix/")
        || name.starts_with("wayland-")
        || name.starts_with("sway-ipc")
        || text.contains("/hypr/")
        || name.starts_with("niri")
    {
        ExitKind::DesktopIpc
    } else {
        ExitKind::Socket
    };
    let mut need = ExitNeed::new(kind, part, source);
    if kind == ExitKind::Socket {
        need.grants = vec![Grant::Socket { path: path.clone() }];
    }
    need.target = Some(path);
    need
}

/// A host the model names, in lower case, without a scheme, user info or path, and
/// its port: 443 unless given.
// NOTE: names outside ASCII stay as written; the proxy of phase 2 converts them with
// IDNA, which this crate has no dependency for.
pub(super) fn host_and_port(word: &str) -> Option<(String, u16)> {
    let word = word.trim();
    let rest = word.split_once("://").map_or(word, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']')?;
        (host, after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let port = match port {
        Some(port) => port.parse().ok()?,
        None => 443,
    };
    let host = host.trim_end_matches('.').to_lowercase();
    let valid =
        !host.is_empty() && host.chars().all(|c| !c.is_whitespace() && !c.is_control() && c != '@');
    valid.then_some((host, port))
}

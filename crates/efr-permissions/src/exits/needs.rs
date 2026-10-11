//! The model's `needs`, normalized into exits.

use std::path::{Path, PathBuf};

use efr_protocol::{BusKind, DesktopKind, ExitKind, ExitSource, Grant, Needs};

use super::envelope::{Envelope, resolve};
use super::programs::is_host_device;
use super::{Approver, ExitNeed, WriteFacts, write_need};
use crate::path_class::normalize;
use crate::{AutoSupport, CallFacts, Egress};

/// The exits that `needs` asks for, each list cut at its limit. A write that the
/// envelope already allows is dropped, and so is a device that every contained call
/// has; a read of a secret is a floor.
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
    let cx = WriteFacts { facts, support, linked: &[], installs: &[] };
    for word in needs.write.iter().take(Needs::MAX_WRITE) {
        let Some(path) = resolve(word, dir, locations) else { continue };
        let exit = envelope.write_exit(&path, facts, made);
        found.extend(write_need(
            envelope,
            &cx,
            exit,
            &path,
            &format!("needs.write {}", path.display()),
            source,
        ));
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
        found.push(socket(envelope, path, source));
    }
    if let Some(bus) = needs.bus {
        let mut need = ExitNeed::new(ExitKind::Bus, "needs.bus", source);
        need.grants = vec![Grant::Bus { bus }];
        found.push(need);
    }
    if let Some(device) = &needs.device
        && let Some(path) = normalize(Path::new(device)).filter(|path| is_host_device(path))
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
/// a bus socket is the bus, a display socket the desktop: its grant binds the one
/// control socket that efrd finds for the prompt's terminal, never the named path, and
/// only the user approves it. A socket inside an engine
/// secret, such as `~/.gnupg/S.gpg-agent`, is a `secret` floor, and one of efr's own
/// roots, such as efrd's socket, a `config` floor. Only the user approves a socket
/// under a mask or one of a credential agent ([`is_credential_agent`]), because a key
/// there signs and logs in as the user.
fn socket(envelope: &Envelope<'_>, path: PathBuf, source: ExitSource) -> ExitNeed {
    let part = format!("needs.sockets {}", path.display());
    let locations = envelope.locations();
    let floor = if envelope.is_secret(&path) {
        Some(ExitKind::Secret)
    } else if locations.is_efr_own(&path) || locations.is_write_sealed(&path) {
        Some(ExitKind::Config)
    } else {
        None
    };
    if let Some(kind) = floor {
        let mut need = ExitNeed::new(kind, part, source);
        need.target = Some(path);
        return need;
    }
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
    } else if let Some(desktop) = named_desktop(&path, name) {
        let mut need = ExitNeed::new(ExitKind::DesktopIpc, part, source);
        need.grants = vec![Grant::Desktop { desktop }];
        need.target = Some(path);
        return need;
    } else {
        ExitKind::Socket
    };
    let mut need = ExitNeed::new(kind, part, source);
    if kind == ExitKind::Socket {
        need.grants = vec![Grant::Socket { path: path.clone() }];
        if envelope.is_masked(&path) || is_credential_agent(&path, locations) {
            need.approver = Approver::User;
        }
    }
    need.target = Some(path);
    need
}

/// The desktop of a socket path that the model names: an X11 socket in
/// `/tmp/.X11-unix`, a Wayland socket (`wayland-*`), sway's (`sway-ipc*`), i3's
/// (`i3/ipc-socket*`), one of Hyprland's (below a `hypr` dir) or niri's (`niri*`).
fn named_desktop(path: &Path, name: &str) -> Option<DesktopKind> {
    let text = path.to_string_lossy();
    Some(if text.starts_with("/tmp/.X11-unix/") {
        DesktopKind::X11
    } else if name.starts_with("wayland-") {
        DesktopKind::Wayland
    } else if name.starts_with("sway-ipc") {
        DesktopKind::Sway
    } else if name.starts_with("ipc-socket") && text.contains("/i3/") {
        DesktopKind::I3
    } else if text.contains("/hypr/") {
        DesktopKind::Hyprland
    } else if name.starts_with("niri") {
        DesktopKind::Niri
    } else {
        return None;
    })
}

/// The words in a socket's path that mark a credential agent: gpg-agent, ssh-agent,
/// the GNOME keyring and gcr agents, and the agents of password managers.
const AGENT_WORDS: &[&str] = &["agent", "ssh", "gnupg", "gpg", "keyring", "gcr", "keyboxd"];

/// True when `path` is the socket of a credential agent: one that the daemon knows from
/// the environment ([`Locations::with_credential_socket`](crate::Locations::with_credential_socket)),
/// or one whose name or dir holds a word of [`AGENT_WORDS`], such as
/// `/run/user/1000/gnupg/S.gpg-agent`, `/tmp/ssh-XXXX/agent.123`,
/// `/run/user/1000/ssh-agent.socket` or `~/.1password/agent.sock`. A word counts in any
/// case, inside a longer name, so a socket of another program can ask too; that only
/// adds a question.
pub(super) fn is_credential_agent(path: &Path, locations: &crate::Locations) -> bool {
    if locations.credential_sockets().iter().any(|known| known == path) {
        return true;
    }
    path.components().any(|component| match component {
        std::path::Component::Normal(name) => {
            let name = name.to_string_lossy().to_lowercase();
            AGENT_WORDS.iter().any(|word| name.contains(word))
        }
        _ => false,
    })
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

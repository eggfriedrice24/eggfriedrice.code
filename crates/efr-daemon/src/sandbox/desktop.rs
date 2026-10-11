//! The desktop sockets of a call's `desktop` grants (FA1-FA4 of fix batch A).
//!
//! efrd is a user service, so its own environment goes stale when the compositor
//! restarts (Hyprland's signature holds a time), and is empty under linger. The
//! variables of the prompt's terminal are the source: the zsh plugin's `efr` reads the
//! six names of [`DesktopEnv`] and sends them with the shell context, and efrd keeps
//! the last ones of each conversation. Only when a prompt sent none does efrd use its
//! own start environment. efrd never scans `$XDG_RUNTIME_DIR` for a socket.
//!
//! For each grant efrd computes the one control socket from the variable
//! ([`efr_sandbox::desktop_socket_path`]) and checks it without following a link: a
//! socket of the user (an X server's socket may also be root's), and every dir between
//! the user's runtime dir, or the X11 dir, and the socket a real dir. An X server that
//! needs a cookie is refused: the cookie file is a mask, and efr never unmasks it.

use std::collections::BTreeMap;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::Path;

use efr_protocol::{DesktopEnv, DesktopKind};
use efr_sandbox::{DesktopSocket, SandboxError, X11_SOCKET_DIR, desktop_socket_path};

/// What `lstat` says about one path, without following a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Node {
    pub(crate) kind: NodeKind,
    /// The owner.
    pub(crate) uid: u32,
}

/// The kind of a [`Node`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NodeKind {
    Socket,
    Dir,
    /// A symbolic link, a file or anything else.
    Other,
}

/// What the checks of a desktop socket compare with.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Facts<'a> {
    /// `$XDG_RUNTIME_DIR` of the user.
    pub(crate) user_runtime: &'a Path,
    /// The user's id.
    pub(crate) uid: u32,
    /// True when an X server needs a cookie: the host has `$XAUTHORITY` or
    /// `~/.Xauthority`.
    pub(crate) needs_cookie: bool,
}

/// The real `lstat`.
pub(crate) fn lstat(path: &Path) -> Option<Node> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let kind = meta.file_type();
    let kind = if kind.is_socket() {
        NodeKind::Socket
    } else if kind.is_dir() {
        NodeKind::Dir
    } else {
        NodeKind::Other
    };
    Some(Node { kind, uid: meta.uid() })
}

/// The six desktop variables of `env`, an environment such as efrd's own at start.
pub(crate) fn from_env(env: &BTreeMap<String, String>) -> DesktopEnv {
    let get = |name: &str| env.get(name).filter(|value| !value.is_empty()).cloned();
    DesktopEnv {
        wayland_display: get("WAYLAND_DISPLAY"),
        display: get("DISPLAY"),
        hyprland_instance_signature: get("HYPRLAND_INSTANCE_SIGNATURE"),
        swaysock: get("SWAYSOCK"),
        i3sock: get("I3SOCK"),
        niri_socket: get("NIRI_SOCKET"),
    }
}

/// The variables that a call uses: the ones of the prompt's terminal, `client`, when
/// it sent any; else efrd's own start environment, `daemon`.
pub(crate) fn chosen<'a>(client: Option<&'a DesktopEnv>, daemon: &'a DesktopEnv) -> &'a DesktopEnv {
    client.filter(|env| !env.is_empty()).unwrap_or(daemon)
}

/// The value of the variable of `desktop` in `env`.
fn value(env: &DesktopEnv, desktop: DesktopKind) -> Option<&str> {
    match desktop {
        DesktopKind::Wayland => env.wayland_display.as_deref(),
        DesktopKind::X11 => env.display.as_deref(),
        DesktopKind::Hyprland => env.hyprland_instance_signature.as_deref(),
        DesktopKind::Sway => env.swaysock.as_deref(),
        DesktopKind::I3 => env.i3sock.as_deref(),
        DesktopKind::Niri => env.niri_socket.as_deref(),
        _ => None,
    }
}

/// The control socket of `desktop` that `env` names, checked through `lstat`, or
/// [`SandboxError::NoDesktop`].
pub(crate) fn find(
    desktop: DesktopKind,
    env: &DesktopEnv,
    facts: &Facts<'_>,
    lstat: &dyn Fn(&Path) -> Option<Node>,
) -> Result<DesktopSocket, SandboxError> {
    let missing = SandboxError::NoDesktop { desktop, needs_cookie: false };
    let Some(value) = value(env, desktop) else { return Err(missing) };
    let Some(path) = desktop_socket_path(desktop, value, facts.user_runtime) else {
        return Err(missing);
    };
    let x11 = desktop == DesktopKind::X11;
    if x11 && facts.needs_cookie {
        return Err(SandboxError::NoDesktop { desktop, needs_cookie: true });
    }
    let root = if x11 { Path::new(X11_SOCKET_DIR) } else { facts.user_runtime };
    // NOTE: the X11 dir is root's, with the sticky bit; the dirs below the user's
    // runtime dir are the user's.
    let mut dirs: Vec<&Path> = path.ancestors().skip(1).take_while(|dir| *dir != root).collect();
    dirs.push(root);
    for dir in dirs {
        let real_dir = match lstat(dir) {
            Some(Node { kind: NodeKind::Dir, uid }) => dir == root || uid == facts.uid,
            _ => false,
        };
        if !real_dir {
            return Err(missing);
        }
    }
    let owner_ok = |uid: u32| uid == facts.uid || (x11 && uid == 0);
    match lstat(&path) {
        Some(Node { kind: NodeKind::Socket, uid }) if owner_ok(uid) => {
            Ok(DesktopSocket { desktop, path, value: value.to_owned() })
        }
        _ => Err(missing),
    }
}

#[cfg(test)]
mod tests;

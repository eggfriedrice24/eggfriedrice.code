//! Where the control socket of a desktop lies, from the one variable that names it.
//!
//! efrd computes the path of a `desktop` grant with [`desktop_socket_path`] from the
//! variables of the prompt's terminal, then checks the file. The plan computes it
//! again from the value in the spec and refuses a socket that is not at that path, so
//! the variable that the call gets always names the one socket that the call can reach.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use efr_protocol::DesktopKind;

use crate::paths::is_normal;

/// The directory of the local X11 sockets, `X0` for display `:0`.
pub const X11_SOCKET_DIR: &str = "/tmp/.X11-unix";

/// The name of Hyprland's control socket in its instance dir. The dir also holds
/// `.socket2.sock`, the event stream with every window title, and `hyprland.log`; a
/// grant never reaches them.
pub const HYPRLAND_SOCKET: &str = ".socket.sock";

/// The longest value of a desktop variable that efr reads.
const MAX_VALUE: usize = 1024;

/// The longest display or screen number of a local X11 display.
const MAX_DISPLAY_DIGITS: usize = 6;

/// The control socket of `desktop` whose variable has `value`, with `user_runtime` the
/// user's `$XDG_RUNTIME_DIR`; `None` when the value does not name one in a place that
/// a grant may reach:
///
/// - Hyprland: `<user_runtime>/hypr/<value>/.socket.sock`, where the instance
///   signature has only ASCII letters, digits and `_`.
/// - Wayland: `<user_runtime>/<value>` for a socket name, or the value itself when it
///   is an absolute path.
/// - sway, i3, niri: the value, an absolute path.
/// - X11: `/tmp/.X11-unix/X<n>` for a local display `:<n>` or `:<n>.<screen>`, never
///   a display on another host or a TCP port.
///
/// The path must also have the place and the name of that desktop's socket
/// ([`is_desktop_socket`]), so a value cannot name another socket of the user, such as
/// the session bus, a credential agent or Hyprland's event socket.
pub fn desktop_socket_path(
    desktop: DesktopKind,
    value: &str,
    user_runtime: &Path,
) -> Option<PathBuf> {
    if value.is_empty() || value.len() > MAX_VALUE || value.chars().any(char::is_control) {
        return None;
    }
    let path = match desktop {
        DesktopKind::Hyprland => {
            is_signature(value).then(|| user_runtime.join("hypr").join(value).join(HYPRLAND_SOCKET))
        }
        DesktopKind::Wayland if !value.starts_with('/') => {
            let name = !value.contains('/') && value != "." && value != "..";
            name.then(|| user_runtime.join(value))
        }
        DesktopKind::Wayland | DesktopKind::Sway | DesktopKind::I3 | DesktopKind::Niri => {
            Some(PathBuf::from(value))
        }
        DesktopKind::X11 => {
            let display = value.strip_prefix(':')?;
            let (number, screen) = match display.split_once('.') {
                Some((number, screen)) => (number, Some(screen)),
                None => (display, None),
            };
            (is_number(number) && screen.is_none_or(is_number))
                .then(|| Path::new(X11_SOCKET_DIR).join(format!("X{number}")))
        }
        // NOTE: a desktop that this build does not know gets no socket.
        _ => None,
    }?;
    is_desktop_socket(desktop, &path, user_runtime).then_some(path)
}

/// True when `path`, in normal form, has the place and the file name of a control
/// socket of `desktop`, with `user_runtime` the user's `$XDG_RUNTIME_DIR`:
///
/// - Hyprland: `<user_runtime>/hypr/<signature>/.socket.sock`.
/// - Wayland: `<user_runtime>/wayland-*`.
/// - sway: `<user_runtime>/sway-ipc.*.sock`.
/// - i3: `<user_runtime>/i3/ipc-socket.*`.
/// - niri: `<user_runtime>/niri.*.sock`.
/// - X11: `/tmp/.X11-unix/X<n>`.
///
/// The plan checks the path again after every link, so a link with a desktop's name
/// that points to another socket binds nothing.
pub fn is_desktop_socket(desktop: DesktopKind, path: &Path, user_runtime: &Path) -> bool {
    if !is_normal(path) {
        return false;
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name().and_then(OsStr::to_str))
    else {
        return false;
    };
    let named = |prefix: &str, suffix: &str| {
        name.len() > prefix.len() + suffix.len()
            && name.starts_with(prefix)
            && name.ends_with(suffix)
    };
    match desktop {
        DesktopKind::Hyprland => {
            name == HYPRLAND_SOCKET
                && parent.file_name().and_then(OsStr::to_str).is_some_and(is_signature)
                && parent.parent() == Some(user_runtime.join("hypr").as_path())
        }
        DesktopKind::Wayland => parent == user_runtime && named("wayland-", ""),
        DesktopKind::Sway => parent == user_runtime && named("sway-ipc.", ".sock"),
        DesktopKind::I3 => parent == user_runtime.join("i3") && named("ipc-socket.", ""),
        DesktopKind::Niri => parent == user_runtime && named("niri.", ".sock"),
        DesktopKind::X11 => {
            parent == Path::new(X11_SOCKET_DIR) && name.strip_prefix('X').is_some_and(is_number)
        }
        _ => false,
    }
}

/// True for a Hyprland instance signature: ASCII letters, digits and `_`.
fn is_signature(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// True for the number of a local X11 display or screen.
fn is_number(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_DISPLAY_DIGITS
        && text.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests;

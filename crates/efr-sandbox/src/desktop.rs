//! Where the control socket of a desktop lies, from the one variable that names it.
//!
//! efrd computes the path of a `desktop` grant with [`desktop_socket_path`] from the
//! variables of the prompt's terminal, then checks the file. The plan computes it
//! again from the value in the spec and refuses a socket that is not at that path, so
//! the variable that the call gets always names the one socket that the call can reach.

use std::path::{Path, PathBuf};

use efr_protocol::DesktopKind;

use crate::paths::{is_normal, is_within};

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
///   is an absolute path below `user_runtime`.
/// - sway, i3, niri: the value, an absolute path in normal form below `user_runtime`.
/// - X11: `/tmp/.X11-unix/X<n>` for a local display `:<n>` or `:<n>.<screen>`, never
///   a display on another host or a TCP port.
pub fn desktop_socket_path(
    desktop: DesktopKind,
    value: &str,
    user_runtime: &Path,
) -> Option<PathBuf> {
    if value.is_empty() || value.len() > MAX_VALUE || value.chars().any(char::is_control) {
        return None;
    }
    let below = |path: &Path| {
        (is_normal(path) && is_within(path, user_runtime) && path != user_runtime)
            .then(|| path.to_path_buf())
    };
    match desktop {
        DesktopKind::Hyprland => {
            let signature = value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            signature.then(|| user_runtime.join("hypr").join(value).join(HYPRLAND_SOCKET))
        }
        DesktopKind::Wayland if value.starts_with('/') => below(Path::new(value)),
        DesktopKind::Wayland => {
            let name = !value.contains('/') && value != "." && value != "..";
            name.then(|| user_runtime.join(value))
        }
        DesktopKind::Sway | DesktopKind::I3 | DesktopKind::Niri => below(Path::new(value)),
        DesktopKind::X11 => {
            let display = value.strip_prefix(':')?;
            let (number, screen) = match display.split_once('.') {
                Some((number, screen)) => (number, Some(screen)),
                None => (display, None),
            };
            let digits = |text: &str| {
                !text.is_empty()
                    && text.len() <= MAX_DISPLAY_DIGITS
                    && text.bytes().all(|byte| byte.is_ascii_digit())
            };
            (digits(number) && screen.is_none_or(digits))
                .then(|| Path::new(X11_SOCKET_DIR).join(format!("X{number}")))
        }
        // NOTE: a desktop that this build does not know gets no socket.
        _ => None,
    }
}

#[cfg(test)]
mod tests;

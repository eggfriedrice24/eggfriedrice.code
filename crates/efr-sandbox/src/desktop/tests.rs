use std::path::{Path, PathBuf};

use efr_protocol::DesktopKind;
use pretty_assertions::assert_eq;

use super::desktop_socket_path;

const XRT: &str = "/run/user/1000";

fn path(desktop: DesktopKind, value: &str) -> Option<PathBuf> {
    desktop_socket_path(desktop, value, Path::new(XRT))
}

#[test]
fn each_desktop_names_its_one_control_socket() {
    let cases = [
        (
            DesktopKind::Hyprland,
            "0b4a3e5a_1760000000_1234",
            "/run/user/1000/hypr/0b4a3e5a_1760000000_1234/.socket.sock",
        ),
        (DesktopKind::Wayland, "wayland-1", "/run/user/1000/wayland-1"),
        (DesktopKind::Wayland, "/run/user/1000/wayland-0", "/run/user/1000/wayland-0"),
        (
            DesktopKind::Sway,
            "/run/user/1000/sway-ipc.1000.42.sock",
            "/run/user/1000/sway-ipc.1000.42.sock",
        ),
        (DesktopKind::I3, "/run/user/1000/i3/ipc-socket.42", "/run/user/1000/i3/ipc-socket.42"),
        (
            DesktopKind::Niri,
            "/run/user/1000/niri.wayland-1.42.sock",
            "/run/user/1000/niri.wayland-1.42.sock",
        ),
        (DesktopKind::X11, ":0", "/tmp/.X11-unix/X0"),
        (DesktopKind::X11, ":12.1", "/tmp/.X11-unix/X12"),
    ];
    for (desktop, value, want) in cases {
        assert_eq!(path(desktop, value), Some(PathBuf::from(want)), "{desktop:?} {value}");
    }
}

#[test]
fn a_value_that_leaves_the_runtime_dir_or_the_host_names_nothing() {
    let cases = [
        (DesktopKind::Hyprland, ""),
        (DesktopKind::Hyprland, "../../../home/u/.ssh"),
        (DesktopKind::Hyprland, "abc/../x"),
        (DesktopKind::Hyprland, "abc-def"),
        (DesktopKind::Hyprland, "abc\n"),
        (DesktopKind::Wayland, ".."),
        (DesktopKind::Wayland, "../wayland-0"),
        (DesktopKind::Wayland, "sub/wayland-0"),
        (DesktopKind::Wayland, "/tmp/wayland-0"),
        (DesktopKind::Wayland, "/run/user/1000"),
        (DesktopKind::Sway, "sway-ipc.sock"),
        (DesktopKind::Sway, "/run/user/1000/../1001/sway-ipc.sock"),
        (DesktopKind::Sway, "/run/user/10001/sway-ipc.sock"),
        (DesktopKind::I3, "/tmp/i3-u/ipc-socket.1"),
        (DesktopKind::Niri, "/home/u/niri.sock"),
        (DesktopKind::X11, "0"),
        (DesktopKind::X11, "host:0"),
        (DesktopKind::X11, "localhost:10.0"),
        (DesktopKind::X11, "unix:0"),
        (DesktopKind::X11, ":"),
        (DesktopKind::X11, ":0."),
        (DesktopKind::X11, ":0/x"),
        (DesktopKind::X11, ":1234567"),
    ];
    for (desktop, value) in cases {
        assert_eq!(path(desktop, value), None, "{desktop:?} {value:?}");
    }
}

/// The review of fix batch A: a value of the shell's environment, which project
/// tooling such as direnv can set, named any socket of the user in the runtime dir.
#[test]
fn a_value_that_names_another_socket_of_the_user_names_nothing() {
    let cases = [
        (DesktopKind::Sway, "/run/user/1000/hypr/x/.socket2.sock"),
        (DesktopKind::Sway, "/run/user/1000/gnupg/S.gpg-agent"),
        (DesktopKind::Sway, "/run/user/1000/bus"),
        (DesktopKind::Sway, "/run/user/1000/sway-ipc.sock"),
        (DesktopKind::Sway, "/run/user/1000/sub/sway-ipc.1000.42.sock"),
        (DesktopKind::I3, "/run/user/1000/bus"),
        (DesktopKind::I3, "/run/user/1000/ipc-socket.42"),
        (DesktopKind::I3, "/run/user/1000/hypr/i3/ipc-socket.42"),
        (DesktopKind::Niri, "/run/user/1000/hypr/x/niri.wayland-1.42.sock"),
        (DesktopKind::Niri, "/run/user/1000/pipewire-0"),
        (DesktopKind::Wayland, "bus"),
        (DesktopKind::Wayland, "pipewire-0"),
        (DesktopKind::Wayland, "wayland-"),
        (DesktopKind::Wayland, "/run/user/1000/bus"),
        (DesktopKind::Wayland, "/run/user/1000/hypr/x/wayland-1"),
        (DesktopKind::Wayland, "/run/user/1000/wayland-0/"),
    ];
    for (desktop, value) in cases {
        assert_eq!(path(desktop, value), None, "{desktop:?} {value:?}");
    }
}

#[test]
fn a_desktop_socket_has_the_place_and_the_name_of_its_desktop() {
    let xrt = Path::new(XRT);
    let fits = |desktop, path: &str| super::is_desktop_socket(desktop, Path::new(path), xrt);
    assert!(fits(DesktopKind::Hyprland, "/run/user/1000/hypr/abc_1/.socket.sock"));
    assert!(!fits(DesktopKind::Hyprland, "/run/user/1000/hypr/abc_1/.socket2.sock"));
    assert!(!fits(DesktopKind::Hyprland, "/run/user/1000/hypr/a-b/.socket.sock"));
    assert!(!fits(DesktopKind::Hyprland, "/home/u/hypr/abc_1/.socket.sock"));
    assert!(fits(DesktopKind::X11, "/tmp/.X11-unix/X0"));
    assert!(!fits(DesktopKind::X11, "/tmp/.X11-unix/X"));
    assert!(!fits(DesktopKind::X11, "/run/user/1000/X0"));
    assert!(!fits(DesktopKind::Wayland, "/run/user/1000/hypr/abc_1/wayland-1"));
}

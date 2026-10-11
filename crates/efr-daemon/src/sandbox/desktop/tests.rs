use std::collections::{BTreeMap, HashMap};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use efr_protocol::{DesktopEnv, DesktopKind};
use efr_sandbox::{DesktopSocket, SandboxError};
use pretty_assertions::assert_eq;

use super::{Facts, Node, NodeKind, chosen, find, from_env, lstat};

const UID: u32 = 1000;
const XRT: &str = "/run/user/1000";
const SIGNATURE: &str = "abc_1760000000_1";

fn hyprland(signature: &str) -> DesktopEnv {
    DesktopEnv { hyprland_instance_signature: Some(signature.to_owned()), ..DesktopEnv::default() }
}

/// A file system of `nodes`, for `lstat`.
fn fake(nodes: &[(&str, NodeKind, u32)]) -> impl Fn(&Path) -> Option<Node> {
    let nodes: HashMap<PathBuf, Node> = nodes
        .iter()
        .map(|(path, kind, uid)| (PathBuf::from(path), Node { kind: *kind, uid: *uid }))
        .collect();
    move |path: &Path| nodes.get(path).copied()
}

fn facts(needs_cookie: bool) -> Facts<'static> {
    Facts { user_runtime: Path::new(XRT), uid: UID, needs_cookie }
}

fn no_desktop(result: Result<DesktopSocket, SandboxError>) -> (DesktopKind, bool) {
    match result {
        Err(SandboxError::NoDesktop { desktop, needs_cookie }) => (desktop, needs_cookie),
        other => panic!("no desktop: {other:?}"),
    }
}

#[test]
fn desktop_env_resolves_hyprland_path() {
    let dir = tempfile::tempdir().unwrap();
    let xrt = dir.path().canonicalize().unwrap();
    let instance = xrt.join("hypr").join(SIGNATURE);
    std::fs::create_dir_all(&instance).unwrap();
    let _control = UnixListener::bind(instance.join(".socket.sock")).unwrap();
    let _events = UnixListener::bind(instance.join(".socket2.sock")).unwrap();
    std::fs::write(instance.join("hyprland.log"), "window titles").unwrap();
    let uid = rustix::process::getuid().as_raw();
    let facts = Facts { user_runtime: &xrt, uid, needs_cookie: false };

    let found = find(DesktopKind::Hyprland, &hyprland(SIGNATURE), &facts, &lstat).unwrap();
    assert_eq!(
        found,
        DesktopSocket {
            desktop: DesktopKind::Hyprland,
            path: instance.join(".socket.sock"),
            value: SIGNATURE.to_owned(),
        }
    );
    // A stale signature, as after a restart of Hyprland, names no socket.
    let stale = find(DesktopKind::Hyprland, &hyprland("abc_1750000000_1"), &facts, &lstat);
    assert_eq!(no_desktop(stale), (DesktopKind::Hyprland, false));
    // A desktop that the terminal does not name is no session.
    let wayland = find(DesktopKind::Wayland, &hyprland(SIGNATURE), &facts, &lstat);
    assert_eq!(no_desktop(wayland), (DesktopKind::Wayland, false));
}

#[test]
fn desktop_env_rejects_bad_signature_and_foreign_owner() {
    let instance = "/run/user/1000/hypr/abc_1760000000_1";
    let socket = "/run/user/1000/hypr/abc_1760000000_1/.socket.sock";
    let good = [
        (XRT, NodeKind::Dir, UID),
        ("/run/user/1000/hypr", NodeKind::Dir, UID),
        (instance, NodeKind::Dir, UID),
        (socket, NodeKind::Socket, UID),
    ];
    let env = hyprland(SIGNATURE);
    assert!(find(DesktopKind::Hyprland, &env, &facts(false), &fake(&good)).is_ok());

    for signature in ["../../../home/u/.ssh", "abc/../../x", "abc-1", ""] {
        let result = find(DesktopKind::Hyprland, &hyprland(signature), &facts(false), &fake(&good));
        assert_eq!(no_desktop(result), (DesktopKind::Hyprland, false), "{signature:?}");
    }
    let changed = |path: &'static str, kind: NodeKind, uid: u32| {
        let mut nodes = good.to_vec();
        nodes.retain(|(known, _, _)| *known != path);
        nodes.push((path, kind, uid));
        nodes
    };
    let refused = [
        ("a socket of another user", changed(socket, NodeKind::Socket, 1001)),
        ("a socket of root", changed(socket, NodeKind::Socket, 0)),
        ("a file, not a socket", changed(socket, NodeKind::Other, UID)),
        ("a link on the way", changed(instance, NodeKind::Other, UID)),
        ("a dir of another user", changed("/run/user/1000/hypr", NodeKind::Dir, 1001)),
        ("no socket", good[..3].to_vec()),
    ];
    for (what, nodes) in refused {
        let result = find(DesktopKind::Hyprland, &env, &facts(false), &fake(&nodes));
        assert_eq!(no_desktop(result), (DesktopKind::Hyprland, false), "{what}");
    }
    // A sway socket that its variable places outside the runtime dir.
    let sway =
        DesktopEnv { swaysock: Some("/tmp/sway-ipc.sock".to_owned()), ..DesktopEnv::default() };
    let outside = fake(&[("/tmp/sway-ipc.sock", NodeKind::Socket, UID)]);
    assert_eq!(
        no_desktop(find(DesktopKind::Sway, &sway, &facts(false), &outside)).0,
        DesktopKind::Sway
    );
}

#[test]
fn desktop_x11_with_xauthority_refuses() {
    let env = DesktopEnv { display: Some(":0".to_owned()), ..DesktopEnv::default() };
    // Xorg runs as root.
    let nodes = [("/tmp/.X11-unix", NodeKind::Dir, 0), ("/tmp/.X11-unix/X0", NodeKind::Socket, 0)];
    let found = find(DesktopKind::X11, &env, &facts(false), &fake(&nodes)).unwrap();
    assert_eq!(found.path, PathBuf::from("/tmp/.X11-unix/X0"));
    assert_eq!(found.value, ":0");

    let refused = find(DesktopKind::X11, &env, &facts(true), &fake(&nodes));
    assert_eq!(no_desktop(refused), (DesktopKind::X11, true));
    let foreign =
        [("/tmp/.X11-unix", NodeKind::Dir, 0), ("/tmp/.X11-unix/X0", NodeKind::Socket, 1001)];
    assert_eq!(no_desktop(find(DesktopKind::X11, &env, &facts(false), &fake(&foreign))).1, false);
    let remote = DesktopEnv { display: Some("host:0".to_owned()), ..DesktopEnv::default() };
    assert_eq!(no_desktop(find(DesktopKind::X11, &remote, &facts(false), &fake(&nodes))).1, false);
}

#[test]
fn desktop_env_falls_back_to_daemon_env() {
    let mut start = BTreeMap::new();
    for (name, value) in [
        ("HYPRLAND_INSTANCE_SIGNATURE", "old_1750000000_1"),
        ("WAYLAND_DISPLAY", "wayland-1"),
        ("DISPLAY", ""),
        ("SSH_AUTH_SOCK", "/run/user/1000/ssh"),
    ] {
        start.insert(name.to_owned(), value.to_owned());
    }
    let daemon = from_env(&start);
    assert_eq!(
        daemon,
        DesktopEnv {
            hyprland_instance_signature: Some("old_1750000000_1".to_owned()),
            wayland_display: Some("wayland-1".to_owned()),
            ..DesktopEnv::default()
        }
    );
    let client = hyprland(SIGNATURE);
    assert_eq!(chosen(Some(&client), &daemon), &client, "the terminal of the prompt first");
    assert_eq!(chosen(None, &daemon), &daemon, "a prompt that sent none");
    assert_eq!(chosen(Some(&DesktopEnv::default()), &daemon), &daemon);
}

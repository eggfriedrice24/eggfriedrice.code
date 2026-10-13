use std::collections::BTreeMap;
use std::ffi::OsString;

use efr_protocol::{DesktopKind, Grant};
use pretty_assertions::assert_eq;

use crate::env_filter::EnvFilter;
use crate::plan::MountPlan;
use crate::spec::{DesktopSocket, NetworkPlan};
use crate::testing::{spec, world};

fn env(pairs: &[(&str, &str)]) -> BTreeMap<OsString, OsString> {
    pairs.iter().map(|(name, value)| (OsString::from(name), OsString::from(value))).collect()
}

fn get<'a>(env: &'a BTreeMap<OsString, OsString>, name: &str) -> Option<&'a str> {
    env.get(&OsString::from(name)).and_then(|value| value.to_str())
}

#[test]
fn env_filter_drops_sockets_and_secret_names() {
    let spec = spec();
    let plan = MountPlan::build(&spec, &world()).unwrap();
    let filter = EnvFilter::new(&spec, &plan);
    let input = env(&[
        ("PATH", "/usr/bin"),
        ("HOME", "/home/u"),
        ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"),
        ("SSH_AUTH_SOCK", "/run/user/1000/ssh"),
        ("WAYLAND_DISPLAY", "wayland-1"),
        ("ZELLIJ_SESSION_NAME", "x"),
        ("GITHUB_TOKEN", "ghp"),
        ("openai_api_key", "sk"),
        ("AWS_PROFILE", "p"),
        ("DB_PASSWORD", "pw"),
        ("https_proxy", "http://p"),
        ("EFR_HIDDEN_SHELL", "1"),
        ("_EFR_HS_X", "1"),
        ("RUST_LOG", "debug"),
    ]);
    let (out, removed) = filter.apply(&input);
    for name in [
        "DBUS_SESSION_BUS_ADDRESS",
        "SSH_AUTH_SOCK",
        "WAYLAND_DISPLAY",
        "ZELLIJ_SESSION_NAME",
        "GITHUB_TOKEN",
        "openai_api_key",
        "AWS_PROFILE",
        "DB_PASSWORD",
        "https_proxy",
        "EFR_HIDDEN_SHELL",
        "_EFR_HS_X",
    ] {
        assert!(get(&out, name).is_none(), "{name}");
        assert!(removed.contains(&name.to_owned()), "{name}");
    }
    assert_eq!(get(&out, "PATH"), Some("/usr/bin"));
    assert_eq!(get(&out, "RUST_LOG"), Some("debug"));
    assert_eq!(get(&out, "TMPDIR"), Some("/tmp"));
    assert_eq!(get(&out, "XDG_RUNTIME_DIR"), Some("/run/user/1000"));
    assert_eq!(get(&out, "EFR_SANDBOX"), Some("1"));
    assert_eq!(get(&out, "GIT_TERMINAL_PROMPT"), Some("0"));
    assert_eq!(get(&out, "GIT_ASKPASS"), Some("/bin/false"));
    assert_eq!(get(&out, "SSH_ASKPASS_REQUIRE"), Some("never"));
    assert_eq!(get(&out, "CARGO_NET_OFFLINE"), Some("true"));
    assert_eq!(get(&out, "GOPROXY"), Some("off"));
}

#[test]
fn keep_opens_a_secret_like_name_and_deny_removes_more() {
    let mut spec = spec();
    spec.env.keep = vec!["AWS_REGION".to_owned()];
    spec.env.deny = vec!["MY_*".to_owned()];
    let plan = MountPlan::build(&spec, &world()).unwrap();
    let (out, removed) = EnvFilter::new(&spec, &plan).apply(&env(&[
        ("AWS_REGION", "eu-west-1"),
        ("MY_TOOL", "x"),
        ("SSH_AUTH_SOCK", "/s"),
    ]));
    assert_eq!(get(&out, "AWS_REGION"), Some("eu-west-1"));
    assert_eq!(removed, ["MY_TOOL", "SSH_AUTH_SOCK"]);
}

#[test]
fn desktop_grant_env_survives_socket_strip() {
    let socket = "/run/user/1000/hypr/new_1760000001_9/.socket.sock";
    let mut fs = world();
    fs.socket(socket).socket("/run/user/1000/wayland-1");
    let mut spec = spec();
    spec.runtime.desktops = vec![
        DesktopSocket {
            desktop: DesktopKind::Hyprland,
            path: socket.into(),
            value: "new_1760000001_9".to_owned(),
        },
        DesktopSocket {
            desktop: DesktopKind::Wayland,
            path: "/run/user/1000/wayland-1".into(),
            value: "wayland-1".to_owned(),
        },
    ];
    spec.grants = vec![Grant::Desktop { desktop: DesktopKind::Hyprland }];
    let plan = MountPlan::build(&spec, &fs).unwrap();
    let filter = EnvFilter::new(&spec, &plan);
    // The shell's own value is stale, as after a restart of Hyprland.
    let input = env(&[
        ("HYPRLAND_INSTANCE_SIGNATURE", "old_1750000000_1"),
        ("WAYLAND_DISPLAY", "wayland-1"),
        ("DISPLAY", ":0"),
    ]);
    let (out, removed) = filter.apply(&input);
    assert_eq!(get(&out, "HYPRLAND_INSTANCE_SIGNATURE"), Some("new_1760000001_9"));
    // Only the granted desktop's variable comes back.
    assert_eq!(get(&out, "WAYLAND_DISPLAY"), None);
    assert_eq!(get(&out, "DISPLAY"), None);
    assert_eq!(removed, ["DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE", "WAYLAND_DISPLAY"]);
}

#[test]
fn offline_hints_only_without_network_and_a_bus_grant_sets_its_address() {
    let mut spec = spec();
    spec.grants = vec![Grant::OpenNetwork, Grant::Bus { bus: efr_protocol::BusKind::System }];
    let plan = MountPlan::build(&spec, &world()).unwrap();
    let (out, _) = EnvFilter::new(&spec, &plan).apply(&env(&[("DBUS_SYSTEM_BUS_ADDRESS", "x")]));
    assert!(get(&out, "CARGO_NET_OFFLINE").is_none());
    assert_eq!(get(&out, "DBUS_SYSTEM_BUS_ADDRESS"), Some("unix:path=/run/dbus/system_bus_socket"));

    let mut spec = crate::testing::spec();
    spec.network = NetworkPlan::Open;
    let plan = MountPlan::build(&spec, &world()).unwrap();
    assert!(get(&EnvFilter::new(&spec, &plan).apply(&env(&[])).0, "GOPROXY").is_none());
}

#[test]
fn netview_offline_hints_stay() {
    let mut spec = spec();
    spec.grants = vec![Grant::NetView];
    let plan = MountPlan::build(&spec, &world()).unwrap();
    assert!(plan.net_view() && !plan.unshares_network());
    let (out, _) = EnvFilter::new(&spec, &plan).apply(&env(&[]));
    for (name, value) in crate::env_filter::OFFLINE_HINTS {
        assert_eq!(get(&out, name), Some(*value), "{name}");
    }
    // With an open network, the call is no network view, and the hints go.
    spec.network = NetworkPlan::Open;
    let plan = MountPlan::build(&spec, &world()).unwrap();
    assert!(get(&EnvFilter::new(&spec, &plan).apply(&env(&[])).0, "CARGO_NET_OFFLINE").is_none());
    // The user's switch still turns them off.
    spec.network = NetworkPlan::None;
    spec.env.offline_hints = false;
    let plan = MountPlan::build(&spec, &world()).unwrap();
    assert!(get(&EnvFilter::new(&spec, &plan).apply(&env(&[])).0, "UV_OFFLINE").is_none());
}

#[test]
fn a_build_cache_outside_the_overlays_moves_below_the_cache_dir() {
    let spec = spec();
    let plan = MountPlan::build(&spec, &world()).unwrap();
    let (out, _) = EnvFilter::new(&spec, &plan).apply(&env(&[
        ("CCACHE_DIR", "/var/cache/ccache"),
        ("SCCACHE_DIR", "/home/u/.cargo/sccache"),
    ]));
    assert_eq!(get(&out, "CCACHE_DIR"), Some("/home/u/.cache/ccache"));
    assert_eq!(get(&out, "SCCACHE_DIR"), Some("/home/u/.cargo/sccache"));
}

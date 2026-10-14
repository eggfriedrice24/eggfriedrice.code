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

/// The spec and the machine of a proxy call.
fn proxy_call() -> (crate::SandboxSpec, crate::testing::FakeFs) {
    let mut spec = spec();
    spec.network = NetworkPlan::Proxy { socket: spec.runtime.proxy_socket() };
    let mut fs = world();
    fs.socket(&spec.runtime.proxy_socket().to_string_lossy());
    (spec, fs)
}

#[test]
fn proxy_env_only_for_proxy_plan() {
    let (spec, fs) = proxy_call();
    let plan = MountPlan::build(&spec, &fs).unwrap();
    // The trusted shell's own proxy goes, and the bridge's comes.
    let (out, removed) = EnvFilter::new(&spec, &plan)
        .apply(&env(&[("https_proxy", "http://corp.example:8080"), ("NO_PROXY", "corp.example")]));
    for name in crate::PROXY_VARIABLES {
        assert_eq!(get(&out, name), Some(crate::PROXY_URL), "{name}");
    }
    assert_eq!(get(&out, "NO_PROXY"), Some("localhost,127.0.0.1,::1"));
    assert_eq!(get(&out, "no_proxy"), Some("localhost,127.0.0.1,::1"));
    assert_eq!(get(&out, "NODE_USE_ENV_PROXY"), Some("1"));
    assert_eq!(removed, ["NO_PROXY", "https_proxy"]);
    // A proxy call has a network: no offline hint.
    for (name, _) in crate::env_filter::OFFLINE_HINTS {
        assert!(get(&out, name).is_none(), "{name}");
    }
    // No other call gets a proxy variable.
    for network in [NetworkPlan::None, NetworkPlan::Open] {
        let mut spec = spec.clone();
        spec.network = network;
        let plan = MountPlan::build(&spec, &fs).unwrap();
        let (out, _) = EnvFilter::new(&spec, &plan).apply(&env(&[]));
        for name in crate::PROXY_VARIABLES.iter().chain(crate::NO_PROXY_VARIABLES) {
            assert!(get(&out, name).is_none(), "{name}");
        }
        assert!(get(&out, "NODE_USE_ENV_PROXY").is_none());
    }
}

#[test]
fn proxy_names_never_exported() {
    let (spec, fs) = proxy_call();
    let plan = MountPlan::build(&spec, &fs).unwrap();
    let mut promote_all = spec.clone();
    promote_all.env.promote = vec!["*".to_owned()];
    let filter = crate::ExportFilter::from_spec(
        &promote_all,
        &plan,
        std::path::Path::new(crate::testing::PROJECT),
    );
    let names = crate::PROXY_VARIABLES.iter().chain(crate::NO_PROXY_VARIABLES);
    for name in names.chain(&["NODE_USE_ENV_PROXY"]) {
        assert!(
            !matches!(filter.check(name, crate::PROXY_URL), crate::ExportVerdict::Promote),
            "{name}"
        );
    }
}

#[test]
fn auth_names_scrubbed() {
    let spec = spec();
    let plan = MountPlan::build(&spec, &world()).unwrap();
    let (out, removed) = EnvFilter::new(&spec, &plan).apply(&env(&[
        ("npm_config__auth", "dXNlcjpwYXNz"),
        ("NPM_CONFIG__AUTH", "dXNlcjpwYXNz"),
        ("REGISTRY_AUTH", "x"),
        ("AUTHOR", "efr"),
        ("SSH_AUTH_SOCK", "/s"),
    ]));
    assert_eq!(removed, ["NPM_CONFIG__AUTH", "REGISTRY_AUTH", "SSH_AUTH_SOCK", "npm_config__auth"]);
    assert_eq!(get(&out, "AUTHOR"), Some("efr"));
    // env_keep opens a name that ends in _AUTH, as for every secret-like name.
    let mut kept = spec.clone();
    kept.env.keep = vec!["REGISTRY_AUTH".to_owned()];
    let (out, _) = EnvFilter::new(&kept, &plan).apply(&env(&[("REGISTRY_AUTH", "x")]));
    assert_eq!(get(&out, "REGISTRY_AUTH"), Some("x"));
}

#[test]
fn userinfo_values_scrubbed_unless_kept() {
    let spec = spec();
    let plan = MountPlan::build(&spec, &world()).unwrap();
    let input = env(&[
        ("PIP_INDEX_URL", "https://user:tok3n@pypi.corp.example/simple"),
        ("PIP_EXTRA_INDEX_URL", "https://pypi.org/simple https://:tok@x.example/s"),
        ("DATABASE_URL", "postgres://app:pw@db.example:5432/app"),
        ("REPO_URL", "ssh://git@git.example/r.git"),
        ("HOMEPAGE", "https://example.com/a:b@c"),
        ("MIRROR", "https://mirror.example/"),
    ]);
    let (out, removed) = EnvFilter::new(&spec, &plan).apply(&input);
    assert_eq!(removed, ["DATABASE_URL", "PIP_EXTRA_INDEX_URL", "PIP_INDEX_URL"]);
    for name in ["REPO_URL", "HOMEPAGE", "MIRROR"] {
        assert!(get(&out, name).is_some(), "{name}");
    }
    let mut kept = spec.clone();
    kept.env.keep = vec!["DATABASE_URL".to_owned()];
    let (out, removed) = EnvFilter::new(&kept, &plan).apply(&input);
    assert_eq!(get(&out, "DATABASE_URL"), Some("postgres://app:pw@db.example:5432/app"));
    assert_eq!(removed, ["PIP_EXTRA_INDEX_URL", "PIP_INDEX_URL"]);
}

#[test]
fn git_config_names_scrubbed() {
    let mut spec = spec();
    // env_keep does not open git's config: it can carry a header with a token.
    spec.env.keep = vec!["GIT_CONFIG_*".to_owned()];
    let plan = MountPlan::build(&spec, &world()).unwrap();
    let (out, removed) = EnvFilter::new(&spec, &plan).apply(&env(&[
        ("GIT_CONFIG_PARAMETERS", "'http.extraheader'='Authorization: Bearer x'"),
        ("GIT_CONFIG_COUNT", "1"),
        ("GIT_CONFIG_KEY_0", "http.extraHeader"),
        ("GIT_CONFIG_VALUE_0", "Authorization: Bearer x"),
        ("GIT_CONFIG_KEY_12", "credential.helper"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_AUTHOR_NAME", "efr"),
    ]));
    assert_eq!(
        removed,
        [
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_KEY_0",
            "GIT_CONFIG_KEY_12",
            "GIT_CONFIG_PARAMETERS",
            "GIT_CONFIG_VALUE_0"
        ]
    );
    assert_eq!(get(&out, "GIT_CONFIG_NOSYSTEM"), Some("1"));
    assert_eq!(get(&out, "GIT_AUTHOR_NAME"), Some("efr"));
}

use std::collections::BTreeMap;
use std::ffi::OsString;

use efr_protocol::Grant;
use pretty_assertions::assert_eq;

use crate::env_filter::EnvFilter;
use crate::plan::MountPlan;
use crate::spec::NetworkPlan;
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

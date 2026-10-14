use std::path::{Path, PathBuf};

use efr_protocol::DesktopKind;
use pretty_assertions::assert_eq;

use crate::SandboxError;
use crate::spec::{
    CacheOverlay, DesktopSocket, JOB_BACKSTOP_SECS, JOB_DIR, JOB_FSIZE_BYTES, JobPlan, SandboxSpec,
    WriteRoot, WriteRootKind, layer_name,
};
use crate::testing::spec;

#[test]
fn a_spec_reads_back_from_its_file() {
    let spec = spec();
    let back = SandboxSpec::from_json(&spec.to_json().unwrap()).unwrap();
    assert_eq!(back, spec);
}

#[test]
fn a_spec_with_a_relative_or_unnormal_path_is_refused() {
    for path in ["p/app", "/home/u/p/../app", "/home/u/p/app/"] {
        let mut spec = spec();
        spec.write_roots = vec![WriteRoot { path: path.into(), kind: WriteRootKind::TurnProject }];
        assert!(
            matches!(spec.check(), Err(SandboxError::SpecPath { field: "write_roots", .. })),
            "{path}"
        );
    }
}

#[test]
fn a_spec_without_desktops_leaves_them_out_and_reads_back() {
    let text = String::from_utf8(spec().to_json().unwrap()).unwrap();
    assert!(!text.contains("desktops"), "{text}");
    let mut with = spec();
    with.runtime.desktops = vec![DesktopSocket {
        desktop: DesktopKind::Hyprland,
        path: "/run/user/1000/hypr/abc_1760000000_123/.socket.sock".into(),
        value: "abc_1760000000_123".into(),
    }];
    let back = SandboxSpec::from_json(&with.to_json().unwrap()).unwrap();
    assert_eq!(back, with);
}

#[test]
fn a_desktop_socket_with_a_relative_path_is_refused() {
    let mut spec = spec();
    spec.runtime.desktops = vec![DesktopSocket {
        desktop: DesktopKind::Wayland,
        path: "wayland-1".into(),
        value: "wayland-1".into(),
    }];
    assert!(matches!(spec.check(), Err(SandboxError::SpecPath { field: "runtime.desktops", .. })));
}

#[test]
fn a_spec_names_its_own_call_and_version() {
    let mut other = spec();
    other.runtime.call_dir = other.runtime.shell_dir.join("019a9b1c-3d00-7a10-8b20-0000000000ff");
    assert!(matches!(other.check(), Err(SandboxError::SpecIds { field: "runtime.call_dir", .. })));
    let mut old = spec();
    old.version = 0;
    assert!(matches!(old.check(), Err(SandboxError::SpecVersion { found: 0, .. })));
    let mut text = String::from_utf8(spec().to_json().unwrap()).unwrap();
    text.insert_str(1, "\"surprise\": 1,");
    assert!(matches!(SandboxSpec::from_json(text.as_bytes()), Err(SandboxError::Json { .. })));
}

#[test]
fn job_call_dir_parent_is_jobs() {
    let call_dir = spec().runtime.call_dir;
    let mut job = spec();
    job.job = Some(JobPlan::new(3600, 19));
    // A job's call dir in the shell dir itself is refused, so the hidden shell's
    // wrapper and a job never share a place.
    assert!(matches!(job.check(), Err(SandboxError::SpecIds { field: "runtime.call_dir", .. })));
    job.runtime.call_dir = job.runtime.shell_dir.join(JOB_DIR).join(call_dir.file_name().unwrap());
    job.check().unwrap();
    assert_eq!(SandboxSpec::from_json(&job.to_json().unwrap()).unwrap(), job);
    // A call's call dir below `jobs` is refused too.
    let mut call = job.clone();
    call.job = None;
    assert!(matches!(call.check(), Err(SandboxError::SpecIds { field: "runtime.call_dir", .. })));
    // Nor may a call dir lie anywhere else.
    let mut elsewhere = spec();
    elsewhere.runtime.call_dir =
        PathBuf::from("/run/user/1000/efr").join(call_dir.file_name().unwrap());
    assert!(matches!(
        elsewhere.check(),
        Err(SandboxError::SpecIds { field: "runtime.call_dir", .. })
    ));
}

#[test]
fn a_job_has_limits_and_a_spec_without_one_leaves_it_out() {
    let plan = JobPlan::new(3600, 25);
    assert_eq!(plan.nice, 19);
    assert_eq!(plan.backstop_secs, JOB_BACKSTOP_SECS);
    assert_eq!(plan.fsize_bytes, JOB_FSIZE_BYTES);
    assert_eq!(plan.deadline(), std::time::Duration::from_secs(3660));
    let text = String::from_utf8(spec().to_json().unwrap()).unwrap();
    assert!(!text.contains("\"job\""), "{text}");
    let mut job = spec();
    job.runtime.call_dir =
        job.runtime.shell_dir.join(JOB_DIR).join(job.runtime.call_dir.file_name().unwrap());
    for bad in [
        JobPlan { max_seconds: 0, ..plan },
        JobPlan { nice: 20, ..plan },
        JobPlan { fsize_bytes: 0, ..plan },
    ] {
        job.job = Some(bad);
        assert!(matches!(job.check(), Err(SandboxError::SpecJob)), "{bad:?}");
    }
}

#[test]
fn a_cache_overlay_gets_its_layers_and_pins() {
    let cargo =
        CacheOverlay::new(Path::new("/home/u/.cargo"), Path::new("/s/sbx"), Path::new("/home/u"));
    assert_eq!(cargo.upper, PathBuf::from("/s/sbx/cache/home%u%.cargo/upper"));
    assert_eq!(cargo.work, PathBuf::from("/s/sbx/cache/home%u%.cargo/work"));
    assert_eq!(cargo.pins.len(), 4);
    let rustup =
        CacheOverlay::new(Path::new("/home/u/.rustup"), Path::new("/s"), Path::new("/home/u"));
    assert_eq!(rustup.pins, [PathBuf::from("/home/u/.rustup/settings.toml")]);
    assert_eq!(layer_name(Path::new("/a%b/c")), "a%25b%c");
    assert_ne!(layer_name(Path::new("/a/b_c")), layer_name(Path::new("/a_b/c")));
}

#[test]
fn the_inside_paths_live_below_the_masked_runtime_dir() {
    let spec = spec();
    assert_eq!(spec.runtime.inside_launcher(), PathBuf::from("/run/user/1000/efr-sbx/efr-sbx"));
    assert_eq!(spec.runtime.private_tmp(), spec.runtime.sandbox_dir.join("tmp"));
    assert!(spec.runtime.quarantine(spec.call).starts_with(&spec.runtime.sandbox_dir));
}

#[test]
fn spec_check_refuses_proxy_with_open_network() {
    let mut spec = spec();
    spec.network = crate::NetworkPlan::Proxy { socket: spec.runtime.proxy_socket() };
    assert!(spec.check().is_ok());
    spec.grants.push(efr_protocol::Grant::OpenNetwork);
    assert!(matches!(spec.check(), Err(SandboxError::ProxyWithOpenNetwork)));
    // The launcher reads the spec from its file and refuses it there.
    assert!(matches!(
        SandboxSpec::from_json(&spec.to_json().unwrap()),
        Err(SandboxError::ProxyWithOpenNetwork)
    ));
    // A host grant narrows the network and stays.
    spec.grants = vec![efr_protocol::Grant::Host { host: "extra.test".to_owned(), port: 443 }];
    assert!(spec.check().is_ok());
}

#[test]
fn spec_check_refuses_foreign_proxy_socket() {
    let base = spec();
    let call_dir = base.runtime.call_dir.clone();
    assert_eq!(base.runtime.proxy_socket(), call_dir.join(crate::PROXY_SOCKET_FILE));
    let foreign = [
        call_dir.join("other.sock"),
        call_dir.parent().unwrap().join("proxy.sock"),
        PathBuf::from("/run/user/1000/bus"),
        base.runtime.user_runtime.join("efr-proxy.sock"),
    ];
    for socket in foreign {
        let mut spec = base.clone();
        spec.network = crate::NetworkPlan::Proxy { socket: socket.clone() };
        assert!(
            matches!(spec.check(), Err(SandboxError::ProxySocket { path }) if path == socket),
            "{socket:?}"
        );
    }
    // A job's socket lies in the job's own call dir.
    let mut job = base;
    let call = job.call.to_string();
    job.runtime.call_dir = job.runtime.shell_dir.join(JOB_DIR).join(call);
    job.job = Some(JobPlan::new(60, 19));
    job.network = crate::NetworkPlan::Proxy { socket: job.runtime.proxy_socket() };
    assert!(job.check().is_ok());
}

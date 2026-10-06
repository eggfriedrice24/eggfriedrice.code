use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use crate::SandboxError;
use crate::spec::{CacheOverlay, SandboxSpec, WriteRoot, WriteRootKind, layer_name};
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

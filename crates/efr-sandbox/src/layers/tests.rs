use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use crate::layers::{CacheLayer, CacheLayers, moved_mounts};

#[test]
fn only_the_outermost_mounts_inside_the_cache_move() {
    let targets = [
        "/home/u",
        "/home/u/.cargo",
        "/home/u/.cargo/bin",
        "/home/u/.cargo/bin/inner",
        "/home/u/.cargo/config.toml",
        "/home/u/.cargo/config.toml",
        "/home/u/.cargoish",
    ]
    .map(Path::new);
    let moved = moved_mounts(Path::new("/home/u/.cargo"), targets);
    assert_eq!(
        moved,
        [PathBuf::from("/home/u/.cargo/bin"), PathBuf::from("/home/u/.cargo/config.toml")]
    );
}

#[test]
fn layers_round_trip_and_refuse_a_large_plan() {
    let layers = CacheLayers {
        staging: "/run/user/1000/efr-sbx/layers".into(),
        fresh: true,
        layers: vec![CacheLayer {
            target: "/home/u/.cargo".into(),
            dir: "/run/user/1000/efr-sbx/layers/0".into(),
            upper: "/run/user/1000/efr-sbx/layers/0/upper".into(),
            work: "/run/user/1000/efr-sbx/layers/0/work".into(),
            moved: vec!["/home/u/.cargo/bin".into()],
        }],
    };
    let bytes = layers.to_json().unwrap();
    assert_eq!(CacheLayers::from_json(&bytes).unwrap(), layers);
    let large = vec![b' '; crate::layers::MAX_LAYERS_BYTES + 1];
    assert!(CacheLayers::from_json(&large).is_err());
}

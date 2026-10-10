use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};

use efr_protocol::SandboxSummary;
use pretty_assertions::assert_eq;

use crate::layers::CacheLayer;
use crate::layers::deletes::{CacheDeletes, DeleteLimits, LayerEntry, LayerView};
use crate::testing::FakeFs;

/// An upper layer in memory: each directory, relative to the layer's root, with its
/// entries.
#[derive(Default)]
struct FakeLayer {
    dirs: BTreeMap<PathBuf, Vec<(OsString, LayerEntry)>>,
}

impl FakeLayer {
    fn add(&mut self, dir: &str, name: impl Into<OsString>, entry: LayerEntry) -> &mut Self {
        let name = name.into();
        if let LayerEntry::Dir { .. } = entry {
            self.dirs.entry(Path::new(dir).join(&name)).or_default();
        }
        self.dirs.entry(PathBuf::from(dir)).or_default().push((name, entry));
        self
    }
}

impl LayerView for FakeLayer {
    fn entries(&self, dir: &Path) -> io::Result<Vec<(OsString, LayerEntry)>> {
        self.dirs.get(dir).cloned().ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
}

const DIR: LayerEntry = LayerEntry::Dir { opaque: false };
const OPAQUE: LayerEntry = LayerEntry::Dir { opaque: true };
const WHITEOUT: LayerEntry = LayerEntry::Whiteout;

fn layer(target: &str) -> CacheLayer {
    CacheLayer {
        target: target.into(),
        dir: "/run/efr-sbx/layers/0".into(),
        upper: "/run/efr-sbx/layers/0/upper".into(),
        work: "/run/efr-sbx/layers/0/work".into(),
        moved: Vec::new(),
    }
}

fn walk(
    layer: &CacheLayer,
    skip: &[PathBuf],
    upper: &FakeLayer,
    lower: &FakeFs,
    limits: DeleteLimits,
) -> CacheDeletes {
    let mut deletes = CacheDeletes::new(limits);
    deletes.walk(layer, skip, upper, lower);
    deletes
}

#[test]
fn a_whiteout_of_a_lower_file_or_dir_is_a_delete_and_an_opaque_dir_only_counts() {
    let mut lower = FakeFs::new();
    lower
        .file("/h/.cache/go-build/aa/1", "x")
        .file("/h/.cache/go-build/bb/2", "x")
        .file("/h/.cache/pip/remade/3", "x")
        .file("/h/.cache/kept", "x");
    let mut upper = FakeLayer::default();
    upper
        .add("", "go-build", DIR)
        .add("go-build", "aa", WHITEOUT)
        .add("go-build", "bb", DIR)
        .add("go-build/bb", "2", WHITEOUT)
        .add("go-build/bb", "new", LayerEntry::Other)
        .add("", "pip", DIR)
        .add("pip", "remade", OPAQUE)
        .add("", "kept", LayerEntry::Other);

    let deletes = walk(&layer("/h/.cache"), &[], &upper, &lower, DeleteLimits::default());

    assert_eq!(
        deletes.paths(),
        [PathBuf::from("/h/.cache/go-build/aa"), PathBuf::from("/h/.cache/go-build/bb/2")]
    );
    assert_eq!(deletes.more(), 1, "the opaque dir");
    let mut summary = SandboxSummary::default();
    deletes.fill(&mut summary);
    assert_eq!(summary.cache_deletes.len(), 2);
    assert_eq!(summary.cache_deletes_more, 1);
}

#[test]
fn cache_delete_skips_forged_whiteout() {
    // Contained code made a whiteout with mknod for a name that the cache lacks, and a
    // new dir that it filled with more of them.
    let mut lower = FakeFs::new();
    lower.dir("/h/.cache");
    let mut upper = FakeLayer::default();
    upper
        .add("", "planted", WHITEOUT)
        .add("", "newdir", DIR)
        .add("newdir", "deep", WHITEOUT)
        .add("", "made", OPAQUE);

    let deletes = walk(&layer("/h/.cache"), &[], &upper, &lower, DeleteLimits::default());

    assert_eq!(deletes.paths(), [] as [PathBuf; 0]);
    assert_eq!(deletes.more(), 0);
}

#[test]
fn cache_delete_never_below_pins() {
    let mut lower = FakeFs::new();
    lower
        .file("/h/.cargo/bin/x", "x")
        .file("/h/.cargo/config.toml", "x")
        .file("/h/.cargo/registry/cache/a.crate", "x")
        .file("/h/.cache/mozilla/profile", "x")
        .file("/h/.cache/moved/inner", "x");
    let mut upper = FakeLayer::default();
    upper
        .add("", "bin", DIR)
        .add("bin", "x", WHITEOUT)
        .add("", "config.toml", WHITEOUT)
        .add("", "registry", DIR)
        .add("registry", "cache", DIR)
        .add("registry/cache", "a.crate", WHITEOUT);
    let pins = [PathBuf::from("/h/.cargo/bin"), PathBuf::from("/h/.cargo/config.toml")];

    let cargo = walk(&layer("/h/.cargo"), &pins, &upper, &lower, DeleteLimits::default());

    assert_eq!(cargo.paths(), [PathBuf::from("/h/.cargo/registry/cache/a.crate")]);
    assert_eq!(cargo.more(), 0);

    // A mask inside the cache, a mount that the plan moved, and a whiteout of a dir
    // that holds a mask.
    let mut upper = FakeLayer::default();
    upper.add("", "mozilla", WHITEOUT).add("", "moved", DIR).add("moved", "inner", WHITEOUT);
    let mut cache = layer("/h/.cache");
    cache.moved = vec![PathBuf::from("/h/.cache/moved")];
    let masks = [PathBuf::from("/h/.cache/mozilla")];
    let skipped = walk(&cache, &masks, &upper, &lower, DeleteLimits::default());
    assert_eq!(skipped.paths(), [] as [PathBuf; 0]);

    let mut upper = FakeLayer::default();
    upper.add("", "mozilla", WHITEOUT);
    let holds = [PathBuf::from("/h/.cache/mozilla/profile")];
    let holding = walk(&layer("/h/.cache"), &holds, &upper, &lower, DeleteLimits::default());
    assert_eq!(holding.paths(), [] as [PathBuf; 0]);
    assert_eq!(holding.more(), 1, "a delete that would reach a mask only counts");

    let whole = walk(
        &layer("/h/.cache"),
        &[PathBuf::from("/h/.cache")],
        &upper,
        &lower,
        DeleteLimits::default(),
    );
    assert_eq!((whole.paths().len(), whole.more()), (0, 0), "a masked cache lists nothing");
}

#[test]
fn cache_delete_skips_names_that_are_not_utf8() {
    let mut lower = FakeFs::new();
    lower.file("/h/.cache/ok", "x").dir("/h/.cache");
    let mut upper = FakeLayer::default();
    upper.add("", OsString::from_vec(b"bad\xff".to_vec()), WHITEOUT).add("", "ok", WHITEOUT);

    let deletes = walk(&layer("/h/.cache"), &[], &upper, &lower, DeleteLimits::default());

    assert_eq!(deletes.paths(), [PathBuf::from("/h/.cache/ok")]);
    assert_eq!(deletes.more(), 1);
}

#[test]
fn cache_delete_list_is_bounded() {
    let mut lower = FakeFs::new();
    let mut upper = FakeLayer::default();
    for n in 0..10 {
        lower.file(&format!("/h/.cache/f{n}"), "x");
        upper.add("", format!("f{n}"), WHITEOUT);
    }
    let by_count = DeleteLimits { max_paths: 4, ..DeleteLimits::default() };
    let deletes = walk(&layer("/h/.cache"), &[], &upper, &lower, by_count);
    assert_eq!((deletes.paths().len(), deletes.more()), (4, 6));

    // Each path takes its length plus a quote on each side and a comma.
    let one = "/h/.cache/f0".len() + 3;
    let by_bytes = DeleteLimits { max_bytes: 3 * one, ..DeleteLimits::default() };
    let deletes = walk(&layer("/h/.cache"), &[], &upper, &lower, by_bytes);
    assert_eq!((deletes.paths().len(), deletes.more()), (3, 7));

    let by_entries = DeleteLimits { max_entries: 5, ..DeleteLimits::default() };
    let deletes = walk(&layer("/h/.cache"), &[], &upper, &lower, by_entries);
    assert_eq!(deletes.paths().len(), 5);
    assert!(deletes.more() >= 1, "the walk says that it stopped");

    // The limits hold across the layers of one call.
    let mut both = CacheDeletes::new(by_count);
    both.walk(&layer("/h/.cache"), &[], &upper, &lower);
    both.walk(&layer("/h/.cache"), &[], &upper, &lower);
    assert_eq!((both.paths().len(), both.more()), (4, 16));
}

#[test]
fn a_layer_that_cannot_be_read_counts_once() {
    let lower = FakeFs::new();
    let upper = FakeLayer::default();
    let deletes = walk(&layer("/h/.cache"), &[], &upper, &lower, DeleteLimits::default());
    assert_eq!((deletes.paths().len(), deletes.more()), (0, 1));
}

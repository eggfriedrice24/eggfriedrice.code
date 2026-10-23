use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use efr_shell::CommandResult;
use efr_test_support::bench::{self, Report};
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use serde_json::json;

use efr_scope::Home;

use super::{RealForms, check_real, reached, resolve, same_place};
use crate::testing::{FakeRunner, Fixture};
use crate::{PathAccess, ShellTool, Tool as _, ToolError, ToolRequirements};

#[test]
fn paths_resolve_against_home_and_cwd() {
    let fixture = Fixture::new();
    let ctx = fixture.context();
    let home = fixture.home();
    let cwd = fixture.cwd();
    assert_eq!(resolve(&ctx, "~").unwrap(), home);
    assert_eq!(resolve(&ctx, "~/.zshrc").unwrap(), home.join(".zshrc"));
    assert_eq!(resolve(&ctx, "notes.txt").unwrap(), cwd.join("notes.txt"));
    assert_eq!(resolve(&ctx, "/etc/./hosts").unwrap(), PathBuf::from("/etc/hosts"));
    assert_eq!(resolve(&ctx, "../x").unwrap(), cwd.parent().unwrap().join("x"));
    assert_eq!(resolve(&ctx, "/../../etc//passwd").unwrap(), PathBuf::from("/etc/passwd"));
}

#[test]
fn a_tilde_user_path_is_a_plain_relative_name() {
    let fixture = Fixture::new();
    assert_eq!(resolve(&fixture.context(), "~root/x").unwrap(), fixture.cwd().join("~root/x"));
}

#[test]
fn an_empty_path_is_refused() {
    let fixture = Fixture::new();
    assert!(matches!(resolve(&fixture.context(), ""), Err(ToolError::EmptyPath)));
}

#[test]
fn real_paths_and_paths_that_do_not_exist_yet_pass() {
    let fixture = Fixture::new();
    let home = Home::new(fixture.home()).unwrap();
    std::fs::write(fixture.cwd().join("file"), "x").unwrap();
    check_real(&home, &fixture.cwd().join("file")).unwrap();
    check_real(&home, &fixture.cwd().join("new/dir/file")).unwrap();
}

#[test]
fn a_symlink_in_the_path_is_refused_with_the_real_path() {
    let fixture = Fixture::new();
    let home = Home::new(fixture.home()).unwrap();
    let secrets = fixture.root().join("secrets");
    std::fs::create_dir(&secrets).unwrap();
    symlink(&secrets, fixture.cwd().join("link")).unwrap();
    let error = check_real(&home, &fixture.cwd().join("link/key")).unwrap_err();
    let ToolError::ThroughSymlink { real, .. } = error else {
        panic!("expected a symlink error, got {error:?}");
    };
    assert_eq!(real, secrets.join("key"));
}

#[test]
fn a_symlinked_file_is_refused() {
    let fixture = Fixture::new();
    let home = Home::new(fixture.home()).unwrap();
    std::fs::write(fixture.cwd().join("target"), "x").unwrap();
    symlink(fixture.cwd().join("target"), fixture.cwd().join("alias")).unwrap();
    assert!(matches!(
        check_real(&home, &fixture.cwd().join("alias")),
        Err(ToolError::ThroughSymlink { .. })
    ));
}

#[test]
fn a_home_reached_through_a_link_is_not_a_symlink_problem() {
    let fixture = Fixture::new();
    let real_home = fixture.home();
    let linked_home = fixture.root().join("linked-home");
    symlink(&real_home, &linked_home).unwrap();
    let home = Home::new(&linked_home).unwrap();
    std::fs::write(real_home.join(".zshrc"), "").unwrap();
    check_real(&home, &linked_home.join(".zshrc")).unwrap();
}

/// The look-up of v0.0.4, one `canonicalize` for each missing level of each path: the
/// reference that [`RealForms::real_form`] must equal.
fn reference_real_form(home: &Home, declared: &Path) -> Option<PathBuf> {
    match reached(declared) {
        Ok(Some(real)) if !same_place(home, declared, &real) => Some(real),
        Ok(_) | Err(_) => None,
    }
}

/// `ToolRequirements::with_real_paths` of v0.0.4, with linear checks and
/// [`reference_real_form`]: the reference of the new one.
fn reference_with_real_paths(mut declared: ToolRequirements, home: &Home) -> ToolRequirements {
    let mut paths: Vec<PathAccess> = Vec::with_capacity(declared.paths.len());
    for access in declared.paths {
        let real =
            if access.path.is_absolute() { reference_real_form(home, &access.path) } else { None };
        let mode = access.mode;
        let path = access.path.clone();
        paths.push(access);
        if let Some(real) = real {
            if !declared.links.iter().any(|(link, to)| *link == path && *to == real) {
                declared.links.push((path, real.clone()));
            }
            if !paths.iter().any(|known| known.path == real && known.mode == mode) {
                paths.push(PathAccess { path: real, mode });
            }
        }
    }
    declared.paths = paths;
    declared
}

/// A tree with every kind of link that a path can go through, below the root of a
/// fixture: dirs, files, absolute and relative links, a link to a file, a dangling
/// link, a loop, a link to `..`, a chain of links, a dir that cannot be searched, and a
/// link to the home.
struct LinkTree {
    fixture: Fixture,
    closed: PathBuf,
}

impl LinkTree {
    fn new() -> Self {
        let fixture = Fixture::new();
        let t = fixture.root().join("t");
        std::fs::create_dir_all(t.join("d/e")).unwrap();
        std::fs::write(t.join("d/e/f"), "f").unwrap();
        std::fs::write(t.join("d/file"), "file").unwrap();
        symlink(t.join("d"), t.join("ld")).unwrap();
        symlink("d/e", t.join("lrel")).unwrap();
        symlink(t.join("d/file"), t.join("lf")).unwrap();
        symlink(t.join("nowhere"), t.join("dangling")).unwrap();
        symlink("loop2", t.join("loop1")).unwrap();
        symlink("loop1", t.join("loop2")).unwrap();
        symlink("..", t.join("lup")).unwrap();
        symlink("../ld", t.join("d/back")).unwrap();
        std::fs::create_dir_all(t.join("noperm/inner")).unwrap();
        std::fs::write(fixture.home().join(".zshrc"), "").unwrap();
        symlink(t.join("d"), fixture.home().join("ld")).unwrap();
        symlink(fixture.home(), fixture.root().join("lhome")).unwrap();
        let closed = t.join("noperm");
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
        LinkTree { fixture, closed }
    }

    /// The dirs that the paths of a test start in.
    fn starts(&self) -> [PathBuf; 3] {
        [self.fixture.root().join("t"), self.fixture.home(), self.fixture.root().join("lhome")]
    }

    /// The home in its real form and through its link.
    fn homes(&self) -> [Home; 2] {
        [
            Home::new(self.fixture.home()).unwrap(),
            Home::new(self.fixture.root().join("lhome")).unwrap(),
        ]
    }
}

impl Drop for LinkTree {
    fn drop(&mut self) {
        // NOTE: the temporary dir cannot be removed while a dir in it cannot be read.
        let _ = std::fs::set_permissions(&self.closed, std::fs::Permissions::from_mode(0o755));
    }
}

/// One name of a path: a name of the tree, a missing one, `.`, `..`, an empty one
/// (`a//b`), a name as long as a name may be, one longer, and one with a NUL byte.
fn name() -> impl Strategy<Value = Vec<u8>> {
    let names = [
        "d", "e", "f", "file", "ld", "lrel", "lf", "dangling", "loop1", "lup", "back", "noperm",
        "inner", "missing", "x", "..", ".", "", ".zshrc", "t", "lhome", "home",
    ];
    prop_oneof![
        20 => prop::sample::select(names.to_vec()).prop_map(|name| name.as_bytes().to_vec()),
        1 => Just(vec![b'a'; 255]),
        1 => Just(vec![b'b'; 300]),
        1 => Just(b"n\0l".to_vec()),
    ]
}

/// One path of a call: the index of its start, its names, and a slash at the end.
type Drawn = (usize, Vec<Vec<u8>>, bool);

/// The path that `drawn` names below `starts`.
fn path_of(starts: &[PathBuf], (start, names, slash): &Drawn) -> PathBuf {
    let mut bytes = starts[*start % starts.len()].as_os_str().as_bytes().to_vec();
    for name in names {
        bytes.push(b'/');
        bytes.extend_from_slice(name);
    }
    if *slash {
        bytes.push(b'/');
    }
    PathBuf::from(OsStr::from_bytes(&bytes))
}

/// The paths of one call. The few names make many shared parents, and a long path is
/// longer than the system's limit.
fn calls() -> impl Strategy<Value = Vec<Drawn>> {
    let one = (0_usize..3, prop::collection::vec(name(), 0..8), prop::bool::weighted(0.1));
    let long = (0_usize..3, prop::collection::vec(name(), 14..24), Just(false));
    prop::collection::vec(prop_oneof![8 => one, 1 => long], 1..16)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn the_real_forms_of_a_call_are_those_of_one_look_up_of_each_level(
        call in calls(),
        home_at in 0_usize..2,
    ) {
        let tree = LinkTree::new();
        let starts = tree.starts();
        let home = &tree.homes()[home_at];
        let mut forms = RealForms::new(home);
        for path in call.iter().map(|drawn| path_of(&starts, drawn)) {
            prop_assert_eq!(forms.real_form(&path), reference_real_form(home, &path), "{:?}", path);
        }
    }

    #[test]
    fn with_real_paths_declares_what_it_declared_with_linear_look_ups(
        call in calls(),
        modes in prop::collection::vec(0_u8..3, 16),
        home_at in 0_usize..2,
    ) {
        let tree = LinkTree::new();
        let starts = tree.starts();
        let home = &tree.homes()[home_at];
        let mut declared = ToolRequirements::none();
        for (drawn, mode) in call.iter().zip(&modes) {
            let path = path_of(&starts, drawn);
            declared = match mode {
                0 => declared.with_read(path),
                1 => declared.with_read_tree(path),
                _ => declared.with_write(path),
            };
        }
        declared.links.push((starts[0].join("ld"), starts[0].join("d")));
        let expected = reference_with_real_paths(declared.clone(), home);
        prop_assert_eq!(declared.with_real_paths(home), expected);
    }
}

#[test]
fn a_path_below_a_missing_dir_needs_no_look_up_of_its_own() {
    let tree = LinkTree::new();
    let [t, ..] = tree.starts();
    let home = &tree.homes()[0];
    let mut forms = RealForms::new(home);
    assert_eq!(forms.real_form(&t.join("ld/missing/a/b")), Some(t.join("d/missing/a/b")));
    let after_first = forms.lookups();
    for name in ["c", "d", "e/f", "g/h/i"] {
        let path = t.join("ld/missing/a/b").join(name);
        assert_eq!(forms.real_form(&path), Some(t.join("d/missing/a/b").join(name)));
    }
    assert_eq!(forms.lookups(), after_first);
    // A dir that cannot be searched fails for every path below it, as before.
    for path in [t.join("noperm/inner/x"), t.join("noperm/inner/x/y/z"), t.join("lf/x")] {
        assert_eq!(forms.real_form(&path), reference_real_form(home, &path));
    }
}

/// The line of the shell call bench of efrd (`bench_perf_shell_call_auto`): `k` cds
/// into nested dirs, a read there, and a cd back.
fn cds_line(project: &Path, k: u64) -> String {
    let cds: Vec<String> = (1..=k).map(|n| format!("cd c{n}")).collect();
    format!("{} && cat x; cd {}", cds.join(" && "), project.display())
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_real_paths() {
    let fixture = Fixture::new();
    let project = fixture.cwd();
    let mut deep = project.clone();
    for n in 1..=10 {
        deep = deep.join(format!("c{n}"));
    }
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("x"), "x\n").unwrap();
    let home = Home::new(fixture.home()).unwrap();
    let tool = ShellTool::new(FakeRunner::answering(Ok(CommandResult::finished(Some(0), "", "/"))));
    let mut report = Report::new("bench_perf_real_paths");
    let mut failures = Vec::new();
    let mut points = Vec::new();
    // NOTE: ten cds reach the cap of 1024 bases, so more cds declare no more paths.
    for k in [4_u64, 6, 8, 10] {
        let input = json!({ "command": cds_line(&project, k) });
        let declared = tool.requirements(&fixture.context(), &input).unwrap();
        let paths = declared.paths.len() as u64;
        let mut forms = RealForms::new(&home);
        for access in &declared.paths {
            forms.real_form(&access.path);
        }
        let lookups = forms.lookups() as u64;
        let runs = bench::run(2, 10, || {
            std::hint::black_box(declared.clone().with_real_paths(&home));
        });
        let stats = report.time(&format!("{k} cds: with_real_paths"), &runs);
        let start = Instant::now();
        let expected = reference_with_real_paths(declared.clone(), &home);
        report.time(&format!("{k} cds: the look-ups of v0.0.4"), &[start.elapsed()]);
        assert_eq!(declared.clone().with_real_paths(&home), expected);
        report.count(&format!("{k} cds: declared paths"), paths);
        report.count(&format!("{k} cds: look-ups"), lookups);
        points.push((paths, stats.p50));
        // Budget: a path below a dir that does not exist needs no look-up of its own,
        // so at 10 cds the look-ups stay below a tenth of the paths. v0.0.4 looked up
        // each missing level of each path: about 68 ms of the 95 ms that efrd took to
        // decide the line of 10 cds.
        if k == 10 && lookups * 10 > paths {
            failures.push(format!("{k} cds: {lookups} look-ups for {paths} paths"));
        }
        if stats.p50 > Duration::from_millis(15) {
            failures.push(format!("{k} cds: with_real_paths took {:?}", stats.p50));
        }
    }
    let slope = report.slope("slope over declared paths", &points);
    report.finish();
    if slope > 1.3 {
        failures.push(format!("the slope over the declared paths is {slope:.2}"));
    }
    assert!(failures.is_empty(), "the budget failed: {failures:?}");
}

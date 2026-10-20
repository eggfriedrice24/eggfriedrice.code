use std::path::PathBuf;

use pretty_assertions::assert_eq;

use crate::SandboxError;
use crate::modules::{MAX_MODULE_DEPTH, module_git_dirs, walk};
use crate::testing::FakeFs;

const GIT: &str = "/p/app/.git";

/// A module git dir as git makes it, at `GIT/modules/<name>`.
fn module(fs: &mut FakeFs, name: &str) {
    let dir = format!("{GIT}/modules/{name}");
    fs.file(&format!("{dir}/HEAD"), "ref: refs/heads/main\n")
        .file(&format!("{dir}/config"), "[core]\n\tbare = false\n")
        .file(&format!("{dir}/hooks/pre-push.sample"), "#!/bin/sh\n")
        .dir(&format!("{dir}/refs/heads"))
        .dir(&format!("{dir}/objects/pack"))
        .dir(&format!("{dir}/logs/refs"));
}

fn paths(names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(|name| PathBuf::from(format!("{GIT}/modules/{name}"))).collect()
}

#[test]
fn walk_finds_slash_named_and_nested_modules() {
    let mut fs = FakeFs::new();
    fs.file(&format!("{GIT}/config"), "");
    module(&mut fs, "lib");
    module(&mut fs, "vendor/foo");
    module(&mut fs, "a");
    module(&mut fs, "a/modules/b");
    module(&mut fs, "a/modules/b/modules/c/d");
    let found = module_git_dirs(GIT.as_ref(), &fs).unwrap();
    assert_eq!(found, paths(&["a", "a/modules/b", "a/modules/b/modules/c/d", "lib", "vendor/foo"]));
}

#[test]
fn walk_without_modules_is_empty() {
    let mut fs = FakeFs::new();
    fs.file(&format!("{GIT}/config"), "");
    assert_eq!(module_git_dirs(GIT.as_ref(), &fs).unwrap(), Vec::<PathBuf>::new());
}

#[test]
fn walk_skips_objects_refs_logs() {
    // A dir that looks like a git dir inside a module's objects, refs, logs or lfs is
    // never one that git uses for a submodule; the walk does not even list them.
    let mut fs = FakeFs::new();
    module(&mut fs, "lib");
    for skipped in ["objects/ab", "refs/heads/x", "logs/refs/y", "lfs/objects/z"] {
        fs.file(&format!("{GIT}/modules/lib/{skipped}/HEAD"), "")
            .file(&format!("{GIT}/modules/lib/{skipped}/config"), "");
    }
    assert_eq!(module_git_dirs(GIT.as_ref(), &fs).unwrap(), paths(&["lib"]));

    // The same names below a dir that is not a git dir are entered.
    let mut fs = FakeFs::new();
    module(&mut fs, "objects/x");
    assert_eq!(module_git_dirs(GIT.as_ref(), &fs).unwrap(), paths(&["objects/x"]));
}

#[test]
fn walk_stops_at_depth_8_and_says_so() {
    // Seven dirs and a module git dir with no subdir: depth 8 is listed.
    let mut fs = FakeFs::new();
    let deepest = "1/2/3/4/5/6/7/8";
    fs.file(&format!("{GIT}/modules/{deepest}/HEAD"), "")
        .file(&format!("{GIT}/modules/{deepest}/config"), "");
    assert_eq!(module_git_dirs(GIT.as_ref(), &fs).unwrap(), paths(&[deepest]));

    // One level more: the walk does not list it and says where it stopped.
    let mut fs = FakeFs::new();
    fs.file(&format!("{GIT}/modules/{deepest}/9/HEAD"), "");
    module(&mut fs, "lib");
    let error = module_git_dirs(GIT.as_ref(), &fs).unwrap_err();
    let at = PathBuf::from(format!("{GIT}/modules/{deepest}/9"));
    assert!(
        matches!(&error, SandboxError::ModulesTooDeep { at: found } if *found == at),
        "{error:?}"
    );
    // The plan and the guard still get every module above the limit.
    let found = walk(GIT.as_ref(), &fs);
    assert_eq!(found.git_dirs, paths(&["lib"]));
    assert_eq!(found.too_deep, Some(at));
    assert_eq!(MAX_MODULE_DEPTH, 8);
}

#[test]
fn walk_refuses_a_link_at_any_depth() {
    for link in ["modules", "modules/vendor", "modules/vendor/foo", "modules/a/modules/b"] {
        let mut fs = FakeFs::new();
        module(&mut fs, "a");
        fs.dir("/p/evil/hooks").link(&format!("{GIT}/{link}"), "/p/evil");
        let error = module_git_dirs(GIT.as_ref(), &fs).unwrap_err();
        let path = PathBuf::from(format!("{GIT}/{link}"));
        assert!(
            matches!(&error, SandboxError::SymlinkedPin { path: found } if *found == path),
            "{link}: {error:?}"
        );
    }
    // A link inside a module git dir that is not its `modules` is the plan's floor rule.
    let mut fs = FakeFs::new();
    module(&mut fs, "lib");
    fs.link(&format!("{GIT}/modules/lib/info"), "/p/evil");
    assert_eq!(module_git_dirs(GIT.as_ref(), &fs).unwrap(), paths(&["lib"]));
}

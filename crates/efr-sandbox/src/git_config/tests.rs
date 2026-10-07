use pretty_assertions::assert_eq;

use crate::git_config::{cargo_code_keys, code_keys, is_code_key, parse_config};

#[test]
fn code_keys_are_found_in_both_listing_forms() {
    let plain = "core.bare=false\ncore.fsmonitor=./x\nalias.st=status\nalias.x=!sh -c y\nfilter.lfs.clean=git-lfs clean\n";
    assert_eq!(code_keys(plain), ["core.fsmonitor", "alias.x", "filter.lfs.clean"]);
    let z = "core.bare\nfalse\0remote.origin.uploadpack\n./up\0credential.helper\nstore\0core.fsmonitor\nfalse\0";
    assert_eq!(code_keys(z), ["remote.origin.uploadpack", "credential.helper"]);
}

#[test]
fn every_key_of_the_rule_list_is_a_code_key() {
    for key in [
        "core.fsmonitor",
        "core.hookspath",
        "core.sshcommand",
        "core.pager",
        "core.editor",
        "core.askpass",
        "core.gitproxy",
        "sequence.editor",
        "diff.external",
        "diff.pdf.textconv",
        "diff.x.command",
        "merge.ours.driver",
        "filter.a.smudge",
        "credential.https://x.helper",
        "gpg.program",
        "gpg.ssh.program",
        "include.path",
        "includeIf.gitdir:~/w/.path",
        "uploadpack.packObjectsHook",
        "remote.a.b.uploadpack",
        "remote.o.receivepack",
    ] {
        assert!(is_code_key(key, "x"), "{key}");
    }
    for key in ["core.bare", "user.name", "remote.origin.url", "diff.renames", "alias.co"] {
        assert!(!is_code_key(key, "checkout"), "{key}");
    }
    assert!(!is_code_key("core.fsmonitor", "false"));
}

#[test]
fn the_config_reader_handles_sections_quotes_and_comments() {
    let text = "[core]\n\thooksPath = \".husky/_\" # husky\n[includeIf \"gitdir:~/w/\"]\n\tpath = ~/w.gitconfig\n[Include]\npath=../a ; c\n[x.Y]\nflag\n";
    assert_eq!(
        parse_config(text),
        [
            ("core.hookspath".to_owned(), ".husky/_".to_owned()),
            ("includeif.gitdir:~/w/.path".to_owned(), "~/w.gitconfig".to_owned()),
            ("include.path".to_owned(), "../a".to_owned()),
            ("x.y.flag".to_owned(), "true".to_owned()),
        ]
    );
}

#[test]
fn cargo_keys_that_run_programs_are_named() {
    let text = "\
[build]
rustc-wrapper = \"sccache\" # cache
jobs = 4
[target.'cfg(unix)']
runner = \"./run\"
[host]
linker = \"cc\"
[source.crates-io]
replace-with = \"vendored\"
[env]
X = \"rustc-wrapper\"
target.x86_64-unknown-linux-gnu.linker = \"mold\"
";
    assert_eq!(
        cargo_code_keys(text),
        [
            "build.rustc-wrapper",
            "target.cfg(unix).runner",
            "host.linker",
            "source.crates-io.replace-with",
        ]
    );
    assert_eq!(
        cargo_code_keys("build.rustc-workspace-wrapper = \"w\"\n"),
        ["build.rustc-workspace-wrapper"]
    );
    assert_eq!(cargo_code_keys("target = { x = { runner = \"r\" } }\n"), ["target.*.runner"]);
    assert_eq!(
        cargo_code_keys("target = { x = { rustflags = [\"-C\"] } }\n"),
        Vec::<String>::new()
    );
    assert_eq!(cargo_code_keys("[target]\nx86 = { runner = \"r\" }\n"), ["target.x86.runner"]);
}

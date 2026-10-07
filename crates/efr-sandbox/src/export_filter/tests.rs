use std::path::PathBuf;

use pretty_assertions::assert_eq;

use crate::export_filter::{ExportFilter, ExportVerdict, KeepReason, overlay_denied};
use crate::plan::MountPlan;
use crate::testing::{PROJECT, spec, world};

fn filter() -> ExportFilter {
    let spec = spec();
    let plan = MountPlan::build(&spec, &world()).unwrap();
    ExportFilter::from_spec(&spec, &plan, &PathBuf::from(PROJECT))
}

#[test]
fn export_filter_promotes_only_listed_names() {
    let filter = filter();
    assert_eq!(filter.check("RUST_LOG", "debug"), ExportVerdict::Promote);
    assert_eq!(filter.check("LC_ALL", "C.UTF-8"), ExportVerdict::Promote);
    assert_eq!(filter.check("FOO", "1"), ExportVerdict::KeepInSandbox(KeepReason::NotListed));
    assert_eq!(
        filter.check("VIRTUAL_ENV", "/x"),
        ExportVerdict::KeepInSandbox(KeepReason::NotListed)
    );
    assert_eq!(filter.check("1BAD", "1"), ExportVerdict::KeepInSandbox(KeepReason::BadName));
}

#[test]
fn export_filter_never_list_wins() {
    // PATH is on the promote list of the test spec, and still never returns.
    let filter = filter();
    assert_eq!(
        filter.check("PATH", "/usr/bin"),
        ExportVerdict::KeepInSandbox(KeepReason::NeverList)
    );
    let wide = ExportFilter::new(
        vec!["*".to_owned()],
        vec!["MINE_*".to_owned()],
        Vec::new(),
        "/home/u".into(),
        "/".into(),
        4096,
    );
    for name in
        ["GIT_DIR", "NODE_OPTIONS", "CARGO_HOME", "PYTHONPATH", "MINE_X", "PS1", "SSH_AUTH_SOCK"]
    {
        assert_eq!(
            wide.check(name, "x"),
            ExportVerdict::KeepInSandbox(KeepReason::NeverList),
            "{name}"
        );
    }
    assert_eq!(wide.check("NODE_ENV", "production"), ExportVerdict::Promote);
    assert_eq!(wide.check("GH_TOKEN", "x"), ExportVerdict::KeepInSandbox(KeepReason::SecretName));
    assert_eq!(wide.check("TZ", "a\u{1b}b"), ExportVerdict::KeepInSandbox(KeepReason::BadValue));
    assert_eq!(
        wide.check("TZ", &"a".repeat(4097)),
        ExportVerdict::KeepInSandbox(KeepReason::BadValue)
    );
    assert_eq!(wide.check("TZ", "UTC\tx"), ExportVerdict::Promote);
}

#[test]
fn export_filter_keeps_out_values_in_roots() {
    let mut spec = spec();
    spec.env.promote = vec!["*".to_owned()];
    let plan = MountPlan::build(&spec, &world()).unwrap();
    let filter = ExportFilter::from_spec(&spec, &plan, &PathBuf::from(PROJECT));
    for value in [
        "/home/u/p/app/bin",
        "x:/tmp/evil",
        "--flag=./tool",
        "~/p/app/x",
        "/home/u/.cargo/bin/x",
        "/var/tmp/a b",
    ] {
        assert_eq!(
            filter.check("TOOL_HOME", value),
            ExportVerdict::KeepInSandbox(KeepReason::ValueInRoot),
            "{value}"
        );
    }
    assert_eq!(filter.check("TOOL_HOME", "/usr/share/tool"), ExportVerdict::Promote);

    let mut fs = world();
    fs.link("/home/u/link", "/home/u/p/app/bin").dir("/home/u/p/app/bin");
    assert_eq!(filter.check("TOOL_HOME", "/home/u/link"), ExportVerdict::Promote);
    assert_eq!(
        filter.check_resolving("TOOL_HOME", "/home/u/link", &fs),
        ExportVerdict::KeepInSandbox(KeepReason::ValueInRoot)
    );
}

#[test]
fn export_filter_keeps_out_relative_paths() {
    let mut spec = spec();
    spec.env.promote = vec!["*".to_owned()];
    let plan = MountPlan::build(&spec, &world()).unwrap();
    let filter = ExportFilter::from_spec(&spec, &plan, &PathBuf::from(PROJECT));
    // In the trusted shell a relative path resolves against that shell's directory,
    // which can be a write root, so it never returns, wherever the call ended.
    let outside = ExportFilter::from_spec(&spec, &plan, &PathBuf::from("/usr/share"));
    for value in ["node_modules/.bin", "/usr/lib:lib/x", "Europe/Paris", "~bob/bin", ".bin"] {
        for filter in [&filter, &outside] {
            let verdict = filter.check("TOOL_PATH", value);
            assert!(
                matches!(
                    verdict,
                    ExportVerdict::KeepInSandbox(
                        KeepReason::RelativePath | KeepReason::ValueInRoot
                    )
                ),
                "{value}: {verdict:?}"
            );
        }
    }
    assert_eq!(
        outside.check("TOOL_PATH", "node_modules/.bin"),
        ExportVerdict::KeepInSandbox(KeepReason::RelativePath)
    );
    // A bare word that names an entry of the call's directory in a write root is a
    // path into that root; one that names nothing is a word.
    let mut fs = world();
    fs.dir("/home/u/p/app/bin");
    for value in ["bin", "/usr/bin:bin", "bin:/usr/bin"] {
        assert_eq!(
            filter.check_resolving("TOOL_PATH", value, &fs),
            ExportVerdict::KeepInSandbox(KeepReason::ValueInRoot),
            "{value}"
        );
    }
    assert_eq!(filter.check_resolving("RUST_LOG", "debug", &fs), ExportVerdict::Promote);
    assert_eq!(filter.check_resolving("LANG", "C.UTF-8", &fs), ExportVerdict::Promote);
    assert_eq!(outside.check_resolving("TOOL_PATH", "bin", &fs), ExportVerdict::Promote);
    // An empty value, as an unset checks it, still returns.
    assert_eq!(filter.check("TOOL_PATH", ""), ExportVerdict::Promote);
}

#[test]
fn overlay_deny_drops_ld_star() {
    let filter = filter();
    for name in
        ["LD_PRELOAD", "LD_LIBRARY_PATH", "BASH_ENV", "ZDOTDIR", "HOME", "EFR_X", "http_proxy"]
    {
        assert!(overlay_denied(name), "{name}");
        assert_eq!(filter.check(name, "x"), ExportVerdict::Drop, "{name}");
    }
    assert!(!overlay_denied("PATH"));
}

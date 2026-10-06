use std::fs;

use efr_sandbox::{ExportFilter, Records};
use pretty_assertions::assert_eq;

use crate::real_fs::RealFs;
use crate::testing::TestCall;

use super::*;

#[test]
fn a_cwd_in_the_project_returns_to_the_shell() {
    let call = TestCall::new();
    let plan = call.plan();
    let dir = call.project().join("src");
    fs::create_dir(&dir).unwrap();
    assert_eq!(contained_cwd(&dir, &plan, &RealFs), FinalCwd::Host(dir));
}

#[test]
fn a_cwd_in_the_private_tmp_stays_in_the_sandbox() {
    let call = TestCall::new();
    let plan = call.plan();
    fs::create_dir(call.spec.runtime.private_tmp().join("work")).unwrap();
    assert_eq!(
        contained_cwd(Path::new("/tmp/work"), &plan, &RealFs),
        FinalCwd::Private {
            inside: PathBuf::from("/tmp/work"),
            host: call.spec.runtime.private_tmp().join("work"),
        }
    );
    assert_eq!(contained_cwd(Path::new("/var/tmp/gone"), &plan, &RealFs), FinalCwd::Stay);
}

#[test]
fn a_masked_or_missing_cwd_stays() {
    let call = TestCall::new();
    fs::create_dir_all(call.spec.runtime.data.join("scratch")).unwrap();
    let plan = call.plan();
    assert_eq!(contained_cwd(&call.spec.runtime.data, &plan, &RealFs), FinalCwd::Stay);
    assert_eq!(contained_cwd(&call.project().join("missing"), &plan, &RealFs), FinalCwd::Stay);
}

#[test]
fn the_exit_child_cwd_skips_efr_roots() {
    let call = TestCall::new();
    fs::create_dir_all(&call.spec.runtime.state).unwrap();
    assert_eq!(exit_child_cwd(&call.spec.runtime.state, &call.spec, &RealFs), FinalCwd::Stay);
    assert_eq!(
        exit_child_cwd(call.project(), &call.spec, &RealFs),
        FinalCwd::Host(call.project().to_path_buf())
    );
}

#[test]
fn only_listed_exports_outside_the_roots_are_promoted() {
    let call = TestCall::new();
    let plan = call.plan();
    let mut spec = call.spec.clone();
    spec.env.promote = vec!["RUST_LOG".to_owned(), "VIRTUAL_ENV".to_owned()];
    let filter = ExportFilter::from_spec(&spec, &plan, call.project());
    let venv = call.project().join(".venv").display().to_string();
    let records = Records {
        exports: vec![
            ("RUST_LOG".to_owned(), "debug".to_owned()),
            ("PATH".to_owned(), "/usr/bin".to_owned()),
            ("LD_PRELOAD".to_owned(), "/tmp/x.so".to_owned()),
            ("VIRTUAL_ENV".to_owned(), venv),
        ],
        unsets: vec!["RUST_LOG".to_owned(), "PATH".to_owned()],
        status: Some(0),
        ..Records::default()
    };
    let mut summary = SandboxSummary::default();
    let promotion = promote(&records, None, &filter, &RealFs, &mut summary);
    assert_eq!(promotion.exports, vec![("RUST_LOG".to_owned(), "debug".to_owned())]);
    assert_eq!(promotion.unsets, vec!["RUST_LOG".to_owned()]);
    assert_eq!(summary.promoted, vec!["RUST_LOG".to_owned()]);
    assert_eq!(summary.kept_out, vec!["PATH".to_owned(), "VIRTUAL_ENV".to_owned()]);
    assert_eq!(summary.dropped, vec!["LD_PRELOAD".to_owned()]);
}

#[test]
fn a_private_cwd_is_kept_with_the_shell_dir_of_its_time() {
    let mut state = SandboxState::default();
    let records = Records { status: Some(0), ..Records::default() };
    let cwd =
        FinalCwd::Private { inside: PathBuf::from("/tmp/w"), host: PathBuf::from("/x/tmp/w") };
    update_state(&mut state, &records, &cwd, Path::new("/home/u/p"));
    assert_eq!(
        state.sandbox_cwd,
        Some(SandboxCwd { path: PathBuf::from("/tmp/w"), shell_pwd: PathBuf::from("/home/u/p") })
    );
    update_state(&mut state, &records, &FinalCwd::Stay, Path::new("/home/u/p"));
    assert_eq!(state.sandbox_cwd, None);
}

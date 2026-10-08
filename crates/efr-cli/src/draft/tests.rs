use std::os::unix::fs::PermissionsExt as _;

use efr_stdx::env::{Env, Var};
use pretty_assertions::assert_eq;

use super::hand_back;
use crate::context::Context;
use crate::testing::TestEnv;

fn with_draft_file(env: &TestEnv, path: &std::path::Path) -> Context {
    Context { env: Env::fixed([(Var::DraftFile, path.as_os_str())]), ..env.context() }
}

#[tokio::test]
async fn the_text_goes_to_the_plugins_file_without_a_final_newline_and_private() {
    let env = TestEnv::new();
    let dir = tempfile::tempdir().unwrap();
    // The directory that holds the file does not exist yet.
    let path = dir.path().join("drafts").join("4242");
    let ctx = with_draft_file(&env, &path);

    assert_eq!(hand_back(&ctx, "fix the tests\nthen push\n").await, None);

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "fix the tests\nthen push");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let dir_mode = std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode();
    assert_eq!(dir_mode & 0o777, 0o700);
}

#[tokio::test]
async fn without_the_plugin_the_text_is_one_note() {
    let env = TestEnv::new();
    let note = hand_back(&env.context(), "fix the tests\nthen push").await;
    assert_eq!(note.as_deref(), Some("not sent: fix the tests then push"));
}

#[tokio::test]
async fn a_file_that_cannot_be_written_leaves_the_note() {
    let env = TestEnv::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing").join("deeper").join("4242");
    let note = hand_back(&with_draft_file(&env, &path), "keep me").await;
    assert_eq!(note.as_deref(), Some("not sent: keep me"));
    assert!(!path.exists());
}

#[tokio::test]
async fn a_blank_text_is_not_handed_back() {
    let env = TestEnv::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("4242");
    assert_eq!(hand_back(&with_draft_file(&env, &path), " \n\n").await, None);
    assert!(!path.exists());
    assert_eq!(hand_back(&env.context(), "").await, None);
}

#[tokio::test]
async fn a_control_character_shows_as_a_stand_in_in_the_note() {
    let env = TestEnv::new();
    let note = hand_back(&env.context(), "a\x1b[2Jb").await;
    assert_eq!(note.as_deref(), Some("not sent: a\u{241b}[2Jb"));
}

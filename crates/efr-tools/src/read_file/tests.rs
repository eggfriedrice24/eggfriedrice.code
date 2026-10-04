use std::os::unix::fs::symlink;

use pretty_assertions::assert_eq;
use serde_json::json;

use super::{ReadFileTool, select_lines};
use crate::testing::Fixture;
use crate::{AccessMode, NoOutput, PathAccess, Tool as _, ToolError};

#[test]
fn it_declares_the_resolved_path_for_reading() {
    let fixture = Fixture::new();
    let requirements =
        ReadFileTool::new().requirements(&fixture.context(), &json!({"path": "~/.zshrc"})).unwrap();
    assert_eq!(
        requirements.paths,
        [PathAccess { path: fixture.home().join(".zshrc"), mode: AccessMode::Read }]
    );
    assert_eq!(requirements.command, None);
    assert!(!requirements.network && !requirements.interactive);
}

#[test]
fn input_with_unknown_fields_is_invalid() {
    let fixture = Fixture::new();
    let error =
        ReadFileTool::new().requirements(&fixture.context(), &json!({"file": "x"})).unwrap_err();
    assert!(matches!(error, ToolError::InvalidInput { .. }), "{error:?}");
}

#[tokio::test]
async fn it_reads_a_whole_file() {
    let fixture = Fixture::new();
    std::fs::write(fixture.cwd().join("notes.txt"), "one\ntwo\n").unwrap();
    let result = ReadFileTool::new()
        .invoke(fixture.context(), json!({"path": "notes.txt"}), &mut NoOutput)
        .await
        .unwrap();
    assert_eq!(result.output, "one\ntwo\n");
    assert!(!result.is_error && !result.truncated);
}

#[tokio::test]
async fn it_reads_a_range_of_lines() {
    let fixture = Fixture::new();
    std::fs::write(fixture.cwd().join("f"), "1\n2\n3\n4\n5\n").unwrap();
    let result = ReadFileTool::new()
        .invoke(fixture.context(), json!({"path": "f", "offset": 2, "limit": 2}), &mut NoOutput)
        .await
        .unwrap();
    assert_eq!(result.output, "2\n3\n[lines 2 to 3 of 5]");
}

#[tokio::test]
async fn long_files_are_cut_in_the_middle() {
    let fixture = Fixture::new();
    std::fs::write(fixture.cwd().join("big"), "x".repeat(10_000)).unwrap();
    let result = ReadFileTool::new()
        .with_output_limit(200)
        .invoke(fixture.context(), json!({"path": "big"}), &mut NoOutput)
        .await
        .unwrap();
    assert!(result.truncated);
    assert!(result.output.len() <= 200);
}

#[tokio::test]
async fn binary_files_directories_and_missing_files_fail() {
    let fixture = Fixture::new();
    std::fs::write(fixture.cwd().join("bin"), b"\x7fELF\x00\x01").unwrap();
    let tool = ReadFileTool::new();
    let ctx = fixture.context();
    let binary = tool.invoke(ctx.clone(), json!({"path": "bin"}), &mut NoOutput).await;
    assert!(matches!(binary, Err(ToolError::NotText { .. })), "{binary:?}");
    let dir = tool.invoke(ctx.clone(), json!({"path": "."}), &mut NoOutput).await;
    assert!(matches!(dir, Err(ToolError::NotAFile { .. })), "{dir:?}");
    let missing = tool.invoke(ctx, json!({"path": "nope"}), &mut NoOutput).await;
    assert!(matches!(missing, Err(ToolError::Read { .. })), "{missing:?}");
}

#[tokio::test]
async fn a_path_through_a_symlink_fails() {
    let fixture = Fixture::new();
    std::fs::write(fixture.root().join("secret"), "key").unwrap();
    symlink(fixture.root().join("secret"), fixture.cwd().join("innocent")).unwrap();
    let result = ReadFileTool::new()
        .invoke(fixture.context(), json!({"path": "innocent"}), &mut NoOutput)
        .await;
    assert!(matches!(result, Err(ToolError::ThroughSymlink { .. })), "{result:?}");
}

#[test]
fn a_range_past_the_end_says_so() {
    assert_eq!(select_lines("a\nb\n", Some(5), None), "[the file has 2 lines; none from line 5]");
    assert_eq!(select_lines("a\nb", Some(2), Some(9)), "b\n[lines 2 to 2 of 2]");
    assert_eq!(select_lines("a\nb", None, None), "a\nb");
}

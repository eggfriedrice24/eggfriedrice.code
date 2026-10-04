use std::path::PathBuf;

use efr_protocol::Seq;
use efr_test_support::TestRng;
use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::{SentinelRun, quote, sentinel_line, token};
use crate::run::{Completion, RunOutput};

const TOKEN: &str = "00c0ffee00c0ffee";

/// The bytes a shell sends back for a sentinel line: the echo, the begin marker, the
/// output and the end marker, with the terminal's `\r\n` line ends.
fn reply(output: &str, status: i32, pwd: &str) -> Vec<u8> {
    format!(
        "printf '__efr_%s_b\\n' {TOKEN}; eval 'x'; printf '\\n__efr_%s_e:%s:%s\\n' {TOKEN} \"$?\" \"$PWD\"\r\n\
         __efr_{TOKEN}_b\r\n{output}\r\n__efr_{TOKEN}_e:{status}:{pwd}\r\n$ "
    )
    .into_bytes()
}

fn run_whole(bytes: &[u8]) -> RunOutput {
    let mut run = SentinelRun::new(TOKEN, 1024);
    let (ended, _) = run.on_bytes(Seq::new(100), bytes);
    ended.expect("the run should end")
}

#[test]
fn the_line_prints_markers_around_an_eval() {
    let line = sentinel_line("ls -la", TOKEN).unwrap();
    assert_eq!(
        &line[..],
        format!(
            "printf '__efr_%s_b\\n' {TOKEN}; eval 'ls -la'; printf '\\n__efr_%s_e:%s:%s\\n' {TOKEN} \"$?\" \"$PWD\"\r"
        )
        .as_bytes()
    );
}

#[test]
fn the_typed_line_never_contains_a_marker() {
    let line = sentinel_line("echo hi", TOKEN).unwrap();
    let text = String::from_utf8(line.to_vec()).unwrap();
    assert!(!text.contains(&format!("__efr_{TOKEN}_b")));
    assert!(!text.contains(&format!("__efr_{TOKEN}_e:")));
}

#[test]
fn quotes_keep_single_quotes_and_control_characters_out_of_the_line() {
    assert_eq!(quote("echo 'hi'"), r"'echo '\''hi'\'''");
    assert_eq!(quote("a\nb\tc\\d'e"), r"$'a\nb\tc\\d\'e'");
    assert_eq!(quote("x\u{1b}y"), r"$'x\x1by'");
}

#[test]
fn nul_bytes_and_empty_commands_are_refused() {
    assert!(sentinel_line("a\0b", TOKEN).is_err());
    assert!(sentinel_line("  ", TOKEN).is_err());
}

#[test]
fn tokens_come_from_the_generator() {
    let first = token(&TestRng::new(7));
    assert_eq!(first, token(&TestRng::new(7)));
    assert_eq!(first.len(), 16);
    assert_ne!(first, token(&TestRng::new(8)));
}

#[test]
fn the_output_status_and_directory_come_from_the_markers() {
    let ended = run_whole(&reply("one\r\ntwo", 3, "/etc/nixos"));
    assert_eq!(ended.completion, Completion::Finished);
    assert_eq!(ended.exit_code, Some(3));
    assert_eq!(ended.cwd, Some(PathBuf::from("/etc/nixos")));
    assert_eq!(ended.captured.text, "one\ntwo");
}

#[test]
fn output_that_ends_with_a_newline_keeps_it() {
    let ended = run_whole(&reply("line\r\n", 0, "/"));
    assert_eq!(ended.captured.text, "line\n");
}

#[test]
fn empty_output_is_empty() {
    let ended = run_whole(&reply("", 0, "/"));
    assert_eq!(ended.captured.text, "");
    assert_eq!(ended.captured.bytes, 0);
}

#[test]
fn the_range_points_into_the_stream() {
    let bytes = reply("abc", 0, "/");
    let ended = run_whole(&bytes);
    let range = ended.range.unwrap();
    let start = usize::try_from(range.start.get() - 100).unwrap();
    let end = usize::try_from(range.end.get() - 100).unwrap();
    assert_eq!(&bytes[start..end], b"abc");
}

#[test]
fn a_marker_with_another_token_is_output() {
    let other = "__efr_ffffffffffffffff_e:9:/nope\r\n";
    let ended = run_whole(&reply(other.trim_end(), 0, "/"));
    assert_eq!(ended.captured.text, other.trim_end());
    assert_eq!(ended.exit_code, Some(0));
}

#[test]
fn a_run_without_its_end_marker_keeps_going() {
    let mut run = SentinelRun::new(TOKEN, 1024);
    let (ended, captured) =
        run.on_bytes(Seq::ZERO, format!("__efr_{TOKEN}_b\r\nwaiting").as_bytes());
    assert!(ended.is_none());
    assert!(!captured, "a short output stays held in case it starts a marker");
    let (ended, captured) = run.on_bytes(Seq::new(30), &[b'.'; 64]);
    assert!(ended.is_none());
    assert!(captured);
    assert!(run.partial().0.text.starts_with("waiting"));
}

#[test]
fn a_status_that_is_not_a_number_is_unknown() {
    let bytes = format!("__efr_{TOKEN}_b\r\nx\r\n__efr_{TOKEN}_e::/tmp\r\n").into_bytes();
    let ended = run_whole(&bytes);
    assert_eq!(ended.exit_code, None);
    assert_eq!(ended.cwd, Some(PathBuf::from("/tmp")));
}

proptest! {
    /// However the reply is split into chunks, the run ends the same way.
    #[test]
    fn chunks_split_anywhere_give_the_same_result(
        output in "[a-z \r\n_]{0,40}",
        cuts in proptest::collection::vec(0usize..300, 0..10),
    ) {
        let bytes = reply(&output, 1, "/srv");
        let whole = run_whole(&bytes);
        let mut run = SentinelRun::new(TOKEN, 1024);
        let mut cuts: Vec<usize> = cuts.into_iter().map(|cut| cut.min(bytes.len())).collect();
        cuts.sort_unstable();
        cuts.push(bytes.len());
        let mut from = 0;
        let mut ended = None;
        for cut in cuts {
            let (result, _) = run.on_bytes(Seq::new(100 + from as u64), &bytes[from..cut]);
            if result.is_some() {
                ended = result;
                break;
            }
            from = cut;
        }
        prop_assert_eq!(ended, Some(whole));
    }
}

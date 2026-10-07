use efr_render::{ColourMode, RenderOptions, WidthMethod};
use jiff::SignedDuration;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{Call, Outcome, last_lines, notes, output_lines, tail};
use crate::format;
use crate::testing::{call, now, readable};

fn shell(command: &str) -> Call {
    Call::new(call(), format::call_text("shell", &json!({ "command": command })), Some(now()))
}

fn at(millis: i64) -> jiff::Timestamp {
    now() + SignedDuration::from_millis(millis)
}

#[test]
fn a_call_under_a_second_shows_no_time() {
    let options = RenderOptions::new(80);
    let call = shell("make");
    assert_eq!(readable(&call.result(Some(at(400)), Outcome::Ran, &options)), "  \\e[32m✓\\e[0m\n");
    assert_eq!(
        readable(&call.result(Some(at(6_200)), Outcome::Ran, &options)),
        "  \\e[32m✓ 6.2s\\e[0m\n"
    );
    // Without the daemon's times, no time shows.
    assert_eq!(readable(&call.result(None, Outcome::Ran, &options)), "  \\e[32m✓\\e[0m\n");
}

#[test]
fn a_failure_names_its_exit_code_in_the_error_role() {
    let options = RenderOptions::new(80);
    let call = shell("cargo test -p app");
    let exited = Outcome::Exited(101);
    assert_eq!(
        readable(&call.result(Some(at(6_200)), exited, &options)),
        "  \\e[31m✗ exit 101 · 6.2s\\e[0m\n"
    );
    // A call in the sandbox says nothing more: the end of the turn says where it ran.
    assert_eq!(
        readable(&call.result(Some(at(400)), Outcome::Exited(2), &options)),
        "  \\e[31m✗ exit 2\\e[0m\n"
    );
    let failed = Outcome::Failed;
    assert_eq!(readable(&call.result(None, failed, &options)), "  \\e[31m✗ failed\\e[0m\n");
    assert!(exited.failed() && failed.failed() && !Outcome::Ran.failed());
}

#[test]
fn a_refusal_says_why_on_a_row_of_its_own() {
    let options = RenderOptions::new(80);
    let call = shell("rm -rf build");
    let refused = Outcome::Refused("efr's config (floor)");
    assert_eq!(
        readable(&call.result(Some(at(3_000)), refused, &options)),
        "  \\e[31m✗ refused: efr's config (floor)\\e[0m\n"
    );
    assert!(!refused.failed());
    let not_started = Outcome::NotStarted("bwrap: no user namespaces");
    assert_eq!(
        readable(&call.result(Some(at(3_000)), not_started, &options)),
        "  \\e[31m✗ the sandbox could not start: bwrap: no user namespaces; efr checks it again\\e[0m\n"
    );
}

#[test]
fn the_rows_are_plain_when_the_output_is_not_a_terminal() {
    let options = RenderOptions::new(80).with_terminal(false);
    let call = shell("make\nmake install");
    let exited = Outcome::Exited(2);
    assert_eq!(call.header(&options), "· $ make\n    make install\n");
    assert_eq!(call.result(Some(at(1_500)), exited, &options), "  ✗ exit 2 · 1.5s\n");
    assert_eq!(
        notes(&["network: blocked a:443".to_owned()], &options),
        "  network: blocked a:443\n"
    );
}

#[test]
fn a_long_command_goes_on_in_the_next_row_and_is_never_cut() {
    let long = format!("cargo test {} --no-fail-fast", "x".repeat(100));
    let call = shell(&long);
    for columns in [40_u16, 80] {
        let options = RenderOptions::new(columns).with_colour(ColourMode::None);
        let header = call.header(&options);
        let plain: String =
            header.replace("\x1b[1m", "").replace("\x1b[2m", "").replace("\x1b[0m", "");
        for row in plain.lines() {
            let width = efr_render::display_width(row, WidthMethod::CodePoint);
            assert!(width <= usize::from(columns), "{}", readable(&header));
        }
        let joined: String = plain
            .lines()
            .map(|row| {
                // NOTE: the leading spaces are the indent of a row that goes on; a cut
                // inside a word adds none to the command.
                let row = row.trim_start_matches("· $ ").trim_start();
                let row = row.strip_suffix('\\').unwrap_or(row);
                row.strip_suffix('\u{21a9}').unwrap_or(row).to_owned()
            })
            .collect();
        assert_eq!(joined, long);
        assert!(!header.contains('\u{2026}'));
        // The live row is cut with a mark, and keeps its time.
        let mut running = shell(&long);
        running.show(now());
        let row = running.running('⠋', at(12_000), &RenderOptions::new(columns));
        let width = efr_render::display_width(row.trim_end(), WidthMethod::CodePoint);
        assert_eq!(width, usize::from(columns), "{}", readable(&row));
        assert!(row.contains('\u{2026}') && row.ends_with("12s\u{1b}[0m\n"), "{}", readable(&row));
    }
    insta::assert_snapshot!(
        [40_u16, 80].map(|columns| readable(&call.header(&RenderOptions::new(columns)))).join("")
    );
}

#[test]
fn the_running_rows_show_the_time_from_one_second_on_and_a_few_lines() {
    let options = RenderOptions::new(80);
    let mut call = shell("cargo build");
    call.show(at(0));
    assert_eq!(
        readable(&call.running('⠹', at(900), &options)),
        "\\e[33m\u{2839}\\e[0m \\e[36m$ cargo build\\e[0m\n"
    );
    assert_eq!(
        readable(&call.running('⠹', at(12_300), &options)),
        "\\e[33m\u{2839}\\e[0m \\e[36m$ cargo build\\e[0m  \\e[2m12s\\e[0m\n"
    );
    // An approval answered later starts the time again.
    call.approved(Some(at(20_000)));
    call.show(at(20_000));
    assert!(!call.running('⠹', at(20_500), &options).contains("s\u{1b}[0m\n"));
    let several = shell("cd src\nls\nmake\nmake test\nmake install");
    let rows = several.running('⠹', at(0), &options.with_colour(ColourMode::None));
    assert_eq!(
        readable(&rows),
        "\\e[1m\u{2839}\\e[0m $ cd src\n    ls\n    make\n    \\e[2m(2 more lines)\\e[0m\n"
    );
}

#[test]
fn the_tail_is_the_last_three_lines_with_text_after_a_bar() {
    let lines = last_lines(
        "   Compiling app\n\n     Running tests\n test a ... ok\ntest b ... FAILED\n  \n",
    );
    assert_eq!(lines, ["Running tests", "test a ... ok", "test b ... FAILED"]);
    let options = RenderOptions::new(20);
    insta::assert_snapshot!(readable(&tail(&lines, false, &options)));
    // The prompt of a question stands out.
    let asked = tail(&["Password:".to_owned()], true, &options);
    assert_eq!(readable(&asked), "\\e[2m  \u{2502} \\e[0m\\e[1;33mPassword:\\e[0m\n");
    assert_eq!(tail(&lines, false, &options.with_terminal(false)).lines().count(), 3);
}

#[test]
fn the_output_for_the_model_loses_the_notes_of_efr_at_its_end() {
    let output =
        "error: no rule to make target\nmake: *** [all] Error 2\n[exit code 2, cwd /home/u/p]";
    assert_eq!(output_lines(output), ["error: no rule to make target", "make: *** [all] Error 2"]);
    assert_eq!(output_lines("[exit code 1, cwd /]"), Vec::<String>::new());
    assert_eq!(last_lines("a\u{1b}[2Jb"), ["a\u{241b}[2Jb"]);
}

#[test]
fn a_note_of_one_long_url_starts_on_its_first_row() {
    let url = format!("https://registry.example.com/{}", "a".repeat(40));
    let options = RenderOptions::new(30).with_colour(ColourMode::None);
    let shown =
        notes(std::slice::from_ref(&url), &options).replace("\x1b[2m", "").replace("\x1b[0m", "");
    let rows: Vec<&str> = shown.lines().collect();
    // No row is only its indent, and the rows give the URL back.
    assert!(rows.iter().all(|row| !row.trim().is_empty()), "{shown:?}");
    assert!(rows[0].starts_with("  https://"), "{shown:?}");
    assert!(rows.iter().all(|row| row.len() <= 30), "{shown:?}");
    let joined: String = rows.iter().map(|row| row.trim_start()).collect();
    assert_eq!(joined, url);
}

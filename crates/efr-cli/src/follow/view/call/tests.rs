use efr_render::{ColourMode, RenderOptions, WidthMethod};
use jiff::SignedDuration;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{Call, Outcome, last_lines, output_lines, tail};
use crate::format;
use crate::testing::{call, now, readable};

fn shell(command: &str) -> Call {
    Call::new(call(), format::call_line("shell", &json!({ "command": command })), Some(now()))
}

fn at(millis: i64) -> jiff::Timestamp {
    now() + SignedDuration::from_millis(millis)
}

#[test]
fn a_call_under_a_second_shows_no_time() {
    let options = RenderOptions::new(80);
    let call = shell("make");
    assert_eq!(
        readable(&call.ended(Some(at(400)), Outcome::Ran, &options)),
        "\\e[2m$ make\\e[0m\n"
    );
    assert_eq!(
        readable(&call.ended(Some(at(6_200)), Outcome::Ran, &options)),
        "\\e[2m$ make\\e[0m  \\e[2m6.2s\\e[0m\n"
    );
    // Without the daemon's times, no time shows.
    assert_eq!(readable(&call.ended(None, Outcome::Ran, &options)), "\\e[2m$ make\\e[0m\n");
}

#[test]
fn a_failure_names_its_exit_code_in_the_error_role() {
    let options = RenderOptions::new(80);
    let call = shell("cargo test -p app");
    let exited = Outcome::Exited { code: 101, contained: false };
    assert_eq!(
        readable(&call.ended(Some(at(6_200)), exited, &options)),
        "\\e[2m$ cargo test -p app\\e[0m  \\e[31mexit 101\\e[0m  \\e[2m6.2s\\e[0m\n"
    );
    let sandboxed = Outcome::Exited { code: 2, contained: true };
    assert_eq!(
        readable(&call.ended(Some(at(400)), sandboxed, &options)),
        "\\e[2m$ cargo test -p app\\e[0m  \\e[31mexit 2\\e[0m\\e[2m (sandbox)\\e[0m\n"
    );
    let failed = Outcome::Failed { contained: false };
    assert_eq!(
        readable(&call.ended(None, failed, &options)),
        "\\e[2m$ cargo test -p app\\e[0m  \\e[31mfailed\\e[0m\n"
    );
    assert!(exited.failed() && failed.failed() && !Outcome::Ran.failed());
}

#[test]
fn a_refusal_says_why_in_the_warning_role() {
    let options = RenderOptions::new(80);
    let call = shell("rm -rf build");
    let refused = Outcome::Refused("efr's config (floor)");
    assert_eq!(
        readable(&call.ended(Some(at(3_000)), refused, &options)),
        "\\e[2m$ rm -rf build\\e[0m  \\e[1;33mrefused: efr's config (floor)\\e[0m\n"
    );
    assert!(!refused.failed());
}

#[test]
fn the_lines_are_plain_when_the_output_has_no_colour() {
    let options = RenderOptions::new(80).with_colour(ColourMode::None);
    let call = shell("make");
    let exited = Outcome::Exited { code: 2, contained: false };
    assert_eq!(
        readable(&call.ended(Some(at(1_500)), exited, &options)),
        "\\e[2m$ make\\e[0m  \\e[1mexit 2\\e[0m  \\e[2m1.5s\\e[0m\n"
    );
}

#[test]
fn a_call_line_is_cut_to_the_width_and_keeps_its_end() {
    let long = format!("cargo test {}", "x".repeat(100));
    let call = shell(&long);
    let exited = Outcome::Exited { code: 101, contained: false };
    for columns in [40_u16, 80] {
        let options = RenderOptions::new(columns);
        let line = call.ended(Some(at(6_200)), exited, &options);
        let width = efr_render::display_width(line.trim_end(), WidthMethod::CodePoint);
        assert_eq!(width, usize::from(columns), "{}", readable(&line));
        assert!(line.contains("exit 101") && line.contains("6.2s"), "{}", readable(&line));
        assert!(line.contains('\u{2026}'));
        let mut running = shell(&long);
        running.show(now());
        let row = running.running('⠋', at(12_000), &options);
        let width = efr_render::display_width(row.trim_end(), WidthMethod::CodePoint);
        assert_eq!(width, usize::from(columns), "{}", readable(&row));
        assert!(row.ends_with("12s\u{1b}[0m\n"), "{}", readable(&row));
    }
    insta::assert_snapshot!(
        [40_u16, 80]
            .map(|columns| {
                let options = RenderOptions::new(columns);
                readable(&call.ended(Some(at(6_200)), exited, &options))
            })
            .join("")
    );
}

#[test]
fn the_running_line_shows_its_time_from_one_second_on() {
    let options = RenderOptions::new(80);
    let mut call = shell("cargo build");
    call.show(at(0));
    assert_eq!(
        readable(&call.running('⠹', at(900), &options)),
        "\\e[33m\u{2839}\\e[0m \\e[2m$ cargo build\\e[0m\n"
    );
    assert_eq!(
        readable(&call.running('⠹', at(12_300), &options)),
        "\\e[33m\u{2839}\\e[0m \\e[2m$ cargo build\\e[0m  \\e[2m12s\\e[0m\n"
    );
    // An approval answered later starts the time again.
    call.approved(Some(at(20_000)));
    call.show(at(20_000));
    assert!(!call.running('⠹', at(20_500), &options).contains("s\u{1b}[0m\n"));
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

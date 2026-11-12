use efr_protocol::{BlockReason, Blocked, JobId};
use efr_render::{Colour, ColourMode, Glyphs, Palette, RenderOptions, Role, WidthMethod};
use jiff::SignedDuration;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{Call, Outcome, blocked_rows, last_lines, note, notes, output_lines, tail};
use crate::format;
use crate::testing::{bare, call, now, readable};

fn shell(command: &str) -> Call {
    Call::new(call(), format::call_text("shell", &json!({ "command": command })), Some(now()))
}

fn at(millis: i64) -> jiff::Timestamp {
    now() + SignedDuration::from_millis(millis)
}

/// The row of `call` that ended at `millis` as `outcome`, plain.
fn row(call: &Call, millis: Option<i64>, outcome: Outcome<'_>, columns: u16) -> String {
    let options = RenderOptions::new(columns).with_colour(ColourMode::None);
    bare(&call.row(millis.map(at), outcome, None, &options))
}

#[test]
fn a_call_that_went_well_is_one_muted_row_with_its_time() {
    let options = RenderOptions::new(80);
    let call = shell("make");
    assert_eq!(
        readable(&call.row(Some(at(6_200)), Outcome::Ran, None, &options)),
        "\\e[32m✓\\e[0m \\e[2m$\\e[0m \\e[2mmake\\e[0m  \\e[2m6.2s\\e[0m\n"
    );
    // A call of a few milliseconds says nothing of its time; from a tenth of a second
    // on it does.
    assert_eq!(row(&call, Some(40), Outcome::Ran, 80), "✓ $ make\n");
    assert_eq!(row(&call, Some(400), Outcome::Ran, 80), "✓ $ make  0.4s\n");
    // Without the daemon's times, no time shows.
    assert_eq!(row(&call, None, Outcome::Ran, 80), "✓ $ make\n");
    // A call that starts a job says so after its time.
    let background = shell("npm run dev").in_background();
    assert_eq!(
        row(&background, Some(400), Outcome::Ran, 80),
        "✓ $ npm run dev  0.4s  (background)\n"
    );
}

#[test]
fn a_failure_is_loud_and_always_says_its_time() {
    let options = RenderOptions::new(80);
    let call = shell("cargo test -p app");
    let exited = Outcome::Exited(101);
    assert_eq!(
        readable(&call.row(Some(at(6_200)), exited, None, &options)),
        "\\e[1;31m✗\\e[0m \\e[2m$\\e[0m cargo test -p app  \\e[31mexit 101\\e[0m \\e[2m· 6.2s\\e[0m\n"
    );
    assert_eq!(
        row(&call, Some(40), Outcome::Exited(2), 80),
        "✗ $ cargo test -p app  exit 2 · 0.0s\n"
    );
    assert_eq!(row(&call, None, Outcome::Failed, 80), "✗ $ cargo test -p app  failed\n");
    assert!(exited.failed() && Outcome::Failed.failed() && !Outcome::Ran.failed());
}

#[test]
fn a_refusal_says_why_after_its_word() {
    let options = RenderOptions::new(100);
    let call = shell("curl -sI https://example.com");
    let refused = Outcome::Refused("no network in the sandbox");
    assert_eq!(
        readable(&call.row(Some(at(3_000)), refused, None, &options)),
        "\\e[33m!\\e[0m \\e[2m$\\e[0m curl -sI https://example.com  \\e[33mrefused\\e[0m \\e[2m· no network in the sandbox\\e[0m\n"
    );
    assert!(!refused.failed());
    let not_started = Outcome::NotStarted("bwrap: no user namespaces");
    assert_eq!(
        row(&call, Some(3_000), not_started, 120),
        "! $ curl -sI https://example.com  refused · the sandbox could not start: bwrap: no user namespaces; efr checks it again\n"
    );
}

#[test]
fn a_denied_call_and_a_moved_one_are_muted() {
    let call = shell("cargo update -p rusqlite");
    assert_eq!(
        row(&call, Some(3_000), Outcome::Denied, 80),
        "\u{b7} $ cargo update -p rusqlite  did not run\n"
    );
    let job: JobId = "j4".parse().unwrap();
    assert_eq!(
        row(&shell("cargo test --workspace"), Some(42_000), Outcome::Moved(&job), 80),
        "\u{2192} $ cargo test --workspace  moved to job j4 after 42s\n"
    );
    assert_eq!(
        row(&call, Some(1_000), Outcome::LeftRunning, 80),
        "\u{2192} $ cargo update -p rusqlite  still runs in the hidden shell\n"
    );
}

#[test]
fn the_rows_take_the_ascii_glyphs_outside_a_utf8_locale() {
    let options = RenderOptions::new(80).with_colour(ColourMode::None).with_glyphs(&Glyphs::ASCII);
    let call = shell("make");
    let rows: Vec<String> =
        [Outcome::Ran, Outcome::Exited(2), Outcome::Refused("no"), Outcome::Denied]
            .into_iter()
            .map(|outcome| bare(&call.row(Some(at(1_500)), outcome, None, &options)))
            .collect();
    assert_eq!(
        rows,
        [
            "+ $ make  1.5s\n",
            "x $ make  exit 2 - 1.5s\n",
            "! $ make  refused - no\n",
            "- $ make  did not run\n"
        ]
    );
}

#[test]
fn the_rows_are_plain_when_the_output_is_not_a_terminal() {
    let options = RenderOptions::new(80).with_terminal(false);
    let call = shell("make\nmake install");
    let exited = Outcome::Exited(2);
    assert_eq!(
        call.row(Some(at(1_500)), exited, None, &options),
        "\u{2717} $ make\n    make install  exit 2 \u{b7} 1.5s\n"
    );
    assert_eq!(
        notes(&[note("network: blocked a:443".to_owned())], &options),
        "  network: blocked a:443\n"
    );
}

#[test]
fn a_long_command_goes_on_four_columns_after_its_name_and_is_never_cut() {
    let long = format!("cargo test {} --no-fail-fast", "x".repeat(100));
    let call = shell(&long);
    for columns in [40_u16, 80] {
        let shown = row(&call, Some(4_100), Outcome::Ran, columns);
        for line in shown.lines() {
            let width = efr_render::display_width(line, WidthMethod::CodePoint);
            assert!(width <= usize::from(columns), "{shown}");
        }
        let joined: String = shown
            .lines()
            .map(|line| {
                // NOTE: the leading spaces are the indent of a row that goes on; a cut
                // inside a word adds none to the command.
                let line = line.trim_start_matches("\u{2713} $ ").trim_start();
                let line = line.strip_suffix("  4.1s").unwrap_or(line);
                let line = line.strip_suffix('\\').unwrap_or(line);
                line.strip_suffix('\u{21a9}').unwrap_or(line).to_owned()
            })
            .collect();
        assert_eq!(joined, long);
        assert!(shown.ends_with("  4.1s\n"), "{shown}");
        assert!(!shown.contains('\u{2026}'));
        // The live row is cut with a mark, and keeps its time.
        let mut running = shell(&long);
        running.show(now());
        let live = running.running('⠋', at(12_000), false, None, &RenderOptions::new(columns));
        let width = efr_render::display_width(live.trim_end(), WidthMethod::CodePoint);
        assert_eq!(width, usize::from(columns), "{}", readable(&live));
        assert!(
            live.contains('\u{2026}') && live.ends_with("12s\u{1b}[0m\n"),
            "{}",
            readable(&live)
        );
    }
    insta::assert_snapshot!(
        [40_u16, 80].map(|columns| row(&call, Some(4_100), Outcome::Ran, columns)).join("")
    );
}

#[test]
fn a_row_that_has_no_room_for_its_end_cuts_the_command_shorter() {
    // TM-Calls 16 at 60 columns: the cut falls after `--dry-run`, and the time follows
    // the last row.
    let call =
        shell("cargo run -p efr-store --bin migrate -- --dry-run ~/.local/share/efr/sessions");
    assert_eq!(
        row(&call, Some(4_100), Outcome::Ran, 60),
        "\u{2713} $ cargo run -p efr-store --bin migrate -- --dry-run \\\n      ~/.local/share/efr/sessions  4.1s\n"
    );
    // A reason too long for any row goes on, 4 columns after the name.
    let refused = Outcome::Refused("a command that must run outside the sandbox must run alone");
    let shown = row(&shell("cd ~/proj && find . -type f | sort"), None, refused, 40);
    for line in shown.lines() {
        assert!(efr_render::display_width(line, WidthMethod::CodePoint) <= 40, "{shown}");
    }
    assert!(shown.starts_with("! $ cd ~/proj && find . -type f | sort"), "{shown}");
    assert!(shown.contains("\n      "), "{shown}");
}

#[test]
fn the_running_rows_show_the_time_from_one_second_on_and_a_few_lines() {
    let options = RenderOptions::new(80);
    let mut call = shell("cargo build");
    call.show(at(0));
    assert_eq!(
        readable(&call.running('⠹', at(900), false, None, &options)),
        "\\e[33m\u{2839}\\e[0m \\e[2m$\\e[0m cargo build\n"
    );
    assert_eq!(
        readable(&call.running('⠹', at(12_300), false, None, &options)),
        "\\e[33m\u{2839}\\e[0m \\e[2m$\\e[0m cargo build  \\e[2m12s\\e[0m\n"
    );
    // An approval answered later starts the time again.
    call.approved(Some(at(20_000)));
    call.show(at(20_000));
    assert!(!call.running('⠹', at(20_500), false, None, &options).contains("s\u{1b}[0m\n"));
    let several = shell("cd src\nls\nmake\nmake test\nmake install");
    let rows = several.running('⠹', at(0), false, None, &options.with_colour(ColourMode::None));
    assert_eq!(
        readable(&rows),
        "\\e[1m\u{2839}\\e[0m \\e[2m$\\e[0m cd src\n    ls\n    make\n    \\e[2m(2 more lines)\\e[0m\n"
    );
}

#[test]
fn the_running_row_of_a_full_turn_shows_full_in_the_error_role() {
    let palette = Palette::new().with(Role::Error, Colour::Rgb(0xe0, 0x6c, 0x75));
    let options = RenderOptions::new(80).with_colour(ColourMode::TrueColor).with_palette(palette);
    let mut call = shell("cargo build");
    call.show(at(0));

    let row = call.running('⠹', at(12_300), true, None, &options);

    let full = "   \x1b[38;2;224;108;117mfull\x1b[0m\n";
    assert!(row.ends_with(full), "{}", readable(&row));
    assert!(readable(&row).contains("12s"), "{}", readable(&row));
    // The tag counts in the width: a long line is cut, and the row keeps the tag.
    let long = shell(&"x".repeat(200));
    let narrow = RenderOptions::new(40).with_colour(ColourMode::None);
    let row = long.running('⠹', at(0), true, None, &narrow);
    let width = efr_render::display_width(row.trim_end(), WidthMethod::CodePoint);
    assert_eq!(width, 40, "{row:?}");
    assert!(bare(&row).ends_with("\u{2026}   full\n"), "{row:?}");
    assert!(!call.running('⠹', at(0), false, None, &narrow).contains("full"));
}

#[test]
fn the_tail_is_the_last_three_lines_with_text_four_columns_in() {
    let lines = last_lines(
        "   Compiling app\n\n     Running tests\n test a ... ok\ntest b ... FAILED\n  \n",
    );
    assert_eq!(lines, ["     Running tests", " test a ... ok", "test b ... FAILED"]);
    let options = RenderOptions::new(20);
    insta::assert_snapshot!(readable(&tail(&lines, &options)));
    assert_eq!(tail(&lines, &options.clone().with_terminal(false)).lines().count(), 3);
    // A failed call keeps the spaces at the start of its lines, in the text role.
    let failed = super::failed_tail(&["   --> src/a.rs:9:9".to_owned()], &options);
    assert_eq!(readable(&failed), "       --> src/a.rs\u{2026}\n");
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
fn a_blocked_connection_names_its_host_in_the_warning_role() {
    let options = RenderOptions::new(100);
    let blocked = Blocked {
        host: "registry.npmjs.org".into(),
        port: 443,
        reason: BlockReason::NotAllowed,
        count: 3,
    };
    let shown = notes(&blocked_rows(&[blocked], 2, &options), &options);
    assert_eq!(
        readable(&shown),
        "  \\e[2mnetwork: blocked \\e[0m\\e[33mregistry.npmjs.org:443\\e[0m\\e[2m (not on the allow list) · 3 times\\e[0m\n  \\e[2mnetwork: and 2 more refused\\e[0m\n"
    );
}

#[test]
fn a_note_of_one_long_url_starts_on_its_first_row() {
    let url = format!("https://registry.example.com/{}", "a".repeat(40));
    let options = RenderOptions::new(30).with_colour(ColourMode::None);
    let shown = notes(&[note(url.clone())], &options).replace("\x1b[2m", "").replace("\x1b[0m", "");
    let rows: Vec<&str> = shown.lines().collect();
    // No row is only its indent, and the rows give the URL back.
    assert!(rows.iter().all(|row| !row.trim().is_empty()), "{shown:?}");
    assert!(rows[0].starts_with("  https://"), "{shown:?}");
    assert!(rows.iter().all(|row| row.len() <= 30), "{shown:?}");
    let joined: String = rows.iter().map(|row| row.trim_start()).collect();
    assert_eq!(joined, url);
}

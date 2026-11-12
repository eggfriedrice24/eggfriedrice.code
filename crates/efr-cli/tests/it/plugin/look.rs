//! The look of the plugin against the frames of the design: the prompt corner, home,
//! sticky mode, the lines of `,mode` and `,model` and the notices at the next prompt
//! (`TM-Scope`, `TM-Legend` 1-3, `TM-Endings`).
//!
//! Each test starts `zsh -i` on a pseudo-terminal of the frame's width, with its own
//! `ZDOTDIR` and a `.zshrc` that sets a prompt like the board's theme, sources the
//! plugin and calls `efr_corner`. It types keys, then replays what the terminal got on
//! a vt100 screen and compares rows of the screen with rows of the frame, cell by cell:
//! the text, the colour and the attributes ([`efr_test_support::design`]). The runtime
//! root is the test's own, the `efr` is the fake of [`super::FAKE_EFR`], and no efrd
//! runs: the tests write `projects.tsv`, `prompt.tsv` and the notices as efrd does.
//!
//! The theme's own parts of a row (the directory, the branch, `❯`, `42s`) come from the
//! test's prompt, in the colours of the board, so the whole row compares.

use std::fmt::Write as _;
use std::path::PathBuf;

use efr_test_support::design::{Depth, Frame, Ink, Look, assert_rows, expect};

use super::{Home, plugin, zsh_tests_enabled};

/// The palette of the design for the roles that the plugin paints, as efrd writes it
/// to `prompt.tsv` from a theme file (`TM-Legend`, the roles).
const DESIGN_PROMPT_FILE: &str = "color.text\t#d8d3c3\ncolor.muted\t#8695b7\n\
     color.accent\t#ffc940\ncolor.warning\t#ffc940\ncolor.error\t#e06c75\n";

/// The theme's part of row `row` of `frame` in `depth` (see `design::expect`) as a zsh
/// prompt: the row up to the prompt character and its blank (`❯ ` or `% `), each piece
/// in its look, so the test's prompt draws the board's theme in every depth.
fn theme_of(frame: &Frame, row: usize, depth: Depth) -> String {
    let frame = undimmed(frame, depth);
    let line = &frame.rows[row];
    let end = ["\u{276f} ", "% "]
        .iter()
        .filter_map(|mark| line.text.find(mark).map(|at| at + mark.len()))
        .min()
        .unwrap();
    pieces(&frame, row, 0..end)
}

/// The theme's own right side of row `row` of `frame` in `depth`, as a zsh prompt: the
/// pieces between the typed text and the two blanks before the corner (`app · ` or
/// `⌂ · `), such as `42s  ✗ 1`.
fn right_of(frame: &Frame, row: usize, depth: Depth) -> String {
    let frame = expect(frame, depth).unwrap();
    let text = &frame.rows[row].text;
    let corner = ["app \u{b7} ", "\u{2302} \u{b7} "]
        .iter()
        .filter_map(|mark| text.rfind(mark))
        .max()
        .unwrap();
    let end = corner - 2;
    let start = text[..end].rfind("   ").unwrap() + 3;
    pieces(&frame, row, start..end)
}

/// The bytes `range` of row `row` of `frame` as pieces of a zsh prompt, each in its look.
fn pieces(frame: &Frame, row: usize, range: std::ops::Range<usize>) -> String {
    let mut prompt = String::new();
    let mut at = 0;
    for segment in &frame.rows[row].segments {
        let (from, to) = (at, at + segment.text.len());
        at = to;
        let (start, end) = (from.max(range.start), to.min(range.end));
        if start >= end {
            continue;
        }
        let text = &segment.text[start - from..end - from];
        prompt.push_str(&prompt_piece(text, segment.look, frame.foreground));
    }
    prompt
}

/// `text` in `look` as a piece of a zsh prompt.
fn prompt_piece(text: &str, look: Look, foreground: Ink) -> String {
    let mut open = String::new();
    let mut close = String::new();
    match look.fg {
        Ink::Rgb(r, g, b) if look.fg != foreground => {
            open.push_str(&format!("%F{{#{r:02x}{g:02x}{b:02x}}}"));
            close.insert_str(0, "%f");
        }
        Ink::Slot(slot) => {
            open.push_str(&format!("%F{{{slot}}}"));
            close.insert_str(0, "%f");
        }
        _ => {}
    }
    if look.bold {
        open.push_str("%B");
        close.insert_str(0, "%b");
    }
    if look.dim {
        open.push_str("%{\x1b[2m%}");
        close.insert_str(0, "%{\x1b[22m%}");
    }
    format!("{open}{}{close}", text.replace('%', "%%"))
}

/// One terminal of a test: its home, the frame's width and depth, and its `.zshrc`.
struct Term {
    home: Home,
    cols: u16,
    depth: Depth,
    /// Lines of `.zshrc` before the plugin is sourced.
    before: Vec<String>,
    /// Lines of `.zshrc` after the plugin is sourced, such as `efr_corner`.
    after: Vec<String>,
    /// The environment of the terminal.
    env: Vec<(String, String)>,
}

/// What a terminal showed after each step of [`Term::run`].
struct Shown {
    cols: u16,
    /// The bytes that the terminal got up to the end of each step.
    steps: Vec<Vec<u8>>,
}

/// A step of [`Term::run`].
enum Step<'a> {
    /// Types the text and Enter, and waits for the next prompt.
    Line(&'a str),
    /// Types the text and waits until the terminal shows its end.
    Type(&'a str),
}

impl Term {
    /// A terminal in `app`, a registered project at `~/p/app`, with the board's theme
    /// prompt and the corner, in the colours of `depth`.
    fn new(cols: u16, depth: Depth) -> Term {
        let home = Home::new();
        let mut term = Term {
            home,
            cols,
            depth,
            before: Vec::new(),
            after: Vec::new(),
            env: vec![("LANG".to_owned(), "C.UTF-8".to_owned())],
        };
        match depth {
            Depth::TrueColor => term.env.push(("COLORTERM".to_owned(), "truecolor".to_owned())),
            Depth::Ansi16 => {}
            Depth::NoColor => term.env.push(("NO_COLOR".to_owned(), "1".to_owned())),
        }
        term.after("efr_corner");
        term.projects(&[("p/app", "app")]);
        let runtime = term.runtime();
        let colours = if depth == Depth::TrueColor { DESIGN_PROMPT_FILE } else { "" };
        std::fs::write(runtime.join("prompt.tsv"), format!("mode\tcautious\n{colours}")).unwrap();
        term
    }

    fn runtime(&self) -> PathBuf {
        let runtime = self.home.path().join("efr");
        std::fs::create_dir_all(&runtime).unwrap();
        runtime
    }

    /// Writes the projects file with `lines` of `(root below the home, name)`.
    fn projects(&mut self, lines: &[(&str, &str)]) {
        let real = std::fs::canonicalize(self.home.path()).unwrap();
        let text: String = lines
            .iter()
            .map(|(root, name)| format!("{}\t{name}\n", real.join(root).display()))
            .collect();
        std::fs::write(self.runtime().join("projects.tsv"), text).unwrap();
        for (root, _) in lines {
            std::fs::create_dir_all(real.join(root)).unwrap();
        }
    }

    /// Writes the prompt file: the default `mode` and the colours of the depth.
    fn default_mode(&mut self, mode: &str) {
        let colours = if self.depth == Depth::TrueColor { DESIGN_PROMPT_FILE } else { "" };
        std::fs::write(self.runtime().join("prompt.tsv"), format!("mode\t{mode}\n{colours}"))
            .unwrap();
    }

    /// Sets the theme's `PROMPT`, in single quotes.
    fn prompt(&mut self, prompt: &str) {
        self.before.push(format!("PROMPT={}", super::quoted(prompt)));
    }

    /// Sets the theme's `PROMPT` to the one of row `row` of `frame` (see [`theme_of`]).
    fn theme(&mut self, frame: &Frame, row: usize) {
        self.prompt(&theme_of(frame, row, self.depth));
    }

    fn before(&mut self, line: &str) {
        self.before.push(line.to_owned());
    }

    fn after(&mut self, line: &str) {
        self.after.push(line.to_owned());
    }

    fn env(&mut self, name: &str, value: &str) {
        self.env.push((name.to_owned(), value.to_owned()));
    }

    /// Starts `zsh -i` in `dir` below the home and runs `steps`, then `exit`.
    fn run(&self, dir: &str, steps: &[Step<'_>]) -> Shown {
        let dir = self.home.path().join(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut rc = String::new();
        for line in &self.before {
            let _ = writeln!(rc, "{line}");
        }
        let _ = writeln!(rc, "source {}", plugin().display());
        for line in &self.after {
            let _ = writeln!(rc, "{line}");
        }
        let _ = writeln!(rc, "cd {}", super::quoted(&dir.display().to_string()));
        std::fs::write(self.home.path().join(".zshrc"), rc).unwrap();
        let mut keys = Vec::new();
        for step in steps {
            match step {
                Step::Line(text) => keys.push(format!("L{text}\r")),
                Step::Type(text) => keys.push(format!("T{text}")),
            }
        }
        let output = self
            .home
            .zsh()
            .env("ZDOTDIR", self.home.path())
            .env("EFR_TERMINAL_BG", "dark")
            .env("EFR_COLS", self.cols.to_string())
            .env("EFR_STEPS", keys.join("\u{1e}"))
            .envs(self.env.iter().map(|(name, value)| (name.as_str(), value.as_str())))
            .args(["-f", "-c", SCREEN_DRIVER])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "the driver failed: {stderr}");
        let mut steps = Vec::new();
        let mut seen = Vec::new();
        for part in split(&output.stdout, STEP_MARK.as_bytes()) {
            seen.extend_from_slice(part);
            steps.push(seen.clone());
        }
        Shown { cols: self.cols, steps }
    }
}

/// What the driver prints after each step, between the bytes of the terminal.
const STEP_MARK: &str = "\u{0}efr-step\u{0}";

/// Drives `zsh -i` (it reads `$ZDOTDIR/.zshrc` and no file of the system, such as the
/// `compinit` of Debian's `/etc/zsh/zshrc`) on a pseudo-terminal of `$EFR_COLS`
/// columns from zsh's zpty module, one step of `$EFR_STEPS` after the other: `L<line>`
/// types the line with its Enter and waits for the next prompt, `T<text>` types the
/// text and waits until the terminal shows nothing more for a moment. `<C-Space>` in a step stands for the NUL byte
/// that Ctrl+Space sends. After each step it prints [`STEP_MARK`].
const SCREEN_DRIVER: &str = r#"
zmodload zsh/zpty || exit 90
zmodload zsh/datetime || exit 90
ready=$'*\e\\[\\?2004h*'
# Reads what the terminal shows into $screen: until $1 matches what came since the call,
# or with no $1 until it shows nothing more for 0.3 s; at most 20 s.
read_until() {
  local more got= deadline=$(( EPOCHREALTIME + 20 )) quiet=$(( EPOCHREALTIME + 0.3 ))
  while (( EPOCHREALTIME < deadline )); do
    if zpty -r -t user more; then
      got+=$more
      quiet=$(( EPOCHREALTIME + 0.3 ))
      [[ -n $1 && $got == $~1 ]] && break
    else
      [[ -z $1 ]] && (( EPOCHREALTIME > quiet )) && break
      sleep 0.02
    fi
  done
  screen+=$got
  if (( EPOCHREALTIME >= deadline )); then
    print -u2 -rn -- "the terminal did not show ${1:-a pause}; it showed: ${(q+)screen}"
    exit 95
  fi
}
zpty user "stty cols $EFR_COLS rows 60 2>/dev/null; TERM=xterm-256color exec zsh -o no_global_rcs -i" || exit 91
screen=
read_until $ready
read_until
print -rn -- "$screen"
nul=$'\0'
for step in "${(@ps:\x1e:)EFR_STEPS}"; do
  text=${${step#?}//'<C-Space>'/$nul}
  screen=
  zpty -w -n user "$text"
  if [[ $step == L* ]]; then
    read_until $ready
  fi
  read_until
  print -rn -- "$screen"$'\0efr-step\0'
done
# NOTE: the terminal ends with its zsh, which may still be in sticky mode, where
# `exit` would be a prompt.
zpty -d user
"#;

fn split<'a>(bytes: &'a [u8], mark: &[u8]) -> Vec<&'a [u8]> {
    let mut parts = Vec::new();
    let mut rest = bytes;
    while let Some(at) = rest.windows(mark.len()).position(|window| window == mark) {
        parts.push(&rest[..at]);
        rest = &rest[at + mark.len()..];
    }
    parts
}

impl Shown {
    /// The rows of the screen after step `step` (0 is the first step), as text with SGR
    /// sequences, one line each, without the blanks at their end.
    fn rows(&self, step: usize) -> Vec<String> {
        let mut parser = vt100::Parser::new(60, self.cols, 0);
        parser.process(&self.steps[step]);
        let screen = parser.screen();
        let mut rows = Vec::new();
        for row in 0..60 {
            let mut line = String::new();
            let mut last = 0;
            for col in 0..self.cols {
                let Some(cell) = screen.cell(row, col) else { break };
                let text = cell.contents();
                let _ = write!(line, "\x1b[0m{}", sgr(cell));
                if text.is_empty() {
                    line.push(' ');
                } else {
                    line.push_str(text);
                    last = line.len();
                }
            }
            line.truncate(last);
            line.push_str("\x1b[0m");
            rows.push(line);
        }
        while rows.last().is_some_and(|row| row == "\x1b[0m") {
            rows.pop();
        }
        rows
    }

    /// Row `row` of the screen after step `step`.
    fn row(&self, step: usize, row: usize) -> String {
        self.rows(step).get(row).cloned().unwrap_or_default()
    }
}

/// The SGR parameters of how `cell` looks.
fn sgr(cell: &vt100::Cell) -> String {
    let mut codes: Vec<String> = Vec::new();
    for (on, code) in [
        (cell.bold(), "1"),
        (cell.dim(), "2"),
        (cell.italic(), "3"),
        (cell.underline(), "4"),
        (cell.inverse(), "7"),
    ] {
        if on {
            codes.push(code.to_owned());
        }
    }
    for (colour, base) in [(cell.fgcolor(), 38), (cell.bgcolor(), 48)] {
        match colour {
            vt100::Color::Default => {}
            vt100::Color::Idx(slot) if slot < 8 => {
                codes.push((base - 8 + u16::from(slot)).to_string())
            }
            vt100::Color::Idx(slot) if slot < 16 => {
                codes.push((base + 52 + u16::from(slot)).to_string());
            }
            vt100::Color::Idx(slot) => codes.push(format!("{base};5;{slot}")),
            vt100::Color::Rgb(r, g, b) => codes.push(format!("{base};2;{r};{g};{b}")),
        }
    }
    if codes.is_empty() { String::new() } else { format!("\x1b[{}m", codes.join(";")) }
}

/// `frame` in `depth` (see `design::expect`), with the theme's directory bold only.
///
/// NOTE: the table of `design::expect` makes the theme's cyan directory dim as well as
/// bold without colour, which the board's own NO_COLOR frame (Scope 10) does not draw:
/// the directory is bold only there. A vt100 screen also holds one of bold and dim.
fn undimmed(frame: &Frame, depth: Depth) -> Frame {
    let mut frame = expect(frame, depth).unwrap();
    for row in &mut frame.rows {
        for segment in &mut row.segments {
            segment.look.dim &= !segment.look.bold;
        }
    }
    frame
}

/// Asserts that row `row` of `shown` after step `step` is row `at` of `frame` in `depth`.
/// With `EFR_LOOK_PRINT=1` it also prints both rows on stderr, the real one with its
/// colours, so a person can read them next to each other (`--nocapture`).
fn same(frame: &Frame, at: usize, shown: &Shown, step: usize, row: usize, depth: Depth) {
    let real = shown.row(step, row);
    print_rows(frame, at, &real, depth);
    assert_rows(frame, at..=at, &real, depth);
}

#[expect(clippy::print_stderr, reason = "a person reads the rows next to the frame's")]
fn print_rows(frame: &Frame, at: usize, real: &str, depth: Depth) {
    if std::env::var_os("EFR_LOOK_PRINT").is_none() {
        return;
    }
    let text = frame.rows.get(at).map_or("", |row| row.text.as_str());
    eprintln!(
        "{} {} row {} {depth:?}\n  board: {text}\n  efr:   {real}",
        frame.board,
        frame.number,
        at + 1
    );
}

fn frame(board: &str, number: u32) -> Frame {
    Frame::load(board, number).unwrap()
}

// --- the corner ------------------------------------------------------------------------

/// TM-Scope 1: in a project, each mode, typed and not yet sent.
#[test]
fn e2e_look_the_corner_in_a_project_in_each_mode() {
    if !zsh_tests_enabled() {
        return;
    }
    let scope = frame("Scope", 1);
    for (at, mode) in ["manual", "cautious", "auto", "full"].into_iter().enumerate() {
        let mut term = Term::new(scope.cols, Depth::TrueColor);
        term.theme(&scope, at);
        // NOTE: full mode never comes from the environment; `,mode full` sets this.
        term.after(&format!("_efr_turn_mode={mode}"));
        let shown = term.run("p/app", &[Step::Type(", why does parse_empty fail?")]);
        same(&scope, at, &shown, 0, 0, Depth::TrueColor);
    }
}

/// TM-Scope 2: at home, each mode; home is anywhere outside the registered projects,
/// a git clone too.
#[test]
fn e2e_look_the_corner_at_home() {
    if !zsh_tests_enabled() {
        return;
    }
    let scope = frame("Scope", 2);
    let rows = [
        ("", "cautious", ", is the firewall up?"),
        ("etc/nftables.d", "auto", ", reload the ruleset"),
        ("src/tool", "cautious", ", what does this repo build?"),
        ("", "full", ", clean the pacman cache"),
    ];
    for (at, (dir, mode, typed)) in rows.into_iter().enumerate() {
        let mut term = Term::new(scope.cols, Depth::TrueColor);
        term.after(&format!("_efr_turn_mode={mode}"));
        // NOTE: the theme's text is the board's: /etc/nftables.d stands for a
        // directory outside the home, and ~/src/tool for a git clone.
        term.theme(&scope, at);
        let shown = term.run(dir, &[Step::Type(typed)]);
        same(&scope, at, &shown, 0, 0, Depth::TrueColor);
    }
}

/// TM-Scope 3: before efrd ever ran there is no projects file: the mode alone, from the
/// config. Row 2, nothing at all, is the board's other choice, which it did not take.
///
/// NOTE: before efrd ever ran, no prompt file gives the palette either, so the mode is
/// dim as in 16 colours: the truecolor frame cannot show then.
#[test]
fn e2e_look_the_corner_before_efrd_ever_ran() {
    if !zsh_tests_enabled() {
        return;
    }
    for depth in [Depth::Ansi16, Depth::NoColor] {
        let scope = undimmed(&frame("Scope", 3), depth);
        let mut term = Term::new(scope.cols, depth);
        term.theme(&scope, 0);
        std::fs::remove_file(term.runtime().join("projects.tsv")).unwrap();
        std::fs::remove_file(term.runtime().join("prompt.tsv")).unwrap();
        let config = term.home.path().join(".config/efr");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("config.toml"), "[permissions]\nmode = \"auto\"\n").unwrap();
        let shown = term.run("p/app", &[Step::Type("")]);
        same(&scope, 0, &shown, 0, 0, depth);
    }
}

/// TM-Scope 4: next to the theme's own right side (starship's duration, exit status and
/// jobs), with two blanks between; the corner stays at the edge.
#[test]
fn e2e_look_the_corner_after_the_themes_right_side() {
    if !zsh_tests_enabled() {
        return;
    }
    let scope = frame("Scope", 4);
    for at in 0..scope.rows.len() {
        let mut term = Term::new(scope.cols, Depth::TrueColor);
        term.theme(&scope, at);
        let home = scope.rows[at].text.starts_with("~ ");
        if home {
            term.default_mode("cautious");
        } else {
            term.default_mode("auto");
        }
        if at > 0 {
            term.before(&format!(
                "RPROMPT={}",
                super::quoted(&right_of(&scope, at, Depth::TrueColor))
            ));
        }
        let shown = term.run(if home { "" } else { "p/app" }, &[Step::Type("")]);
        same(&scope, at, &shown, 0, 0, Depth::TrueColor);
    }
}

/// TM-Scope 5: a plain zsh prompt. NOTE: the board's caption sets
/// `RPROMPT='$(efr_prompt_info)'`, which forks at each prompt; DECISIONS.md ("Corner
/// plumbing", spec 2.1 item 11) builds the corner with `efr_corner` and no fork, so
/// the rows are drawn by `efr_corner`.
#[test]
fn e2e_look_the_corner_on_a_plain_prompt() {
    if !zsh_tests_enabled() {
        return;
    }
    let scope = frame("Scope", 5);
    let rows = [
        ("p/app", "auto", ", why does parse_empty fail?"),
        ("p/app", "auto", ""),
        ("", "cautious", ""),
    ];
    for (at, (dir, mode, typed)) in rows.into_iter().enumerate() {
        let mut term = Term::new(scope.cols, Depth::TrueColor);
        term.theme(&scope, at);
        term.default_mode(mode);
        let shown = term.run(dir, &[Step::Type(typed)]);
        same(&scope, at, &shown, 0, 0, Depth::TrueColor);
    }
}

/// TM-Scope 6, rows 4 and 5: a two-line prompt with the corner on the input line,
/// where RPROMPT stands. Rows 1 and 2 (the corner on line 1 next to starship's `$fill`)
/// come from `efr_corner export`: see
/// [`e2e_look_the_corner_exports_its_parts_for_line_one`].
#[test]
fn e2e_look_the_corner_on_the_input_line_of_a_two_line_prompt() {
    if !zsh_tests_enabled() {
        return;
    }
    let scope = frame("Scope", 6);
    let mut term = Term::new(scope.cols, Depth::TrueColor);
    let line_one = pieces(&scope, 3, 0..scope.rows[3].text.len());
    let line_two = theme_of(&scope, 4, Depth::TrueColor);
    term.prompt(&format!("{line_one}\n{line_two}"));
    term.default_mode("auto");
    let shown = term.run("p/app", &[Step::Type(", why does parse_empty fail?")]);
    same(&scope, 3, &shown, 0, 0, Depth::TrueColor);
    same(&scope, 4, &shown, 0, 1, Depth::TrueColor);
}

/// TM-Scope 6, rows 1 and 2: `efr_corner export` exports the parts of the corner for a
/// theme that draws them on line 1, and leaves RPROMPT alone.
#[test]
fn e2e_look_the_corner_exports_its_parts_for_line_one() {
    if !zsh_tests_enabled() {
        return;
    }
    let mut term = Term::new(100, Depth::TrueColor);
    term.after("RPROMPT=right; efr_corner export");
    term.default_mode("auto");
    term.after("show() { print -r -- \"[${EFR_CORNER_SCOPE-unset}|${EFR_CORNER_MODE-unset}|${EFR_CORNER_FULL-unset}|$RPROMPT]\" > ~/corner.$1 }");
    term.run(
        "p/app",
        &[
            Step::Line("show 1"),
            Step::Line("_efr_turn_mode=full"),
            Step::Line("show 2"),
            Step::Line("cd ~"),
            Step::Line("show 3"),
        ],
    );
    let read =
        |n: u8| std::fs::read_to_string(term.home.path().join(format!("corner.{n}"))).unwrap();
    assert_eq!(read(1), "[app|auto|unset|right]\n");
    assert_eq!(read(2), "[app|unset|full|right]\n");
    assert_eq!(read(3), "[\u{2302}|unset|full|right]\n");
    // The variables reach a theme's own programs, such as starship.
    let mut env = Term::new(100, Depth::TrueColor);
    env.after("efr_corner export");
    env.default_mode("auto");
    env.run("p/app", &[Step::Line("env | grep ^EFR_CORNER_ > ~/env.txt")]);
    let exported = std::fs::read_to_string(env.home.path().join("env.txt")).unwrap();
    assert_eq!(exported, "EFR_CORNER_SCOPE=app\nEFR_CORNER_MODE=auto\n");
}

/// TM-Scope 7: zsh draws the corner while the typed text leaves room, and drops it once
/// the text reaches it, also in the scrollback.
#[test]
fn e2e_look_a_long_typed_line_hides_the_corner() {
    if !zsh_tests_enabled() {
        return;
    }
    let scope = frame("Scope", 7);
    let mut term = Term::new(scope.cols, Depth::TrueColor);
    term.theme(&scope, 0);
    term.default_mode("auto");
    let shown = term.run(
        "p/app",
        &[
            Step::Type(", the parser accepts an empty header"),
            Step::Line(" and then reads the whole file as one record"),
        ],
    );
    same(&scope, 0, &shown, 0, 0, Depth::TrueColor);
    same(&scope, 2, &shown, 1, 0, Depth::TrueColor);
}

/// TM-Scope 8: the same corner at 60 columns: it hides sooner.
#[test]
fn e2e_look_the_corner_at_sixty_columns() {
    if !zsh_tests_enabled() {
        return;
    }
    let scope = frame("Scope", 8);
    let rows = [
        ("p/app", "auto", ", why does parse_empty fail?"),
        ("", "cautious", ", is the firewall up?"),
        ("p/app", "full", ", clean the cache"),
        ("p/app", "auto", ", again"),
        ("p/app", "auto", ", why does parse_empty fail on CI?"),
    ];
    for (at, (dir, mode, typed)) in rows.into_iter().enumerate() {
        let mut term = Term::new(scope.cols, Depth::TrueColor);
        term.theme(&scope, at);
        term.after(&format!("_efr_turn_mode={mode}"));
        if at == 3 {
            term.before(&format!(
                "RPROMPT={}",
                super::quoted(&right_of(&scope, at, Depth::TrueColor))
            ));
        }
        let shown = term.run(dir, &[Step::Type(typed)]);
        same(&scope, at, &shown, 0, 0, Depth::TrueColor);
    }
}

/// TM-Scope 9 and 10: 16 colours (the mode dim, `full` in slot 1) and `NO_COLOR` (the
/// mode dim, `full` bold).
#[test]
fn e2e_look_the_corner_in_16_colours_and_without_colour() {
    if !zsh_tests_enabled() {
        return;
    }
    for (number, depth) in [(9, Depth::Ansi16), (10, Depth::NoColor)] {
        let scope = frame("Scope", number);
        let rows = [
            ("p/app", "auto", ", why does parse_empty fail?"),
            ("", "cautious", ", is the firewall up?"),
            ("p/app", "full", ", clean the cache"),
            ("p/app", "auto", ""),
        ];
        for (at, (dir, mode, typed)) in rows.into_iter().enumerate() {
            let mut term = Term::new(scope.cols, depth);
            term.theme(&scope, at);
            term.after(&format!("_efr_turn_mode={mode}"));
            if at == 3 {
                term.before(&format!("RPROMPT={}", super::quoted(&right_of(&scope, at, depth))));
            }
            let shown = term.run(dir, &[Step::Type(typed)]);
            same(&scope, at, &shown, 0, 0, depth);
        }
    }
}

/// The corner cuts a project name over 16 columns to 16, with `…` at the cut (the notes
/// of TM-Scope 8), doubles each `%` so the prompt shows it as typed, and shows `⌂` as
/// `~` and `·` as `-` in a locale that is not UTF-8.
#[test]
fn e2e_look_the_corner_cuts_a_long_name_and_has_ascii_forms() {
    if !zsh_tests_enabled() {
        return;
    }
    let mut term = Term::new(60, Depth::NoColor);
    term.projects(&[("p/app", "the-name-of-a-long-project"), ("p/pct", "50%~ off")]);
    term.default_mode("auto");
    term.after("show() { print -rP -- \"[$_efr_corner_text]\" > ~/corner.$1 }");
    term.run("p/app", &[Step::Line("show 1"), Step::Line("cd ~/p/pct"), Step::Line("show 2")]);
    let read =
        |n: u8| std::fs::read_to_string(term.home.path().join(format!("corner.{n}"))).unwrap();
    assert_eq!(read(1), "[the-name-of-a-l\u{2026}\x1b[2m \u{b7} auto\x1b[22m]\n");
    assert_eq!(read(2), "[50%~ off\x1b[2m \u{b7} auto\x1b[22m]\n");

    let mut ascii = Term::new(60, Depth::NoColor);
    ascii.env.retain(|(name, _)| name != "LANG");
    ascii.env("LANG", "C");
    ascii.default_mode("auto");
    ascii.projects(&[("p/app", "the-name-of-a-long-project")]);
    ascii.after("show() { print -rP -- \"[$_efr_corner_text]\" > ~/corner.$1 }");
    ascii.run("p/app", &[Step::Line("show 1"), Step::Line("cd ~"), Step::Line("show 2")]);
    let read =
        |n: u8| std::fs::read_to_string(ascii.home.path().join(format!("corner.{n}"))).unwrap();
    assert_eq!(read(1), "[the-name-of-a...\x1b[2m - auto\x1b[22m]\n");
    assert_eq!(read(2), "[~\x1b[2m - auto\x1b[22m]\n");
}

// --- the Legend --------------------------------------------------------------------------

/// TM-Legend 1 and 2: the corner alone, in a project and at home.
#[test]
fn e2e_look_the_corner_is_the_legend() {
    if !zsh_tests_enabled() {
        return;
    }
    for (number, dir, mode) in [(1, "p/app", "auto"), (2, "", "cautious")] {
        let legend = frame("Legend", number);
        for depth in Depth::ALL {
            let mut term = Term::new(legend.cols, depth);
            term.prompt("");
            term.default_mode(mode);
            let shown = term.run(dir, &[Step::Line("print -rP -- \"$_efr_corner_text\"")]);
            same(&legend, 0, &shown, 0, 1, depth);
        }
    }
}

/// The marker that the boards draw for sticky mode.
///
/// NOTE: the marker by default is the robot, which the user chose over the board on
/// 2026-11-12, and `◆` where a terminal cannot draw it. The board's `›` is a marker
/// that `EFR_STICKY_MARKER` names here, so these tests still compare the whole row
/// with the board.
const BOARD_MARKER: &str = "\u{203a}";

/// TM-Legend 3: the marker of sticky mode, before the typed text.
#[test]
fn e2e_look_the_sticky_marker_is_the_legend() {
    if !zsh_tests_enabled() {
        return;
    }
    let legend = frame("Legend", 3);
    for depth in Depth::ALL {
        let mut term = Term::new(legend.cols, depth);
        term.env("EFR_STICKY_MARKER", BOARD_MARKER);
        term.prompt("");
        // The marker alone: no corner, and no projects file, so no scope in front.
        term.after.clear();
        std::fs::remove_file(term.runtime().join("projects.tsv")).unwrap();
        let shown = term.run("", &[Step::Type("<C-Space>why does parse_empty fail?")]);
        same(&legend, 0, &shown, 0, 0, depth);
    }
}

// --- sticky mode ---------------------------------------------------------------------------

/// TM-Scope 17: the marker of sticky mode. With the corner it carries nothing else (rows
/// 2 to 5); without the corner it carries the scope and `full` (rows 7 to 9); a locale
/// that is not UTF-8 shows `~` and `>` (row 10). Row 3 is the line in the scrollback
/// after Enter: the marker stays, as the alias of `,` that ran. Row 1 of the board is
/// today's look, which the fixture leaves out.
#[test]
fn e2e_look_the_sticky_marker() {
    if !zsh_tests_enabled() {
        return;
    }
    let scope = frame("Scope", 17);
    let rows: [(usize, bool, &str, &str, &str); 7] = [
        (1, true, "p/app", "auto", "why does parse_empty fail?"),
        (3, true, "", "cautious", "is the firewall up?"),
        (4, true, "p/app", "full", "clean the build caches"),
        (6, false, "p/app", "auto", "why does parse_empty fail?"),
        (7, false, "", "cautious", "is the firewall up?"),
        (8, false, "p/app", "full", "clean the build caches"),
        (9, false, "", "cautious", "is the firewall up?"),
    ];
    for (at, corner, dir, mode, typed) in rows {
        let mut term = Term::new(scope.cols, Depth::TrueColor);
        term.theme(&scope, at);
        if !corner {
            term.after.clear();
        }
        if at == 9 {
            // A locale that is not UTF-8: the board draws `>`, and the marker there is
            // `*` by default, for the reason of the NOTE at `BOARD_MARKER`.
            term.env.retain(|(name, _)| name != "LANG");
            term.env("LANG", "C");
            term.env("EFR_STICKY_MARKER", ">");
        } else {
            term.env("EFR_STICKY_MARKER", BOARD_MARKER);
        }
        term.after(&format!("_efr_turn_mode={mode}"));
        let shown = term.run(dir, &[Step::Type(&format!("<C-Space>{typed}"))]);
        same(&scope, at, &shown, 0, 0, Depth::TrueColor);
    }
    // In the scrollback after Enter.
    let mut term = Term::new(scope.cols, Depth::TrueColor);
    term.env("EFR_STICKY_MARKER", BOARD_MARKER);
    term.theme(&scope, 2);
    term.default_mode("auto");
    let shown =
        term.run("p/app", &[Step::Type("<C-Space>"), Step::Line("why does parse_empty fail?")]);
    same(&scope, 2, &shown, 1, 0, Depth::TrueColor);
    let call = &term.home.calls()[0];
    assert_eq!(call.prompt.as_deref(), Some("why does parse_empty fail?"));
}

/// TM-Scope 18: sticky mode without colour: the marker bold, the scope dim.
///
/// NOTE: zle has no dim for the text before the typed line (zsh 5.9 knows bold,
/// underline, standout and colours there), so without the corner the scope shows
/// plain in 16 colours and without colour; row 3 of the board draws it dim.
#[test]
fn e2e_look_the_sticky_marker_without_colour() {
    if !zsh_tests_enabled() {
        return;
    }
    let scope = frame("Scope", 18);
    for (at, corner, mode, typed) in [
        (0, true, "auto", "why does parse_empty fail?"),
        (1, true, "full", "clean the build caches"),
        (2, false, "auto", "why does parse_empty fail?"),
    ] {
        let mut term = Term::new(scope.cols, Depth::NoColor);
        term.env("EFR_STICKY_MARKER", BOARD_MARKER);
        term.theme(&scope, at);
        if !corner {
            term.after.clear();
        }
        term.after(&format!("_efr_turn_mode={mode}"));
        let shown = term.run("p/app", &[Step::Type(&format!("<C-Space>{typed}"))]);
        let mut expected = scope.clone();
        if !corner {
            for segment in &mut expected.rows[at].segments {
                if segment.text == "app" {
                    segment.look.dim = false;
                }
            }
        }
        same(&expected, at, &shown, 0, 0, Depth::NoColor);
    }
}

// --- the lines of the settings and the notices -----------------------------------------------

/// What the fake `efr settings` prints for `,model b/gpt-5.5` and `,mode full`.
const SETTINGS_SAID: &str = "\
mode = auto  # EFR_MODE; choices: manual, cautious, auto, full
model = b/gpt-5.5  # --model; choices: gpt-5.5, b/gpt-5.5
effort = medium  # the default of b/gpt-5.5; choices: low, medium, high
";

/// TM-Endings 5, rows 1 and 2: `,model` says what the next prompt gets, muted.
///
/// NOTE: the board says `for this conversation`. `,model` sets the model of each prompt
/// of this terminal, in every scope, and DECISIONS.md (the medium: "Words are plain and
/// factual") beats the board: the line says `for this terminal`.
#[test]
fn e2e_look_a_model_that_a_terminal_chose() {
    if !zsh_tests_enabled() {
        return;
    }
    let endings = frame("Endings", 5);
    for depth in Depth::ALL {
        let mut expected = undimmed(&endings, depth);
        let said = &mut expected.rows[1].segments[0].text;
        *said = said.replace("for this conversation", "for this terminal");
        let mut term = Term::new(endings.cols, depth);
        term.theme(&endings, 0);
        term.default_mode("auto");
        std::fs::write(term.home.records().with_extension("out.settings"), SETTINGS_SAID).unwrap();
        let shown = term.run(
            "p/app",
            &[
                Step::Line(",model b/gpt-5.5"),
                Step::Type(", review the fix as a second pair of eyes"),
            ],
        );
        let real = shown.rows(1);
        let rows = [real[0].clone(), real[1].clone(), real[2].clone()].join("\n");
        assert_rows(&expected, 0..=2, &rows, depth);
    }
}

/// TM-Endings 8, row 4: after `,mode full` the corner shows `full` in the error colour.
/// The two lines that say what full mode does come from `efr settings` (its tests
/// compare them with rows 2 and 3); the plugin prints no line of its own.
#[test]
fn e2e_look_the_corner_after_mode_full() {
    if !zsh_tests_enabled() {
        return;
    }
    let endings = frame("Endings", 8);
    let mut term = Term::new(endings.cols, Depth::TrueColor);
    term.theme(&endings, 0);
    term.default_mode("auto");
    std::fs::write(
        term.home.records().with_extension("out.settings"),
        SETTINGS_SAID.replace("mode = auto  # EFR_MODE", "mode = full  # --mode"),
    )
    .unwrap();
    let shown =
        term.run("p/app", &[Step::Line(",mode full"), Step::Type(", clean the build caches")]);
    same(&endings, 0, &shown, 1, 0, Depth::TrueColor);
    same(&endings, 3, &shown, 1, 1, Depth::TrueColor);
}

/// The prompt rows of TM-Endings 1 to 9 in the scrollback, after Enter: the corner
/// records the scope and the mode of each prompt, and without the corner (frame 9)
/// nothing stands at the right.
#[test]
fn e2e_look_the_prompt_rows_of_the_endings() {
    if !zsh_tests_enabled() {
        return;
    }
    for number in 1..=9 {
        let endings = frame("Endings", number);
        for (at, row) in endings.rows.iter().enumerate() {
            let Some((_, typed)) = row.text.split_once("\u{276f} ") else {
                continue;
            };
            // The typed text ends where the blanks before the corner start.
            let typed = typed.split("   ").next().unwrap_or_default().trim_end();
            let home = row.text.starts_with("~ ");
            let mut term = Term::new(endings.cols, Depth::TrueColor);
            term.theme(&endings, at);
            let full = row.text.ends_with("app \u{b7} full");
            term.default_mode(if home { "cautious" } else { "auto" });
            if full {
                term.after("_efr_turn_mode=full");
            }
            if !row.text.contains(" \u{b7} ") {
                term.after.retain(|line| line != "efr_corner");
            }
            let shown = term.run(if home { "" } else { "p/app" }, &[Step::Line(typed)]);
            same(&endings, at, &shown, 0, 0, Depth::TrueColor);
        }
    }
}

/// The notice file of the terminal of a test's zsh, as efrd names it, and a function
/// `notice` of `.zshrc` that appends `$1` to it with `print -r`, so the line is newer
/// than the shell.
const NOTICE: &str = r#"notice() { local f=$XDG_RUNTIME_DIR/efr/notices/${${TTY#/dev/}//\//-}; mkdir -p ${f:h}; print -r -- "$1" >> $f }"#;

/// TM-Endings 11 to 13: notices at the next prompt, as efrd writes them
/// (`crates/efr-daemon/src/notices.rs`): a job that ended, muted; a failed exit code
/// in the error colour; a question that waits in the warning colour.
#[test]
fn e2e_look_the_notices_at_the_next_prompt() {
    if !zsh_tests_enabled() {
        return;
    }
    let lines = [
        (11, "efr: job j3 ended after 4m 12s (exit 0) \u{b7} npm run build"),
        (
            12,
            "muted\tefr: job j4 ended after 42s \terror\t(exit 101)\tmuted\t \u{b7} cargo test \
             --workspace \u{b7} efr jobs",
        ),
        (
            13,
            "warning\tefr: a question waits in \"fix steer race\" (desktop \u{b7} /dev/pts/4) \
             \u{b7} ,resume 1",
        ),
    ];
    for (number, line) in lines {
        for depth in Depth::ALL {
            let endings = undimmed(&frame("Endings", number), depth);
            let mut term = Term::new(endings.cols, depth);
            term.theme(&endings, endings.rows.len() - 1);
            term.default_mode("auto");
            term.after(NOTICE);
            if number == 11 {
                term.after("ls() { print -r -- 'Cargo.lock  Cargo.toml  crates  docs  justfile' }");
            }
            term.after(&format!("line={}", super::quoted(line)));
            let shown = if number == 11 {
                term.run("p/app", &[Step::Line("notice $line; ls")])
            } else {
                term.run("p/app", &[Step::Line("notice $line")])
            };
            let real = shown.rows(0);
            let rows = endings.rows.len();
            // The notice and the prompt after it are the last rows of the screen; frame
            // 11 also has the output of `ls` before them.
            let from = 1;
            let shown_rows = real[from..from + rows - usize::from(number == 11)].join("\n");
            let first = usize::from(number == 11);
            assert_rows(&endings, first.., &shown_rows, depth);
        }
    }
}

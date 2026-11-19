//! The paste of an image into a prompt line (OSC 5522, `TM-Attach` frame 4): the plugin
//! turns mode 5522 on only at a prompt line that its gate allows, catches the paste
//! event in a widget and hands it to `efr clip`, here the fake `efr`, whose answer puts
//! a file or text into the line, shows a message or turns the paste off.

use pretty_assertions::assert_eq;

use super::{Call, Home, plugin, zsh_tests_enabled};

/// Drives `zsh -f -i` on a pseudo-terminal from zsh's zpty module, one step of
/// `$EFR_STEPS` (split at `\x1e`) after the other: it types the step and waits until
/// the terminal shows nothing more for `$EFR_QUIET` seconds, then prints [`STEP_MARK`].
/// `<C-Space>` in a step stands for the NUL byte that Ctrl+Space sends.
const DRIVER: &str = r#"
zmodload zsh/zpty || exit 90
zmodload zsh/datetime || exit 90
quiet() {
  local more deadline=$(( EPOCHREALTIME + 20 )) still=$(( EPOCHREALTIME + EFR_QUIET ))
  while (( EPOCHREALTIME < deadline )); do
    if zpty -r -t user more; then
      print -rn -- "$more"
      still=$(( EPOCHREALTIME + EFR_QUIET ))
    elif (( EPOCHREALTIME > still )); then
      return 0
    else
      sleep 0.02
    fi
  done
  exit 95
}
zpty user "TERM=xterm zsh -f -i" || exit 91
quiet
nul=$'\0'
for step in "${(@ps:\x1e:)EFR_STEPS}"; do
  zpty -w -n user "${step//'<C-Space>'/$nul}"
  quiet
  print -rn -- $'\0efr-step\0'
done
zpty -d user
"#;

/// What the driver prints after each step.
const STEP_MARK: &str = "\0efr-step\0";

/// The one-time password of the scripted events, as base64.
const PASSWORD: &str = "dGhlIG9uZS10aW1lIHBhc3M=";

/// A paste event as Ghostty 1.4 writes it: `OK`, the list of the types (`image/png
/// text/plain`) and `DONE`, each packet with the password.
fn event() -> String {
    let packet = |meta: &str| format!("\x1b]5522;type=read:{meta}:pw={PASSWORD}\x1b\\");
    let listing = "\x1b]5522;type=read:status=DATA:mime=Lg==";
    format!(
        "{}{listing}:pw={PASSWORD};aW1hZ2UvcG5nIHRleHQvcGxhaW4K\x1b\\{}",
        packet("status=OK"),
        packet("status=DONE")
    )
}

const MODE_ON: &str = "\x1b[?5522h";
const MODE_OFF: &str = "\x1b[?5522l";

/// Variables of a terminal's environment.
type Env<'a> = &'a [(&'a str, &'a str)];

/// A terminal that allows the paste by default.
const GHOSTTY: Env<'_> = &[("TERM_PROGRAM", "ghostty"), ("TERM_PROGRAM_VERSION", "1.4.0")];

/// A terminal of `home` with the variables `env`, where `efr clip` answers `answer`.
struct Terminal<'a> {
    home: &'a Home,
    env: Env<'a>,
    /// The seconds with no output that end a step.
    quiet: &'a str,
}

impl<'a> Terminal<'a> {
    fn new(home: &'a Home, answer: &[u8]) -> Terminal<'a> {
        std::fs::write(format!("{}.out.clip", home.records().display()), answer).unwrap();
        Terminal { home, env: GHOSTTY, quiet: "0.3" }
    }

    /// What the terminal showed at each step of `steps`, after a step that sources the
    /// plugin.
    fn run(&self, steps: &[&str]) -> Vec<String> {
        let source = format!("source {}\r", plugin().display());
        let steps: Vec<&str> = [source.as_str()].into_iter().chain(steps.iter().copied()).collect();
        let output = self
            .home
            .zsh()
            // NOTE: set, so the plugin asks the terminal nothing at load.
            .env("EFR_TERMINAL_BG", "dark")
            .env("EFR_QUIET", self.quiet)
            .env("EFR_STEPS", steps.join("\x1e"))
            .envs(self.env.iter().copied())
            .args(["-f", "-c", DRIVER])
            .output()
            .unwrap();
        let shown = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(output.status.success(), "the driver failed: {shown:?}");
        let mut screens: Vec<String> = shown.split(STEP_MARK).map(str::to_owned).collect();
        assert_eq!(screens.pop().as_deref(), Some(""), "{shown:?}");
        assert_eq!(screens.len(), steps.len(), "{shown:?}");
        screens.remove(0);
        screens
    }
}

/// The calls of `efr clip`, and the other calls.
fn clip_calls(home: &Home) -> (Vec<Call>, Vec<Call>) {
    home.calls().into_iter().partition(|call| call.args.first().is_some_and(|arg| arg == "clip"))
}

/// Checks that `call` read the paste event of the terminal of the shell.
fn assert_read(call: &Call) {
    let [clip, tty, tty_path, shell, pid] = call.args.as_slice() else {
        panic!("{:?}", call.args);
    };
    assert_eq!([clip, tty, shell], ["clip", "--tty", "--shell"]);
    assert!(tty_path.starts_with("/dev/"), "{tty_path}");
    assert!(pid.parse::<u32>().is_ok(), "{pid}");
    assert_eq!(call.clip_event.as_deref(), Some(event().as_str()));
    assert!(!call.cmdline.contains(PASSWORD), "the password stays out of argv");
}

#[test]
fn e2e_a_pasted_image_puts_its_file_into_a_prompt_line_and_the_typed_keys_follow() {
    if !zsh_tests_enabled() {
        return;
    }
    for (line, command) in [(", ", "send"), (",new ", "new")] {
        let home = Home::new();
        let terminal = Terminal::new(&home, b"file\0/state/clips/4242.1.png\0why");

        let screens = terminal.run(&[line, &event(), " is\r"]);

        assert!(screens[0].contains(MODE_ON), "{line}: {screens:?}");
        assert!(screens[2].contains(MODE_OFF), "the mode goes off at Enter: {screens:?}");
        let (clips, others) = clip_calls(&home);
        let [clip] = clips.as_slice() else { panic!("{clips:?}") };
        assert_read(clip);
        let [sent] = others.as_slice() else { panic!("{others:?}") };
        assert_eq!(sent.args, [command]);
        assert_eq!(sent.prompt.as_deref(), Some("@/state/clips/4242.1.png why is"));
    }
}

#[test]
fn e2e_a_paste_of_text_goes_into_the_line_and_an_error_adds_nothing() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    // A `,` with no blank after it gets one before the word of a file.
    let screens =
        Terminal::new(&home, b"file\0/state/clips/4242.2.png\0").run(&[",", &event(), "\r"]);
    assert!(screens[1].contains(", @/state/clips/4242.2.png "), "{screens:?}");

    let home = Home::new();
    Terminal::new(&home, b"text\0see the log\0").run(&[", ", &event(), " now\r"]);
    let (_, sent) = clip_calls(&home);
    assert_eq!(sent[0].prompt.as_deref(), Some("see the log now"));

    let home = Home::new();
    let message = "efr: the clipboard has no image and no text.";
    let answer = format!("error\0{message}\0");
    let screens = Terminal::new(&home, answer.as_bytes()).run(&[", ", &event(), "fix it\r"]);
    assert!(screens[1].contains(message), "{screens:?}");
    let (_, sent) = clip_calls(&home);
    assert_eq!(sent[0].prompt.as_deref(), Some("fix it"));
}

#[test]
fn e2e_the_second_failed_read_turns_the_paste_off_for_the_shell() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let message = "efr: the terminal sent no clipboard data in 3 s. Nothing was added.";
    let answer = format!("failed\0{message}\0");
    let event = event();

    let screens = Terminal::new(&home, answer.as_bytes())
        .run(&[", ", &event, "\x15", ", ", &event, "\x15", ", "]);

    assert!(screens[1].contains(message), "{screens:?}");
    assert!(screens[3].contains(MODE_ON), "the mode is on again: {screens:?}");
    let again =
        "efr: the clipboard read failed again. Pastes on , lines are text again in this shell.";
    assert!(screens[4].contains(again) && screens[4].contains(MODE_OFF), "{screens:?}");
    assert!(!screens[6].contains(MODE_ON), "the paste stays off: {screens:?}");
    assert_eq!(clip_calls(&home).0.len(), 2);
}

#[test]
fn e2e_a_terminal_that_refuses_the_read_turns_the_paste_off_at_once() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let message = "efr: the terminal does not allow clipboard reads (clipboard-read). \
                   Pastes on , lines are text again in this shell.";
    let answer = format!("off\0{message}\0");

    let screens = Terminal::new(&home, answer.as_bytes()).run(&[", ", &event(), "\x15", ", "]);

    assert!(screens[1].contains(message) && screens[1].contains(MODE_OFF), "{screens:?}");
    assert!(!screens[3].contains(MODE_ON), "{screens:?}");
}

#[test]
fn e2e_a_late_reply_is_drained_and_never_typed_into_the_line() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    // A data packet of a reply has no password; the keys after it go back to the line.
    let late = "\x1b]5522;type=read:status=DATA:mime=aW1hZ2UvcG5n;iVBORw0KGgo=\x1b\\";

    Terminal::new(&home, b"\0\0go").run(&[", ", late, " on\r"]);

    let (clips, sent) = clip_calls(&home);
    let [drain] = clips.as_slice() else { panic!("{clips:?}") };
    assert_eq!(drain.args[..3], ["clip", "--drain", "--tty"]);
    assert_eq!(drain.clip_event, None);
    assert_eq!(sent[0].prompt.as_deref(), Some("go on"));
}

#[test]
fn e2e_an_event_that_does_not_end_is_drained_and_counts_as_a_failed_read() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let start = format!("\x1b]5522;type=read:status=OK:pw={PASSWORD}\x1b\\");
    let mut terminal = Terminal::new(&home, b"\0\0");
    // NOTE: the widget waits 1 s for the rest of the event.
    terminal.quiet = "1.5";

    let screens = terminal.run(&[", ", &start]);

    assert!(screens[1].contains("efr: the clipboard read stopped. Nothing was added."));
    let (clips, _) = clip_calls(&home);
    assert_eq!(clips.iter().map(|call| call.args[1].as_str()).collect::<Vec<_>>(), ["--drain"]);
}

#[test]
fn e2e_the_mode_is_on_only_at_a_prompt_line_that_the_gate_allows() {
    if !zsh_tests_enabled() {
        return;
    }
    let ghostty = ("TERM_PROGRAM", "ghostty");
    let tmux = ("TMUX", "/tmp/tmux-1000/default,1,0");
    let cases: [(&str, Env<'_>, Option<&str>, bool); 6] = [
        ("auto in Ghostty 1.4", GHOSTTY, None, true),
        ("auto in Ghostty 1.3", &[ghostty, ("TERM_PROGRAM_VERSION", "1.3.1")], None, false),
        ("auto with no version", &[ghostty], None, false),
        ("auto in tmux", &[GHOSTTY[0], GHOSTTY[1], tmux], None, false),
        ("on outside Ghostty", &[], Some("on"), true),
        ("off in Ghostty 1.4", GHOSTTY, Some("off"), false),
    ];
    for (case, env, paste, on) in cases {
        let home = Home::new();
        let runtime = home.path().join("efr");
        std::fs::create_dir_all(&runtime).unwrap();
        let file = paste.map_or_else(String::new, |paste| format!("paste\t{paste}\n"));
        std::fs::write(runtime.join("prompt.tsv"), format!("mode\tauto\n{file}")).unwrap();
        let mut terminal = Terminal::new(&home, b"");
        terminal.env = env;

        let screens = terminal.run(&[", "]);

        assert_eq!(screens[0].contains(MODE_ON), on, "{case}: {screens:?}");
    }

    // A command line and an empty line get no mode; in sticky mode an empty line and a
    // line that does not start with `!` get it, and `!` turns it off.
    let home = Home::new();
    let screens = Terminal::new(&home, b"").run(&["ls ", "\x15", "<C-Space>", "why", "\x15!"]);
    assert!(!screens[0].contains(MODE_ON) && !screens[1].contains(MODE_ON), "{screens:?}");
    assert!(screens[2].contains(MODE_ON), "sticky mode: {screens:?}");
    assert!(!screens[3].contains(MODE_OFF), "{screens:?}");
    assert!(screens[4].contains(MODE_OFF), "a `!` line: {screens:?}");

    // Ctrl+C ends a prompt line with no line-finish hook: the end string of ZLE's
    // bracketed paste turns the mode off, and the next prompt line turns it on again.
    let home = Home::new();
    let screens = Terminal::new(&home, b"").run(&[", ", "\x03", ", "]);
    assert!(screens[0].contains(MODE_ON), "{screens:?}");
    assert!(screens[1].contains(MODE_OFF), "Ctrl+C: {screens:?}");
    assert!(screens[2].contains(MODE_ON), "the next prompt line: {screens:?}");
}

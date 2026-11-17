//! The wake of a waiting prompt (`TM-Endings` 7.2, frames 2 and 3): efrd writes a line
//! to the terminal's FIFO `notices/<tty>.wake` after each notice line, and the plugin
//! prints the rows above the prompt at once and sends the desktop notice (OSC 777) of
//! each body. No efrd runs here: the tests make the FIFO and write the lines as efrd
//! does (`crates/efr-daemon/src/notices/wake_fifo.rs`).

use std::os::fd::OwnedFd;
use std::os::unix::fs::FileTypeExt as _;
use std::path::{Path, PathBuf};

use efr_protocol::{Event, PromptSendResult};
use efr_test_daemon::{Client, ResponsesAnswer, ResponsesServer, TTY, TestDaemon, events_until};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;
use rustix::fs::{Mode, OFlags};

use super::{Home, notice, plugin, zsh_tests_enabled};

/// Sends prompt `n` from the terminal [`TTY`] and follows its turn to the end with
/// `client`, which has no terminal: so the daemon leaves the terminal a notice.
async fn prompt_to_its_end(daemon: &TestDaemon, client: &Client, n: u128) {
    let sent: PromptSendResult = client.call(daemon.prompt(n, "say hello", TTY)).await.unwrap();
    let mut follow = daemon.follow(client, sent.conversation_id).await.unwrap();
    events_until(&mut follow, |event| matches!(event, Event::TurnCompleted { .. })).await.unwrap();
}

/// What waits in the FIFO of `fd` now.
fn drained(fd: &OwnedFd) -> String {
    let mut text = Vec::new();
    let mut buf = [0_u8; 4096];
    while let Ok(read @ 1..) = rustix::io::read(fd, &mut buf) {
        text.extend_from_slice(&buf[..read]);
    }
    String::from_utf8(text).unwrap()
}

/// efrd makes the wake FIFO of the terminal of a prompt, and after a notice line for
/// that terminal it writes a line to the FIFO: an empty one for the end of a turn that
/// a prompt started, which needs no desktop notice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_the_daemon_makes_the_fifo_and_wakes_the_prompt_after_a_notice() {
    let server = ResponsesServer::start().await;
    server.push(ResponsesAnswer::text("Hello."));
    server.push(ResponsesAnswer::text("Hello again."));
    let daemon = TestDaemon::builder().responses(&server).start().await.unwrap();
    let notices = daemon.dirs().dirs().runtime().join("notices");
    let name = TTY.trim_start_matches("/dev/").replace('/', "-");
    let (file, fifo) = (notices.join(&name), notices.join(format!("{name}.wake")));
    let client = daemon.client().await.unwrap();
    prompt_to_its_end(&daemon, &client, 1).await;
    // NOTE: efrd makes the FIFO on its blocking pool and does not wait for it.
    Wait::new("the wake FIFO of the terminal")
        .until(|| std::fs::metadata(&fifo).is_ok_and(|meta| meta.file_type().is_fifo()))
        .await
        .unwrap();
    Wait::new("the notice of the first turn")
        .until(|| std::fs::read(&file).is_ok_and(|text| text.ends_with(b"\n")))
        .await
        .unwrap();
    std::fs::remove_file(&file).unwrap();
    // The plugin's open: read and write, so the FIFO never ends, and without blocking.
    let reader = rustix::fs::open(&fifo, OFlags::RDWR | OFlags::NONBLOCK, Mode::empty()).unwrap();

    prompt_to_its_end(&daemon, &client, 2).await;

    let woke = Wait::new("a line in the FIFO")
        .until_some(|| Some(drained(&reader)).filter(|text| !text.is_empty()))
        .await
        .unwrap();
    assert_eq!(woke, "\n", "no desktop notice for the end of a turn that a prompt started");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("efr: turn finished: "), "the line came first: {text:?}");
    daemon.stop().await.unwrap();
}

/// The desktop notice that a body gives, as the terminal gets it.
fn osc(body: &str) -> String {
    format!("\x1b]777;notify;efr;{body}\x1b\\")
}

/// Makes the wake FIFO of `/dev/pts/77` under the runtime root `runtime`, as efrd does.
fn wake_fifo(runtime: &Path) -> PathBuf {
    let dir = runtime.join("notices");
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("pts-77.wake");
    rustix::fs::mkfifoat(rustix::fs::CWD, &fifo, Mode::from_raw_mode(0o600)).unwrap();
    fifo
}

/// Runs `script` after the plugin in `zsh -f -i -c` at the terminal `/dev/pts/77`, with
/// the runtime root `runtime` and the variables `env`; returns stdout and stderr.
fn run_at_pts_77(home: &Home, runtime: &Path, env: &[(&str, &str)], script: &str) -> [String; 2] {
    let full = format!("source {}\nTTY=/dev/pts/77\n{script}\n", plugin().display());
    let output = home
        .zsh()
        .env("EFR_RUNTIME_DIR", runtime)
        .envs(env.iter().copied())
        .args(["-f", "-i", "-c", &full])
        .output()
        .unwrap();
    let [stdout, stderr] =
        [output.stdout, output.stderr].map(|bytes| String::from_utf8(bytes).unwrap());
    assert!(output.status.success(), "zsh failed: {stderr}");
    [stdout, stderr]
}

#[test]
fn e2e_precmd_empties_the_fifo_shows_the_notices_and_sends_no_desktop_notice() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let runtime = home.path().join("runtime");
    let fifo = wake_fifo(&runtime);
    std::fs::write(runtime.join("prompt.tsv"), "mode\tauto\nnotify\ton\n").unwrap();
    let script = format!(
        "_efr_precmd\nprint -r -- \"open=[${{+_efr_wake_fd}}${{_efr_wake_fd:+yes}}]\"\n\
         print -r -- 'app: a question waits' > {fifo}\n_efr_precmd\n\
         _efr_drain_wake\nprint -r -- \"left=[$REPLY]\"",
        fifo = fifo.display()
    );
    notice(&runtime, "warning\tefr: a question waits in a conversation");

    let [shown, stderr] = run_at_pts_77(&home, &runtime, &[("TERM_PROGRAM", "ghostty")], &script);

    // NOTE: the first precmd shows the row; the second empties the FIFO with no
    // desktop notice, because the user is at the terminal.
    assert_eq!(shown, "efr: a question waits in a conversation\nopen=[1yes]\nleft=[]\n");
    assert_eq!(stderr, "");
}

#[test]
fn e2e_the_plugin_loads_without_a_fifo_and_with_an_old_prompt_file() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let runtime = home.path().join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    let script = "_efr_precmd\n_efr_precmd\n_efr_read_prompt_file\n\
                  print -r -- \"fd=[$_efr_wake_fd] notify=[$_efr_notify]\"";
    let cases = [
        // The prompt file of efrd 0.0.11 has no notify line.
        ("mode\tcautious\n", "auto"),
        ("mode\tcautious\nnotify\toff\n", "off"),
        ("mode\tcautious\nnotify\ton\n", "on"),
        ("mode\tcautious\nnotify\tloud\n", "auto"),
    ];
    for (file, notify) in cases {
        std::fs::write(runtime.join("prompt.tsv"), file).unwrap();

        let [shown, stderr] = run_at_pts_77(&home, &runtime, &[], script);

        assert_eq!(shown, format!("fd=[] notify=[{notify}]\n"), "{file:?}");
        assert_eq!(stderr, "", "{file:?}");
    }
}

#[test]
fn e2e_no_program_that_the_shell_starts_keeps_the_fifo() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let runtime = home.path().join("runtime");
    let fifo = wake_fifo(&runtime);
    let script = "_efr_precmd\nzstat -L +link -- /proc/$$/fd/$_efr_wake_fd\n\
                  print -r -- children:\nls -l /proc/self/fd/";

    let [shown, _] = run_at_pts_77(&home, &runtime, &[], script);

    let (shell, child) = shown.split_once("children:\n").unwrap();
    assert_eq!(shell.trim_end(), fifo.display().to_string(), "the shell reads the FIFO");
    assert!(!child.contains("pts-77.wake"), "a child holds no descriptor of it:\n{child}");
}

#[test]
fn e2e_a_nested_zsh_leaves_the_fifo_to_its_shell_and_exec_zsh_opens_it() {
    if !zsh_tests_enabled() {
        return;
    }
    let home = Home::new();
    let runtime = home.path().join("runtime");
    wake_fifo(&runtime);
    let inner = |word: &str| {
        format!(
            "zsh -f -i -c 'source {}; TTY=/dev/pts/77; _efr_precmd; \
             print -r -- \"{word}=[${{_efr_wake_fd:+open}}]\"'",
            plugin().display()
        )
    };
    let script = format!(
        "_efr_precmd\nprint -r -- \"outer=[${{_efr_wake_fd:+open}}] $EFR_WAKE_TTY\"\n\
         {}\nexec {}",
        inner("nested"),
        inner("exec")
    );

    let [shown, _] = run_at_pts_77(&home, &runtime, &[], &script);

    let lines: Vec<&str> = shown.lines().collect();
    assert_eq!(lines.len(), 3, "{shown}");
    assert!(lines[0].starts_with("outer=[open] /dev/pts/77:"), "{shown}");
    assert_eq!(lines[1..], ["nested=[]", "exec=[open]"]);
}

/// Drives an interactive zsh on a pseudo-terminal with a real ZLE: it sources
/// `$EFR_SETUP` (the plugin, then the FIFO of the terminal, as efrd makes it), types a
/// half command at the waiting prompt, then writes `$EFR_ROW` to the terminal's notice
/// file and `$EFR_BODY` to its FIFO, as efrd does, waits for the row and the redrawn
/// line, and prints what the terminal showed.
const DRIVER: &str = r#"
zmodload zsh/zpty || exit 90
zpty user "TERM=xterm zsh -f -i"
zpty -r user screen $'*\e\\[\\?2004h*' || exit 91
print -rn -- "$screen"
zpty -w -n user "source $EFR_SETUP"$'\r'
zpty -r user screen '*P> *' || exit 92
print -rn -- "$screen"
zpty -w -n user 'git commit -m half'
zpty -r user screen '*half*' || exit 93
print -rn -- "$screen"
name=$(<$EFR_RUNTIME_DIR/tty)
print -r -- "$EFR_ROW" >> $EFR_RUNTIME_DIR/notices/$name
print -r -- "$EFR_BODY" > $EFR_RUNTIME_DIR/notices/$name.wake
zpty -r user screen '*a question waits in*' || exit 94
print -rn -- "$screen"
zpty -r user screen '*half*' || exit 95
print -rn -- "$screen"
zpty -w -n user $'\x15exit\r'
while zpty -r user chunk; do print -rn -- "$chunk"; done
"#;

/// What the setup line of [`DRIVER`] runs in the terminal: the plugin, the terminal's
/// FIFO (zsh cannot make one, so the test runs mkfifo) and a short prompt.
const SETUP: &str = r#"source $EFR_PLUGIN
name=${${TTY#/dev/}//\//-}
mkdir -p $EFR_RUNTIME_DIR/notices
mkfifo -m 600 $EFR_RUNTIME_DIR/notices/$name.wake
print -r -- $name > $EFR_RUNTIME_DIR/tty
PS1='P> '
"#;

/// The row of a question that waits, as efrd writes it.
const QUESTION_ROW: &str =
    "warning\tefr: a question waits in \"deploy the app\" (/dev/pts/4) \u{b7} ,resume 019a9b11";

/// Variables of a terminal's environment.
type Env<'a> = &'a [(&'a str, &'a str)];

/// What a terminal showed when a notice with the body `body` came to its waiting prompt,
/// with `notify` in `prompt.tsv` (none for the default) and the variables `env`.
fn woken(notify: Option<&str>, env: Env<'_>, body: &str) -> String {
    let home = Home::new();
    let runtime = home.path().join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    let mut prompt_file = String::from("mode\tauto\n");
    if let Some(notify) = notify {
        prompt_file.push_str(&format!("notify\t{notify}\n"));
    }
    std::fs::write(runtime.join("prompt.tsv"), prompt_file).unwrap();
    let setup = home.path().join("setup.zsh");
    std::fs::write(&setup, SETUP).unwrap();
    let output = home
        .zsh()
        .env("LANG", "C.UTF-8")
        // NOTE: set, so the plugin asks the terminal nothing at load.
        .env("EFR_TERMINAL_BG", "dark")
        .env("EFR_RUNTIME_DIR", &runtime)
        .env("EFR_PLUGIN", plugin())
        .env("EFR_SETUP", &setup)
        .env("EFR_ROW", QUESTION_ROW)
        .env("EFR_BODY", body)
        .envs(env.iter().copied())
        .args(["-f", "-c", DRIVER])
        .output()
        .unwrap();
    let screen = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the driver failed: {stderr}\n{screen}");
    screen
}

#[test]
fn e2e_a_notice_shows_above_the_waiting_prompt_and_the_typed_text_stays() {
    if !zsh_tests_enabled() {
        return;
    }
    let body = "app: a question waits";

    // Two questions in one wake, and the empty line of a row that needs no notice.
    let screen = woken(Some("on"), &[], &format!("{body}\n\n{body}"));

    let half = screen.find("git commit -m half").unwrap();
    let row = screen.find("efr: a question waits in \"deploy the app\"").unwrap();
    let sent = screen.find(&osc(body)).unwrap_or_else(|| panic!("no notice: {screen:?}"));
    let again = screen.rfind("P> git commit -m half").unwrap();
    assert!(half < row && row < sent && sent < again, "{screen:?}");
    // The row keeps its warning colour (16 colours: slot 3).
    assert!(screen[..sent].contains("\x1b[33mefr: a question waits in"), "{screen:?}");
    assert_eq!(screen.matches("\x1b]777;").count(), 1, "each body once: {screen:?}");
}

#[test]
fn e2e_the_desktop_notice_follows_notify_terminal() {
    if !zsh_tests_enabled() {
        return;
    }
    let body = "app: job notice turn finished";
    let ghostty = ("TERM_PROGRAM", "ghostty");
    let cases: [(&str, Option<&str>, Env<'_>, bool); 6] = [
        ("off", Some("off"), &[ghostty], false),
        ("on outside Ghostty", Some("on"), &[], true),
        ("auto in Ghostty", None, &[ghostty], true),
        ("auto outside Ghostty", None, &[], false),
        ("auto in tmux", None, &[ghostty, ("TMUX", "/tmp/tmux-1000/default,1,0")], false),
        ("auto in zellij", None, &[ghostty, ("ZELLIJ", "0")], false),
    ];
    for (case, notify, env, sends) in cases {
        let screen = woken(notify, env, body);

        assert!(screen.contains("efr: a question waits in"), "{case}: {screen:?}");
        assert_eq!(screen.contains(&osc(body)), sends, "{case}: {screen:?}");
    }
}

#[test]
fn e2e_a_body_from_the_fifo_cannot_send_an_escape() {
    if !zsh_tests_enabled() {
        return;
    }
    let long = "x".repeat(300);
    let body = format!("app: a\x1b]0;pwned\x07 question\x7f waits {long}");

    let screen = woken(Some("on"), &[], &body);

    let clean: String = format!("app: a]0;pwned question waits {long}").chars().take(200).collect();
    assert!(screen.contains(&osc(&clean)), "{screen:?}");
    assert!(!screen.contains("\x1b]0;"), "{screen:?}");
}

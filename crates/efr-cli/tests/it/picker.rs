//! The picker of `efr resume` with the built `efr` on a pseudo-terminal, a fake daemon
//! and a fake `fzf`: the command line and the environment that fzf gets, the reload
//! that it runs on a key, and each way that fzf can end.
//!
//! The fake `fzf` is a `sh` script in a temporary `bin` directory, first on a `PATH`
//! that never holds `~/.local/bin`. `--version` prints the version that the test
//! chooses. Otherwise it records its command line, its environment and its stdin, runs
//! the command of its `reload-sync` action with `FZF_QUERY` set as fzf does, then
//! prints the query, the key that the test chooses and the first row of the reload, as fzf
//! prints them with `--print-query` and `--expect`, and exits with the status that the test
//! chooses.
//!
//! `efr` gets a cleared environment with every efr root in a temporary directory, so it
//! never reaches the real daemon.
//!
//! The `e2e_fzf_` tests at the end run the real fzf against a `TestDaemon` instead;
//! they skip unless `EFR_TEST_FZF=1` (`just test-fzf`).

use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use efr_protocol::framing::{self, Decoder};
use efr_protocol::{
    Capabilities, ClientFrame, ConversationHistoryResult, ConversationResumeResult,
    ConversationStatus, ConversationSummary, ConversationsListResult, ConversationsRecentResult,
    ConversationsSearchResult, DaemonPaths, Event, HelloResult, Method, PROTOCOL_VERSION,
    PromptSendResult, RecentConversation, RequestId, RouteLabel, RouteScope, SearchRow, Seq,
    ServerFrame,
};
use efr_stdx::time::Clock as _;
use efr_test_daemon::{ResponsesAnswer, ResponsesServer, SearchRowHook, TestDaemon, events_until};
use pretty_assertions::assert_eq;
use serde::Serialize;

/// The terminal that the plugin's context names.
pub(crate) const TTY: &str = "/dev/pts/77";

/// The fake `fzf`; `$FAKE_FZF_DIR` is where it records, `$FAKE_FZF_VERSION`,
/// `$FAKE_FZF_KEY`, `$FAKE_FZF_QUERY` and `$FAKE_FZF_EXIT` are what it does.
pub(crate) const FAKE_FZF: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf '%s (fake)\n' "$FAKE_FZF_VERSION"
  exit 0
fi
dir=$FAKE_FZF_DIR
cat /proc/$$/cmdline > "$dir/cmdline"
env > "$dir/env"
cat > "$dir/stdin"
reload=
for arg in "$@"; do
  case $arg in
    --bind=change:first+reload-sync:*) reload=${arg#--bind=change:first+reload-sync:} ;;
  esac
done
if [ -n "$FAKE_FZF_QUERY" ]; then
  FZF_QUERY=$FAKE_FZF_QUERY $SHELL -c "$reload" > "$dir/reload" 2> "$dir/reload.err"
  row=$(sed -n 2p "$dir/reload")
else
  row=$(sed -n 2p "$dir/stdin")
fi
printf '%s\n%s\n%s\n' "$FAKE_FZF_QUERY" "$FAKE_FZF_KEY" "$row"
exit "${FAKE_FZF_EXIT:-0}"
"#;

/// A pseudo-terminal pair: the master, which plays the screen, and the slave, which
/// `efr` gets as stdin and stdout.
pub(crate) fn pty() -> (OwnedFd, OwnedFd) {
    pty_of(24, 100)
}

/// A pseudo-terminal pair of `rows` and `cols`.
fn pty_of(rows: u16, cols: u16) -> (OwnedFd, OwnedFd) {
    use rustix::fs::{Mode, OFlags};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC).unwrap();
    grantpt(&master).unwrap();
    unlockpt(&master).unwrap();
    let name = ptsname(&master, Vec::new()).unwrap();
    let slave =
        rustix::fs::open(name.as_c_str(), OFlags::RDWR | OFlags::NOCTTY, Mode::empty()).unwrap();
    let size = rustix::termios::Winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
    rustix::termios::tcsetwinsize(&master, size).unwrap();
    (master, slave)
}

/// What the master of a pty holds, after the process on its slave ended.
pub(crate) fn screen_of(master: &OwnedFd) -> String {
    let mut screen = Vec::new();
    rustix::io::ioctl_fionbio(master, true).unwrap();
    let mut buffer = [0_u8; 4096];
    while let Ok(read) = rustix::io::read(master, &mut buffer) {
        if read == 0 {
            break;
        }
        screen.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8_lossy(&screen).into_owned()
}

/// `text` without its escape sequences of colour.
fn bare(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out
}

/// Temporary efr roots, a `bin` directory with the fake `fzf`, and where it records.
pub(crate) struct Roots {
    dir: tempfile::TempDir,
}

impl Roots {
    pub(crate) fn new() -> Roots {
        let dir = tempfile::tempdir().unwrap();
        for name in ["home", "config", "data", "state", "runtime", "bin", "fzf", "project"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        let fzf = dir.path().join("bin/fzf");
        std::fs::write(&fzf, FAKE_FZF).unwrap();
        std::fs::set_permissions(&fzf, std::fs::Permissions::from_mode(0o755)).unwrap();
        Roots { dir }
    }

    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    pub(crate) fn socket(&self) -> PathBuf {
        self.path("runtime").join("daemon.sock")
    }

    /// What the fake fzf recorded in `name`; empty when it did not run.
    fn recorded(&self, name: &str) -> String {
        std::fs::read_to_string(self.path("fzf").join(name)).unwrap_or_default()
    }

    /// `efr resume <args>` on the slave of a pty, with the plugin's context, the fake
    /// fzf first on the `PATH`, `version` for `fzf --version`, and the rest of `vars`.
    pub(crate) fn resume(
        &self,
        slave: &OwnedFd,
        args: &[&str],
        version: &str,
        vars: &[(&str, &str)],
    ) -> std::process::Command {
        let context = serde_json::json!({
            "pwd": self.path("project"),
            "tty": TTY,
            "shell_pid": 4242,
        });
        let mut command =
            efr_stdx::process::command(env!("CARGO_BIN_EXE_efr"), self.dir.path()).into_std();
        command
            .env_clear()
            .env("HOME", self.path("home"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.path("bin").display()))
            .env("TERM", "xterm-256color")
            .env("SHELL", "/bin/zsh")
            .env("EFR_CONFIG_DIR", self.path("config"))
            .env("EFR_DATA_DIR", self.path("data"))
            .env("EFR_STATE_DIR", self.path("state"))
            .env("EFR_RUNTIME_DIR", self.path("runtime"))
            .env("EFR_CONTEXT", context.to_string())
            .env("FAKE_FZF_DIR", self.path("fzf"))
            .env("FAKE_FZF_VERSION", version)
            .args(["resume"])
            .args(args)
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(Stdio::piped());
        for (name, value) in vars {
            command.env(name, value);
        }
        command
    }
}

pub(crate) fn hello_result() -> HelloResult {
    HelloResult {
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        protocol: PROTOCOL_VERSION,
        version: "0.1.0".to_owned(),
        capabilities: Capabilities { search: Some(true), ..Capabilities::default() },
        paths: DaemonPaths::default(),
        challenge: "AAAA".to_owned(),
    }
}

fn id(n: u64) -> String {
    format!("019a9b1c-3d00-7a10-8b20-{n:012x}")
}

fn rows() -> Vec<RecentConversation> {
    let at: jiff::Timestamp = "2026-10-20T12:00:00Z".parse().unwrap();
    [(1, "rsync the photos to the laptop"), (2, "docker compose for the blog")]
        .into_iter()
        .map(|(n, title)| RecentConversation {
            summary: ConversationSummary {
                id: id(n).parse().unwrap(),
                title: Some(title.to_owned()),
                status: ConversationStatus::Idle,
                created_at: at,
                updated_at: at,
                last_seq: Seq::new(n),
                cwd: None,
                scope: None,
                tty: None,
            },
            route: RouteScope::Outside,
            project: None,
            turns: 3,
            open_in: None,
            here: false,
        })
        .collect()
}

/// One accepted connection of the fake daemon.
struct Conn {
    stream: UnixStream,
    decoder: Decoder,
    ready: VecDeque<Vec<u8>>,
}

impl Conn {
    fn recv(&mut self) -> Option<ClientFrame> {
        loop {
            if let Some(payload) = self.ready.pop_front() {
                return Some(ClientFrame::from_json(&payload).unwrap());
            }
            let mut buf = [0_u8; 4096];
            let read = match self.stream.read(&mut buf) {
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => 0,
                Err(error) => panic!("reading from efr failed: {error}"),
            };
            if read == 0 {
                return None;
            }
            self.ready.extend(self.decoder.push(&buf[..read]).unwrap());
        }
    }

    fn reply<T: Serialize>(&mut self, id: RequestId, value: &T) {
        self.stream
            .write_all(&framing::encode(&ServerFrame::item(id, value).unwrap()).unwrap())
            .unwrap();
        self.stream.write_all(&framing::encode(&ServerFrame::end(id)).unwrap()).unwrap();
    }
}

/// The requests of each connection, in the order of the connections.
type Seen = Arc<Mutex<Vec<Vec<Method>>>>;

/// A fake daemon that serves each connection on a thread of its own: it searches
/// (the two [`rows`] for any query), lists them as the recent ones, resumes any
/// conversation and gives an empty page of history.
fn serve(socket: &Path) -> Seen {
    let listener = UnixListener::bind(socket).unwrap();
    let seen: Seen = Arc::default();
    let all = Arc::clone(&seen);
    std::thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            let index = {
                let mut all = all.lock().unwrap();
                all.push(Vec::new());
                all.len() - 1
            };
            let all = Arc::clone(&all);
            std::thread::spawn(move || {
                let mut conn = Conn { stream, decoder: Decoder::new(), ready: VecDeque::new() };
                while let Some(frame) = conn.recv() {
                    let ClientFrame::Request { id, method } = frame else { continue };
                    all.lock().unwrap()[index].push(method.clone());
                    answer(&mut conn, id, &method);
                }
            });
        }
    });
    seen
}

fn answer(conn: &mut Conn, id: RequestId, method: &Method) {
    match method {
        Method::Hello(_) => conn.reply(id, &hello_result()),
        Method::ConversationsSearch(_) => {
            let rows = rows()
                .into_iter()
                .map(|conversation| SearchRow {
                    short_id: conversation.summary.id.to_string()[..8].to_owned(),
                    conversation,
                    title_ranges: Vec::new(),
                    hit: None,
                })
                .collect();
            conn.reply(id, &ConversationsSearchResult { rows, ..Default::default() });
        }
        Method::ConversationsRecent(_) => {
            let recent =
                ConversationsRecentResult { scope: None, current: None, conversations: rows() };
            conn.reply(id, &recent);
        }
        Method::ConversationResume(params) => {
            let resumed = ConversationResumeResult::Resumed {
                conversation_id: params.conversation_id,
                scope: RouteLabel {
                    scope: RouteScope::Outside,
                    name: None,
                    dir: PathBuf::from("/srv"),
                },
                title: Some("docker compose for the blog".to_owned()),
                updated_at: "2026-10-20T12:00:00Z".parse().unwrap(),
                shell_dir: None,
                here: false,
                continues_here: true,
            };
            conn.reply(id, &resumed);
        }
        Method::ConversationHistory(_) => {
            conn.reply(id, &ConversationHistoryResult { events: Vec::new(), next_cursor: None });
        }
        Method::ConversationsList(_) => {
            let list = ConversationsListResult { conversations: Vec::new(), next_cursor: None };
            conn.reply(id, &list);
        }
        other => panic!("unexpected {}", other.name()),
    }
}

/// What one run of `efr resume` did.
struct Ran {
    status: ExitStatus,
    screen: String,
    stderr: String,
    seen: Vec<Vec<Method>>,
}

/// Runs `efr resume <args>` with the fake fzf of `version` and `vars`.
fn run(roots: &Roots, args: &[&str], version: &str, vars: &[(&str, &str)]) -> Ran {
    let seen = serve(&roots.socket());
    let (master, slave) = pty();
    let output = roots.resume(&slave, args, version, vars).output().unwrap();
    let screen = bare(&screen_of(&master));
    let seen = seen.lock().unwrap().clone();
    Ran { status: output.status, screen, stderr: String::from_utf8(output.stderr).unwrap(), seen }
}

/// The names of the methods of each connection.
fn methods(seen: &[Vec<Method>]) -> Vec<Vec<&'static str>> {
    seen.iter().map(|conn| conn.iter().map(Method::name).collect()).collect()
}

/// The fake fzf's environment as name and value pairs.
fn env_of(roots: &Roots) -> Vec<(String, String)> {
    roots
        .recorded("env")
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

fn var<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
    env.iter().find(|(known, _)| known == name).map(|(_, value)| value.as_str())
}

#[test]
fn enter_resumes_the_row_and_fzf_gets_only_what_it_needs() {
    let roots = Roots::new();
    let ran = run(
        &roots,
        &[],
        "0.73.1",
        &[
            ("FAKE_FZF_QUERY", "docker"),
            ("FZF_DEFAULT_OPTS", "--tmux"),
            ("FZF_DEFAULT_OPTS_FILE", "/home/user/.fzfrc"),
            ("FZF_DEFAULT_COMMAND", "find ."),
            ("EFR_PROMPT", "a stale prompt"),
        ],
    );

    assert!(ran.status.success(), "{ran:?}", ran = (&ran.status, &ran.screen, &ran.stderr));
    assert!(ran.screen.contains("↪ resumed \"docker compose for the blog\""), "{}", ran.screen);
    // The first connection searched with the empty query and resumed; the reload of
    // fzf searched for the query of fzf on a connection of its own.
    assert_eq!(
        methods(&ran.seen),
        [
            vec!["hello", "conversations.search", "conversation.resume"],
            vec!["hello", "conversations.search"],
        ]
    );
    let Method::ConversationsSearch(first) = &ran.seen[0][1] else { unreachable!() };
    assert_eq!(
        (first.query.as_str(), first.limit, first.tty.as_deref()),
        ("", Some(50), Some(TTY))
    );
    let Method::ConversationsSearch(reload) = &ran.seen[1][1] else { unreachable!() };
    assert_eq!((reload.query.as_str(), reload.tty.as_deref()), ("docker", Some(TTY)));
    assert_eq!(reload.pwd.as_deref(), Some(roots.path("project").as_path()), "fzf runs there");
    let Method::ConversationResume(resume) = &ran.seen[0][2] else { unreachable!() };
    assert_eq!(resume.conversation_id.to_string(), id(1));

    // The command line: the snapshot of the unit tests, with this efr and terminal.
    let cmdline = roots.recorded("cmdline");
    let words: Vec<&str> = cmdline.split_terminator('\0').skip(2).collect();
    let efr = format!("'{}'", env!("CARGO_BIN_EXE_efr"));
    assert!(words.contains(&"--height=40%"), "{words:?}");
    let bind = format!("--bind=change:first+reload-sync:exec {efr} search --picker --tty '{TTY}'");
    assert!(words.contains(&bind.as_str()), "the same efr: {words:?}");
    assert!(!cmdline.contains("docker"), "no query in a command line");

    // The environment: no private variable, none of fzf's defaults, and sh.
    let env = env_of(&roots);
    for private in ["EFR_CONTEXT", "EFR_RESUME_WORDS", "EFR_PROMPT", "EFR_LAST_COMMAND"] {
        assert_eq!(var(&env, private), None, "{private}");
    }
    for default in ["FZF_DEFAULT_OPTS", "FZF_DEFAULT_OPTS_FILE", "FZF_DEFAULT_COMMAND"] {
        assert_eq!(var(&env, default), None, "{default}");
    }
    assert_eq!(var(&env, "SHELL"), Some("/bin/sh"));
    assert_eq!(var(&env, "EFR_RUNTIME_DIR"), Some(roots.path("runtime").to_str().unwrap()));
    assert_eq!(var(&env, "PWD"), Some(roots.path("project").to_str().unwrap()));

    // The first rows: the header, then a row per conversation, the id first.
    let stdin = bare(&roots.recorded("stdin"));
    let lines: Vec<&str> = stdin.lines().collect();
    assert_eq!(lines.len(), 3, "{stdin}");
    assert!(lines[0].starts_with("\tenter resume"), "{stdin}");
    assert!(lines[1].starts_with(&format!("{}\trsync the photos", id(1))), "{stdin}");
    assert_eq!(roots.recorded("reload.err"), "", "the reload writes nothing to stderr");
}

#[test]
fn words_of_resume_reach_fzf_in_its_environment_only() {
    let roots = Roots::new();
    let ran = run(
        &roots,
        &[],
        "0.73.1",
        &[("EFR_RESUME_WORDS", "rsync it's laptop"), ("FAKE_FZF_KEY", "esc")],
    );

    assert!(ran.status.success(), "{}", ran.stderr);
    assert_eq!(methods(&ran.seen), [vec!["hello", "conversations.search"]], "esc moves nothing");
    let Method::ConversationsSearch(first) = &ran.seen[0][1] else { unreachable!() };
    assert_eq!(first.query, "rsync it's laptop");
    let env = env_of(&roots);
    assert_eq!(var(&env, "FZF_DEFAULT_OPTS"), Some(r"--query='rsync it'\''s laptop'"));
    assert_eq!(var(&env, "EFR_RESUME_WORDS"), None);
    let cmdline = roots.recorded("cmdline");
    for word in ["rsync", "laptop"] {
        assert!(!cmdline.contains(word), "{word} in {cmdline:?}");
    }
    assert_eq!(ran.screen, "", "esc prints nothing");
}

#[test]
fn ctrl_o_prints_the_conversation_and_resumes_nothing() {
    let roots = Roots::new();
    let ran = run(&roots, &[], "0.73.1", &[("FAKE_FZF_KEY", "ctrl-o")]);

    assert!(ran.status.success(), "{}", ran.stderr);
    assert_eq!(methods(&ran.seen), [vec!["hello", "conversations.search", "conversation.history"]]);
    assert!(ran.screen.contains(&format!("conversation {}", id(1))), "{}", ran.screen);
}

#[test]
fn ctrl_c_in_fzf_exits_130_with_no_message() {
    let roots = Roots::new();
    let ran = run(&roots, &[], "0.73.1", &[("FAKE_FZF_EXIT", "130")]);

    assert_eq!(ran.status.code(), Some(130));
    assert_eq!((ran.screen.as_str(), ran.stderr.as_str()), ("", ""));
    assert_eq!(methods(&ran.seen), [vec!["hello", "conversations.search"]]);
}

#[test]
fn a_failed_fzf_gives_the_list_and_its_status() {
    let roots = Roots::new();
    let ran = run(&roots, &[], "0.73.1", &[("FAKE_FZF_EXIT", "2")]);

    assert!(ran.status.success(), "{}", ran.stderr);
    assert!(ran.screen.contains(" 1  rsync the photos to the laptop"), "{}", ran.screen);
    assert!(
        ran.screen.ends_with("fzf ended with status 2, so efr shows the list\n"),
        "{}",
        ran.screen
    );
}

#[test]
fn an_fzf_older_than_the_floor_gives_the_list() {
    let roots = Roots::new();
    let ran = run(&roots, &[], "0.44.1", &[]);

    assert!(ran.status.success(), "{}", ran.stderr);
    assert_eq!(methods(&ran.seen), [vec!["hello", "conversations.recent"]]);
    assert!(ran.screen.contains(" 2  docker compose for the blog"), "{}", ran.screen);
    assert!(
        ran.screen.ends_with("fzf 0.44.1 is older than 0.46, so efr shows the list\n"),
        "{}",
        ran.screen
    );
    assert_eq!(roots.recorded("cmdline"), "", "only --version ran");
}

#[test]
fn a_relative_entry_of_the_path_is_never_searched() {
    let roots = Roots::new();
    let seen = serve(&roots.socket());
    let (master, slave) = pty();
    // The fake fzf is only in a relative directory of the PATH; the other entry is
    // empty, so no fzf of the system can run either.
    let mut command = roots.resume(&slave, &[], "0.73.1", &[]);
    command.env("PATH", format!("bin:.:{}", roots.path("home").display()));
    let output = command.output().unwrap();
    let screen = bare(&screen_of(&master));

    assert!(output.status.success());
    assert!(screen.contains(" 1  rsync the photos to the laptop"), "{screen}");
    assert_eq!(methods(&seen.lock().unwrap()), [vec!["hello", "conversations.recent"]]);
    assert_eq!(roots.recorded("cmdline"), "");
}

// The picker with the real fzf.
//
// The `e2e_fzf_` tests run the built `efr resume` with the fzf of this machine against
// a `TestDaemon` whose index holds three conversations. They skip with a message
// unless `EFR_TEST_FZF=1`; with it, a missing fzf or one older than the floor fails.
// `just test-fzf` sets it and names the fzf in `EFR_TEST_FZF_BIN`.
//
// fzf reads its keys from `/dev/tty` and draws there, so efr runs as the leader of a
// session of its own with the pty as its controlling terminal (`setsid --ctty`). The
// test plays the person and the screen: it writes keys to the master, replays every
// byte on a vt100 screen, and answers fzf's question for the cursor position
// (`ESC [ 6 n`) from that screen, as a terminal does. `efr` gets a `PATH` of a
// temporary `bin` with a link to that fzf, then `/usr/bin` and `/bin`, never
// `~/.local/bin`.

/// The width and height of the pty: wide enough that the preview is on the right.
const E2E_COLS: u16 = 100;
const E2E_ROWS: u16 = 24;

/// The first column of the preview window at [`E2E_COLS`]: the list has the left half.
const PREVIEW_FROM: u16 = 50;

/// The longest a test waits for one change of the screen before it fails.
const E2E_WAIT: Duration = Duration::from_secs(30);

/// The oldest fzf that the picker takes (`picker::FZF_FLOOR`).
const FZF_FLOOR: (u32, u32, u32) = (0, 46, 0);

/// The terminal that the three conversations came from.
const OTHER_TTY: &str = "/dev/pts/71";

/// The three conversations of the index: the prompt and the answer of each, oldest
/// first.
const E2E_TURNS: [(&str, &str); 3] = [
    ("rsync the photos to the laptop", "the photos are on the laptop now"),
    ("docker compose for the blog", "the blog runs in two containers"),
    ("fix the flaky test in ci", "the test waited on a timer"),
];

/// The titles of [`E2E_TURNS`] as the recent list orders them: newest first.
const RECENT: [&str; 3] = [E2E_TURNS[2].0, E2E_TURNS[1].0, E2E_TURNS[0].0];

/// The fzf of this machine for the `e2e_fzf_` tests; `None`, with a message, when they
/// skip.
#[expect(clippy::print_stderr, reason = "a skipped test says why")]
fn real_fzf() -> Option<PathBuf> {
    if std::env::var_os("EFR_TEST_FZF").is_none_or(|value| value != "1") {
        eprintln!("skipped: set EFR_TEST_FZF=1 (or run just test-fzf) to run the tests with fzf");
        return None;
    }
    let fzf = std::env::var_os("EFR_TEST_FZF_BIN")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            let path = std::env::var_os("PATH")?;
            std::env::split_paths(&path)
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join("fzf"))
                .find(|candidate| candidate.is_file())
        })
        .expect("EFR_TEST_FZF=1, but no fzf in EFR_TEST_FZF_BIN or on the PATH");
    let output = efr_stdx::process::command(&fzf, "/")
        .into_std()
        .arg("--version")
        .env_remove("FZF_DEFAULT_OPTS")
        .env_remove("FZF_DEFAULT_OPTS_FILE")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut parts = text.split_whitespace().next().unwrap_or_default().split('.').map(|part| {
        part.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<u32>().ok()
    });
    let version = (
        parts.next().flatten().unwrap_or(0),
        parts.next().flatten().unwrap_or(0),
        parts.next().flatten().unwrap_or(0),
    );
    assert!(
        output.status.success() && version >= FZF_FLOOR,
        "EFR_TEST_FZF=1, but {} says {text:?}: older than 0.46, or no version",
        fzf.display()
    );
    Some(fzf)
}

/// What the screen showed: every byte, and the bytes replayed on a vt100 screen.
struct Shown {
    bytes: Vec<u8>,
    parser: vt100::Parser,
    /// True when the pty closed: every process on it ended.
    closed: bool,
}

/// The screen of the pty, which a thread fills from the master.
#[derive(Clone)]
struct Screen {
    shown: Arc<(Mutex<Shown>, Condvar)>,
}

impl Screen {
    /// Reads `master` until the pty closes, and answers each question for the cursor
    /// position from the screen.
    fn follow(master: OwnedFd) -> Screen {
        let shown = Shown {
            bytes: Vec::new(),
            parser: vt100::Parser::new(E2E_ROWS, E2E_COLS, 0),
            closed: false,
        };
        let screen = Screen { shown: Arc::new((Mutex::new(shown), Condvar::new())) };
        let filled = screen.clone();
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                // NOTE: the master reads EIO once no process holds the slave.
                let read = rustix::io::read(&master, &mut buffer).unwrap_or(0);
                let (lock, changed) = &*filled.shown;
                let mut shown = lock.lock().unwrap();
                if read == 0 {
                    shown.closed = true;
                    changed.notify_all();
                    return;
                }
                let bytes = &buffer[..read];
                shown.bytes.extend_from_slice(bytes);
                shown.parser.process(bytes);
                if bytes.windows(4).any(|window| window == b"\x1b[6n") {
                    let (row, col) = shown.parser.screen().cursor_position();
                    let answer = format!("\x1b[{};{}R", row + 1, col + 1);
                    let _ = rustix::io::write(&master, answer.as_bytes());
                }
                changed.notify_all();
            }
        });
        screen
    }

    /// Waits until `ready` gives a value for what the screen showed; fails after
    /// [`E2E_WAIT`] with the screen and `what`.
    fn wait<T>(&self, what: &str, mut ready: impl FnMut(&Shown) -> Option<T>) -> T {
        let (lock, changed) = &*self.shown;
        let mut shown = lock.lock().unwrap();
        let deadline = Instant::now() + E2E_WAIT;
        loop {
            if let Some(value) = ready(&shown) {
                return value;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero() && !shown.closed,
                "no {what} on the screen:\n{}",
                shown.parser.screen().contents()
            );
            shown = changed.wait_timeout(shown, left).unwrap().0;
        }
    }

    /// Waits until the pty closed, and returns the bytes and the last screen.
    fn closed(&self) -> (Vec<u8>, String) {
        let (lock, changed) = &*self.shown;
        let shown = lock.lock().unwrap();
        let (shown, timeout) =
            changed.wait_timeout_while(shown, E2E_WAIT, |shown| !shown.closed).unwrap();
        assert!(!timeout.timed_out(), "efr runs on:\n{}", shown.parser.screen().contents());
        (shown.bytes.clone(), shown.parser.screen().contents())
    }
}

/// The titles of [`E2E_TURNS`] in the list, the left half of the screen.
fn titles_in(shown: &Shown) -> Vec<&'static str> {
    let list: Vec<String> = shown.parser.screen().rows(0, PREVIEW_FROM).collect();
    list.iter()
        .filter_map(|row| {
            E2E_TURNS.iter().map(|(title, _)| *title).find(|title| row.contains(title))
        })
        .collect()
}

/// True when the screen shows the picker's header once, its prompt with `query`, and
/// the rows of `titles` in this order and no other.
fn shows(shown: &Shown, query: &str, titles: &[&str]) -> bool {
    let contents = shown.parser.screen().contents();
    contents.matches("enter resume").count() == 1
        && contents.contains(&format!("resume> {query}"))
        && titles_in(shown) == titles
}

/// For each row of the preview window with `word` and `plain`: whether the first
/// character of `word` has another look than the first of `plain`, which never matches.
fn marked_in_preview(shown: &Shown, word: &str, plain: &str) -> Vec<bool> {
    let screen = shown.parser.screen();
    let mut marks = Vec::new();
    for (row, text) in screen.rows(PREVIEW_FROM, E2E_COLS - PREVIEW_FROM).enumerate() {
        let (Some(at), Some(base)) = (text.find(word), text.find(plain)) else { continue };
        let row = u16::try_from(row).unwrap();
        let col = |byte: usize| PREVIEW_FROM + u16::try_from(text[..byte].chars().count()).unwrap();
        let (Some(cell), Some(base)) = (screen.cell(row, col(at)), screen.cell(row, col(base)))
        else {
            continue;
        };
        marks.push(cell.fgcolor() != base.fgcolor() || cell.bold() != base.bold());
    }
    marks
}

/// A persistent `TestDaemon` with `hook` in its searches, whose index holds the
/// conversations of [`E2E_TURNS`], each started from [`OTHER_TTY`].
async fn indexed_daemon(hook: Option<SearchRowHook>) -> (TestDaemon, ResponsesServer) {
    let server = ResponsesServer::start().await;
    for (_, answer) in E2E_TURNS {
        server.push(ResponsesAnswer::text(answer));
    }
    let mut builder = TestDaemon::builder().responses(&server).persistent();
    if let Some(hook) = hook {
        builder = builder.search_hook(hook);
    }
    let daemon = builder.start().await.unwrap();
    let client = daemon.client_for_tty(OTHER_TTY).await.unwrap();
    let mut newest = Seq::ZERO;
    for (n, (prompt, _)) in (1_u128..).zip(E2E_TURNS) {
        let Method::PromptSend(mut send) = daemon.prompt(n, prompt, OTHER_TTY) else {
            unreachable!("a prompt is a prompt.send")
        };
        send.new_conversation = true;
        let sent: PromptSendResult = client.call(Method::PromptSend(send)).await.unwrap();
        let mut events = daemon.follow(&client, sent.conversation_id).await.unwrap();
        let seen = events_until(&mut events, |event| matches!(event, Event::TurnCompleted { .. }))
            .await
            .unwrap();
        newest = newest.max(seen.last().unwrap().seq);
    }
    // The indexer waits its debounce after a turn's end on the daemon's clock.
    let clock = daemon.clock();
    let mut status = daemon.search_index();
    loop {
        let now = clock.now();
        let (indexed, sleeping) = {
            let seen = status
                .wait_for(|status| {
                    status.indexed_seq >= newest || status.sleeping_until.is_some_and(|at| at > now)
                })
                .await
                .unwrap();
            (seen.indexed_seq, seen.sleeping_until)
        };
        if indexed >= newest {
            break;
        }
        let until = sleeping.unwrap();
        clock.advance(Duration::try_from(until.duration_since(now)).unwrap());
    }
    (daemon, server)
}

/// One run of `efr resume` on a pty with the real fzf.
struct Picking {
    child: std::process::Child,
    master: OwnedFd,
    screen: Screen,
    _bin: tempfile::TempDir,
}

/// How a run of the picker ended: efr's status and the last screen.
type Ended = (ExitStatus, String);

impl Picking {
    /// Starts `efr resume` with the plugin's context and `vars` against `daemon`.
    fn start(daemon: &TestDaemon, fzf: &Path, vars: &[(&str, &str)]) -> Picking {
        let bin = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(fzf, bin.path().join("fzf")).unwrap();
        let (master, slave) = pty_of(E2E_ROWS, E2E_COLS);
        // NOTE: efrd moves a conversation only to the terminal that efr runs on.
        let tty = rustix::pty::ptsname(&master, Vec::new()).unwrap();
        let context = serde_json::json!({
            "pwd": daemon.cwd(),
            "tty": tty.to_str().unwrap(),
            "shell_pid": std::process::id(),
        });
        let dirs = daemon.dirs().dirs();
        let mut command = efr_stdx::process::command("/usr/bin/setsid", daemon.cwd()).into_std();
        command
            .args(["--ctty", "--wait"])
            .arg(env!("CARGO_BIN_EXE_efr"))
            .arg("resume")
            .env_clear()
            .env("HOME", daemon.dirs().home())
            .env("PATH", format!("{}:/usr/bin:/bin", bin.path().display()))
            .env("TERM", "xterm-256color")
            .env("SHELL", "/bin/zsh")
            .env("EFR_CONFIG_DIR", dirs.config())
            .env("EFR_DATA_DIR", dirs.data())
            .env("EFR_STATE_DIR", dirs.state())
            .env("EFR_RUNTIME_DIR", dirs.runtime())
            .env("EFR_CONTEXT", context.to_string())
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave.try_clone().unwrap());
        for (name, value) in vars {
            command.env(name, value);
        }
        let child = command.spawn().unwrap();
        // NOTE: the pty closes only when no process holds the slave, this one included.
        drop(command);
        drop(slave);
        let screen = Screen::follow(master.try_clone().unwrap());
        Picking { child, master, screen, _bin: bin }
    }

    /// Types `keys` as a person does.
    fn keys(&self, keys: &str) {
        rustix::io::write(&self.master, keys.as_bytes()).unwrap();
    }

    /// Waits until efr reads single keys: the terminal is out of its canonical mode.
    fn until_keys_read(&self) {
        let deadline = Instant::now() + E2E_WAIT;
        loop {
            let modes = rustix::termios::tcgetattr(&self.master).unwrap().local_modes;
            if !modes.contains(rustix::termios::LocalModes::ICANON) {
                return;
            }
            assert!(Instant::now() < deadline, "efr never read a key");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Pastes `text`, which fzf takes as one change of its query.
    fn paste(&self, text: &str) {
        self.keys(&format!("\x1b[200~{text}\x1b[201~"));
    }

    /// Waits until the picker shows the recent rows with no query.
    fn recent(&self) {
        self.screen.wait("the recent rows", |shown| shows(shown, "", &RECENT).then_some(()));
    }

    /// Waits until the picker shows the rows of `titles` for `query`.
    fn rows(&self, query: &str, titles: &[&str]) {
        self.screen.wait(&format!("the rows of {query:?}"), |shown| {
            shows(shown, query, titles).then_some(())
        });
    }

    /// Waits for the end of efr, and checks that fzf left no trace: the terminal is in
    /// its cooked mode, no byte switched to the alternate screen, and no row of the
    /// picker stays.
    fn end(mut self) -> Ended {
        let (bytes, screen) = self.screen.closed();
        let status = self.child.wait().unwrap();
        let modes = rustix::termios::tcgetattr(&self.master).unwrap().local_modes;
        let cooked = rustix::termios::LocalModes::ICANON | rustix::termios::LocalModes::ECHO;
        assert!(modes.contains(cooked), "the terminal is not cooked again: {modes:?}");
        let alternate = bytes.windows(8).any(|window| window == b"\x1b[?1049h");
        assert!(!alternate, "fzf drew on the alternate screen");
        assert!(!screen.contains("resume>"), "the prompt of the picker stays:\n{screen}");
        assert!(!screen.contains("enter resume"), "the header stays:\n{screen}");
        (status, screen)
    }
}

/// Runs `steps` against a picker of `daemon`, off the runtime's workers, which must
/// keep serving the daemon.
async fn pick<T: Send + 'static>(
    daemon: &TestDaemon,
    fzf: &Path,
    vars: &[(&str, &str)],
    steps: impl FnOnce(Picking) -> T + Send + 'static,
) -> T {
    let picking = Picking::start(daemon, fzf, vars);
    tokio::task::spawn_blocking(move || steps(picking)).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_fzf_enter_resumes_the_row_of_the_query() {
    let Some(fzf) = real_fzf() else { return };
    let (daemon, _server) = indexed_daemon(None).await;

    let (status, screen) = pick(&daemon, &fzf, &[], |picking| {
        picking.recent();
        // The header comes from each reload (`--header-lines=1`): it shows once, never
        // as a row.
        picking.keys("rsync");
        picking.rows("rsync", &[E2E_TURNS[0].0]);
        picking.keys("\r");
        picking.end()
    })
    .await;

    assert!(status.success(), "{status:?}\n{screen}");
    assert!(screen.contains("resumed \"rsync the photos to the laptop\""), "{screen}");
    assert!(!screen.contains("docker"), "no other row stays:\n{screen}");
    daemon.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_fzf_ctrl_o_prints_the_conversation_and_resumes_nothing() {
    let Some(fzf) = real_fzf() else { return };
    let (daemon, _server) = indexed_daemon(None).await;

    let (status, screen) = pick(&daemon, &fzf, &[], |picking| {
        picking.recent();
        picking.keys("rsync");
        picking.rows("rsync", &[E2E_TURNS[0].0]);
        picking.keys("\x0f");
        picking.end()
    })
    .await;

    assert!(status.success(), "{status:?}\n{screen}");
    assert!(screen.contains("the photos are on the laptop now"), "the transcript:\n{screen}");
    assert!(!screen.contains("resumed"), "no resume:\n{screen}");
    daemon.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_fzf_esc_closes_with_no_line_and_ctrl_c_exits_130() {
    let Some(fzf) = real_fzf() else { return };
    let (daemon, _server) = indexed_daemon(None).await;

    // NOTE: with --expect=esc, Esc ends fzf with exit 0, apart from Ctrl+C (130).
    for (key, code) in [("\x1b", 0), ("\x03", 130)] {
        let (status, screen) = pick(&daemon, &fzf, &[], move |picking| {
            picking.recent();
            picking.keys(key);
            picking.end()
        })
        .await;

        assert_eq!(status.code(), Some(code), "{key:?}\n{screen}");
        assert_eq!(screen.trim(), "", "{key:?} leaves nothing on the screen");
    }
    daemon.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_fzf_words_of_resume_are_the_first_query() {
    let Some(fzf) = real_fzf() else { return };
    let (daemon, _server) = indexed_daemon(None).await;

    let (status, screen) = pick(&daemon, &fzf, &[("EFR_RESUME_WORDS", "laptop")], |picking| {
        picking.rows("laptop", &[E2E_TURNS[0].0]);
        picking.keys("\x1b");
        picking.end()
    })
    .await;

    assert!(status.success(), "{status:?}\n{screen}");
    daemon.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_fzf_preview_marks_follow_the_query_on_the_same_row() {
    let Some(fzf) = real_fzf() else { return };
    let (daemon, _server) = indexed_daemon(None).await;

    let (status, screen) = pick(&daemon, &fzf, &[], |picking| {
        picking.recent();
        picking.keys("rsync");
        picking.screen.wait("a preview with laptop not marked", |shown| {
            let marks = marked_in_preview(shown, "laptop", "photos");
            (shows(shown, "rsync", &[E2E_TURNS[0].0])
                && !marks.is_empty()
                && marks.iter().all(|marked| !marked))
            .then_some(())
        });
        // The same row keeps the focus; fzf runs its preview again after the reload.
        picking.keys(" lap");
        picking.screen.wait("laptop marked in the preview", |shown| {
            let marks = marked_in_preview(shown, "laptop", "photos");
            (shows(shown, "rsync lap", &[E2E_TURNS[0].0]) && marks.iter().any(|marked| *marked))
                .then_some(())
        });
        picking.keys("\x1b");
        picking.end()
    })
    .await;

    assert!(status.success(), "{status:?}\n{screen}");
    daemon.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_fzf_list_keeps_its_rows_until_the_reload_ends() {
    let Some(fzf) = real_fzf() else { return };
    // Once the test arms it, the next search that reaches a row waits in the row loop
    // until the test lets it go on.
    let armed = Arc::new(AtomicBool::new(false));
    let (reached_tx, reached) = std::sync::mpsc::channel::<()>();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    let hook = {
        let armed = Arc::clone(&armed);
        SearchRowHook::new(
            1,
            Arc::new(move |_rows| {
                if armed.swap(false, Ordering::SeqCst) {
                    let _ = reached_tx.send(());
                    // NOTE: a test that ends drops the sender, which ends the wait too.
                    let _ = release_rx.lock().unwrap().recv();
                }
            }),
        )
    };
    let (daemon, _server) = indexed_daemon(Some(hook)).await;

    let (status, screen) = pick(&daemon, &fzf, &[], move |picking| {
        picking.recent();
        armed.store(true, Ordering::SeqCst);
        picking.paste("docker");
        reached.recv_timeout(E2E_WAIT).expect("the reload of docker searched");
        // reload-sync: while the search runs, the old rows stay under the new query.
        picking.rows("docker", &RECENT);
        release.send(()).unwrap();
        picking.rows("docker", &[E2E_TURNS[1].0]);
        picking.keys("\x1b");
        picking.end()
    })
    .await;

    assert!(status.success(), "{status:?}\n{screen}");
    daemon.stop().await.unwrap();
}

/// Reports the time from a key to its new rows on the screen, for 20 changes of the
/// query: a paste of `docker` and Ctrl-U in turn. Report only: the budget (120 ms at
/// p95 on corpus L) is for a real store, by hand in live step Q4.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_fzf_key_to_rows_times() {
    let Some(fzf) = real_fzf() else { return };
    let (daemon, _server) = indexed_daemon(None).await;

    let (times, (status, screen)) = pick(&daemon, &fzf, &[], |picking| {
        picking.recent();
        let mut times = Vec::new();
        for n in 0..20 {
            let started = Instant::now();
            if n % 2 == 0 {
                picking.paste("docker");
                picking.rows("docker", &[E2E_TURNS[1].0]);
            } else {
                picking.keys("\x15");
                picking.recent();
            }
            times.push(started.elapsed());
        }
        picking.keys("\x1b");
        (times, picking.end())
    })
    .await;

    assert!(status.success(), "{status:?}\n{screen}");
    let mut sorted = times;
    sorted.sort();
    let at = |share: usize| sorted[(sorted.len() * share).div_ceil(100).saturating_sub(1)];
    let ms = |time: Duration| time.as_secs_f64() * 1000.0;
    writeln!(
        std::io::stdout(),
        "e2e_fzf: a key to its new rows, 20 keys: p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms \
         (with the wait of 50 ms of efr search --picker)",
        ms(at(50)),
        ms(at(95)),
        ms(sorted[sorted.len() - 1]),
    )
    .unwrap();
    daemon.stop().await.unwrap();
}

/// Runs the built `efr` with `args` against `daemon`, without a terminal and with no
/// variable of the real machine; its status and stdout.
fn efr_by_hand(daemon: &TestDaemon, args: &[&str]) -> (ExitStatus, String) {
    let dirs = daemon.dirs().dirs();
    let output = efr_stdx::process::command(env!("CARGO_BIN_EXE_efr"), daemon.cwd())
        .into_std()
        .args(args)
        .env_clear()
        .env("HOME", daemon.dirs().home())
        .env("PATH", "/usr/bin:/bin")
        .env("EFR_CONFIG_DIR", dirs.config())
        .env("EFR_DATA_DIR", dirs.data())
        .env("EFR_STATE_DIR", dirs.state())
        .env("EFR_RUNTIME_DIR", dirs.runtime())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    (output.status, format!("{}{stderr}", String::from_utf8_lossy(&output.stdout)))
}

/// The id of the conversation whose first prompt is `title`, from the index.
async fn id_of(daemon: &TestDaemon, title: &str) -> efr_protocol::ConversationId {
    let client = daemon.client().await.unwrap();
    let method = Method::ConversationsSearch(efr_protocol::ConversationsSearch {
        query: title.to_owned(),
        limit: Some(5),
        ..Default::default()
    });
    let found: ConversationsSearchResult = client.call(method).await.unwrap();
    found.rows[0].conversation.summary.id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_by_hand_and_efr_search_finds_none_of_its_text() {
    let (daemon, _server) = indexed_daemon(None).await;
    let id = id_of(&daemon, E2E_TURNS[0].0).await;
    let (status, found) =
        tokio::task::block_in_place(|| efr_by_hand(&daemon, &["search", "rsync"]));
    assert!(status.success() && found.contains(E2E_TURNS[0].0), "{found}");

    // NOTE: the ids of one test share their first parts, which hold the time.
    let start = id.to_string()[..30].to_owned();
    let (status, out) = tokio::task::block_in_place(|| {
        efr_by_hand(&daemon, &["conversation", "delete", "--yes", &start])
    });

    assert!(status.success(), "{status:?} {out}");
    assert_eq!(out, format!("deleted \"{}\"\n", E2E_TURNS[0].0));
    for words in ["rsync", "laptop", "photos"] {
        let (status, found) =
            tokio::task::block_in_place(|| efr_by_hand(&daemon, &["search", words]));
        assert!(status.success());
        assert_eq!(found, "no conversation matches\n", "{words}");
    }
    let (_, found) = tokio::task::block_in_place(|| efr_by_hand(&daemon, &["search", "docker"]));
    assert!(found.contains(E2E_TURNS[1].0), "{found}");
    daemon.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_fzf_ctrl_d_asks_deletes_and_opens_the_picker_again_with_its_query() {
    let Some(fzf) = real_fzf() else { return };
    let (daemon, _server) = indexed_daemon(None).await;

    let (status, screen) = pick(&daemon, &fzf, &[], |picking| {
        picking.recent();
        picking.keys("rsync");
        picking.rows("rsync", &[E2E_TURNS[0].0]);
        picking.keys("\x04");
        picking.screen.wait("the question of the delete", |shown| {
            let contents = shown.parser.screen().contents();
            (contents.contains("Nothing takes a delete back. [y/N]")
                && !contents.contains("enter resume"))
            .then_some(())
        });
        // NOTE: efr throws away the keys typed before it reads the answer, as for every
        // question; its key thread leaves the canonical mode after that.
        picking.until_keys_read();
        picking.keys("y");
        // The picker opens again below the lines of the delete, with its query and no
        // row (the lines above name the title, so the rows count, not the titles).
        let counted = |query: &'static str, count: &'static str| {
            move |shown: &Shown| {
                let contents = shown.parser.screen().contents();
                (contents.contains("deleted \"rsync the photos to the laptop\"")
                    && contents.contains(&format!("resume> {query}"))
                    && contents.contains(count))
                .then_some(())
            }
        };
        picking.screen.wait("the picker again with rsync", counted("rsync", "0/0"));
        picking.keys("\x15");
        picking.screen.wait("the two other rows", counted("", "2/2"));
        picking.keys("\x1b");
        picking.end()
    })
    .await;

    assert!(status.success(), "{status:?}\n{screen}");
    assert!(screen.contains("deleted \"rsync the photos to the laptop\""), "{screen}");
    let (_, found) = tokio::task::block_in_place(|| efr_by_hand(&daemon, &["search", "rsync"]));
    assert_eq!(found, "no conversation matches\n");
    daemon.stop().await.unwrap();
}

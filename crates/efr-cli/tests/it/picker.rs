//! The picker of `efr resume` with the built `efr` on a pseudo-terminal, a fake daemon
//! and a fake `fzf`: the command line and the environment that fzf gets, the reload
//! that it runs on a key, and each way that fzf can end.
//!
//! The fake `fzf` is a `sh` script in a temporary `bin` directory, first on a `PATH`
//! that never holds `~/.local/bin`. `--version` prints the version that the test
//! chooses. Otherwise it records its command line, its environment and its stdin, runs
//! the command of its `reload-sync` action with `FZF_QUERY` set as fzf does, then
//! prints the key that the test chooses and the first row of the reload, as fzf prints
//! them with `--expect`, and exits with the status that the test chooses.
//!
//! `efr` gets a cleared environment with every efr root in a temporary directory, so it
//! never reaches the real daemon.

use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};

use efr_protocol::framing::{self, Decoder};
use efr_protocol::{
    Capabilities, ClientFrame, ConversationHistoryResult, ConversationResumeResult,
    ConversationStatus, ConversationSummary, ConversationsListResult, ConversationsRecentResult,
    ConversationsSearchResult, DaemonPaths, HelloResult, Method, PROTOCOL_VERSION,
    RecentConversation, RequestId, RouteLabel, RouteScope, SearchRow, Seq, ServerFrame,
};
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
printf '%s\n%s\n' "$FAKE_FZF_KEY" "$row"
exit "${FAKE_FZF_EXIT:-0}"
"#;

/// A pseudo-terminal pair: the master, which plays the screen, and the slave, which
/// `efr` gets as stdin and stdout.
pub(crate) fn pty() -> (OwnedFd, OwnedFd) {
    use rustix::fs::{Mode, OFlags};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC).unwrap();
    grantpt(&master).unwrap();
    unlockpt(&master).unwrap();
    let name = ptsname(&master, Vec::new()).unwrap();
    let slave =
        rustix::fs::open(name.as_c_str(), OFlags::RDWR | OFlags::NOCTTY, Mode::empty()).unwrap();
    let size = rustix::termios::Winsize { ws_row: 24, ws_col: 100, ws_xpixel: 0, ws_ypixel: 0 };
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

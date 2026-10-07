//! The built `efr` binary: exit codes, the environment it reads, and a whole prompt
//! against a fake daemon on a socket in a temporary directory.
//!
//! The process gets a cleared environment with every efr root in a temporary
//! directory, so it never sees the real home, config or runtime directory. The fake
//! daemon speaks `efr_protocol::framing` on a plain thread; `efr-cli` may not use
//! `efr-transport`, not even here.

use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use assert_cmd::Command;
use efr_protocol::framing::{self, Decoder};
use efr_protocol::{
    AdminStatusResult, Capabilities, ClientFrame, ConversationSubscribeItem, DaemonPaths,
    EffectiveSettings, ErrorBody, ErrorCode, Event, EventEnvelope, HelloResult, Method, Mode,
    ModelInfo, ModelSource, ModelsListResult, OverriddenSettings, PROTOCOL_VERSION,
    PromptSendResult, RequestId, Seq, ServerFrame, TurnSettings,
};
use serde::Serialize;

const CONVERSATION: &str = "019a9b1c-3d00-7a10-8b20-000000000001";
const TURN: &str = "019a9b1c-3d00-7a10-8b20-000000000002";

/// Temporary efr roots.
struct Roots {
    dir: tempfile::TempDir,
}

impl Roots {
    fn new() -> Roots {
        let dir = tempfile::tempdir().unwrap();
        for name in ["home", "config", "data", "state", "runtime"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        Roots { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn socket(&self) -> PathBuf {
        self.path("runtime").join("daemon.sock")
    }

    /// `efr` with only the variables a test gives it.
    fn efr(&self) -> Command {
        Command::from_std(self.std_efr())
    }

    /// [`Roots::efr`] as a standard command, which a test can start and signal.
    fn std_efr(&self) -> std::process::Command {
        let mut command =
            efr_stdx::process::command(env!("CARGO_BIN_EXE_efr"), self.dir.path()).into_std();
        command
            .env_clear()
            .env("HOME", self.path("home"))
            .env("EFR_CONFIG_DIR", self.path("config"))
            .env("EFR_DATA_DIR", self.path("data"))
            .env("EFR_STATE_DIR", self.path("state"))
            .env("EFR_RUNTIME_DIR", self.path("runtime"));
        command
    }

    /// A fake daemon that accepts one connection, answers its hello and runs `script`.
    fn serve(&self, script: impl FnOnce(&mut Conn) + Send + 'static) -> JoinHandle<()> {
        let listener = UnixListener::bind(self.socket()).unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut conn = Conn { stream, decoder: Decoder::new(), ready: VecDeque::new() };
            let (id, method) = conn.request();
            assert!(matches!(method, Method::Hello(_)), "hello first, got {}", method.name());
            conn.reply(id, &hello_result());
            script(&mut conn);
        })
    }
}

fn hello_result() -> HelloResult {
    HelloResult {
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        protocol: PROTOCOL_VERSION,
        version: "0.1.0".to_owned(),
        capabilities: Capabilities::default(),
        paths: DaemonPaths::default(),
        challenge: "AAAA".to_owned(),
    }
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

    fn request(&mut self) -> (RequestId, Method) {
        match self.recv() {
            Some(ClientFrame::Request { id, method }) => (id, method),
            other => panic!("expected a request, got {other:?}"),
        }
    }

    fn send(&mut self, frame: &ServerFrame) {
        self.stream.write_all(&framing::encode(frame).unwrap()).unwrap();
    }

    fn item<T: Serialize>(&mut self, id: RequestId, value: &T) {
        self.send(&ServerFrame::item(id, value).unwrap());
    }

    fn reply<T: Serialize>(&mut self, id: RequestId, value: &T) {
        self.item(id, value);
        self.send(&ServerFrame::end(id));
    }

    /// Waits until efr closes the connection.
    fn drain(&mut self) {
        while self.recv().is_some() {}
    }
}

fn event(seq: u64, event: Event) -> ConversationSubscribeItem {
    ConversationSubscribeItem::Event(EventEnvelope {
        seq: Seq::new(seq),
        conversation_id: Some(CONVERSATION.parse().unwrap()),
        at: "2026-10-04T12:00:00Z".parse().unwrap(),
        event,
    })
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn help_exits_zero_on_stdout() {
    let roots = Roots::new();
    let output = roots.efr().arg("--help").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout_of(&output).contains("Usage: efr <COMMAND>"));
}

#[test]
fn an_unknown_command_exits_two_on_stderr() {
    let roots = Roots::new();
    let output = roots.efr().arg("frobnicate").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr_of(&output).contains("unrecognized subcommand 'frobnicate'"));
    assert_eq!(stdout_of(&output), "");
}

#[test]
fn an_empty_prompt_exits_two() {
    let roots = Roots::new();
    let output = roots.efr().args(["send", "--"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stderr_of(&output), "efr: the prompt is empty\n");
}

#[test]
fn without_a_daemon_status_exits_three_with_the_hint() {
    let roots = Roots::new();
    let output = roots.efr().arg("status").output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    let stderr = stderr_of(&output);
    assert!(stderr.contains("no daemon is listening on"), "{stderr}");
    assert!(stderr.ends_with("efr: start the daemon with: systemctl --user start efrd, or `just run` in the efr checkout for a foreground one\n"));
}

#[test]
fn status_prints_the_daemons_health() {
    let roots = Roots::new();
    let daemon = roots.serve(|conn| {
        let (id, method) = conn.request();
        assert!(matches!(method, Method::AdminStatus(_)));
        conn.reply(
            id,
            &AdminStatusResult {
                daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
                version: "0.1.0".to_owned(),
                protocol: PROTOCOL_VERSION,
                pid: 4242,
                started_at: "2026-10-04T11:00:00Z".parse().unwrap(),
                screen_backend: "vt100".to_owned(),
                conversations: 1,
                shells: 1,
                providers: Vec::new(),
                roots: None,
                config: None,
                sandbox: None,
                sandbox_paths: None,
            },
        );
        conn.drain();
    });
    let output = roots.efr().arg("status").output().unwrap();
    daemon.join().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    let stdout = stdout_of(&output);
    assert!(stdout.starts_with("daemon         efrd 0.1.0, protocol "), "{stdout}");
    assert!(stdout.contains(&format!("socket         {}\n", roots.socket().display())));
}

/// A daemon that takes one prompt and answers it with `reply` or fails the turn.
fn answering(roots: &Roots, outcome: Result<&'static str, &'static str>) -> JoinHandle<()> {
    roots.serve(move |conn| {
        let (id, method) = conn.request();
        let Method::PromptSend(params) = method else { panic!("expected prompt.send") };
        assert_eq!(params.text, "why so slow?");
        assert_eq!(params.last_command.as_deref(), Some("cargo build"));
        let turn_id = TURN.parse().unwrap();
        conn.reply(
            id,
            &PromptSendResult {
                conversation_id: CONVERSATION.parse().unwrap(),
                turn_id,
                seq: Seq::new(3),
                queued: false,
                settings: None,
            },
        );
        let (sub, _) = conn.request();
        match outcome {
            Ok(reply) => {
                let text = reply.to_owned();
                conn.item(
                    sub,
                    &event(4, Event::AssistantMessageCompleted { turn_id, index: 0, text }),
                );
                conn.item(
                    sub,
                    &event(5, Event::TurnCompleted { turn_id, usage: None, changes: None }),
                );
            }
            Err(message) => {
                let error = ErrorBody::new(ErrorCode::Internal, message);
                conn.item(sub, &event(4, Event::TurnFailed { turn_id, error }));
            }
        }
        conn.drain();
    })
}

#[test]
fn a_piped_reply_is_raw_markdown() {
    let roots = Roots::new();
    let daemon = answering(&roots, Ok("Use **sccache**."));
    let output = roots
        .efr()
        .args(["send", "--context-json", r#"{"pwd":"/srv"}"#, "--last-command", "cargo build"])
        .args(["--", "why", "so", "slow?"])
        .output()
        .unwrap();
    daemon.join().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    assert_eq!(stdout_of(&output), "Use **sccache**.\n");
}

#[test]
fn the_plugins_variables_carry_the_prompt_without_arguments() {
    let roots = Roots::new();
    let daemon = answering(&roots, Ok("Use **sccache**."));
    let output = roots
        .efr()
        .arg("send")
        .env("EFR_CONTEXT", r#"{"pwd":"/srv","tty":"/dev/pts/4"}"#)
        .env("EFR_LAST_COMMAND", "cargo build")
        .env("EFR_PROMPT", "why so slow?")
        .output()
        .unwrap();
    daemon.join().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    assert_eq!(stdout_of(&output), "Use **sccache**.\n");
}

#[test]
fn the_terminals_settings_reach_the_prompt_and_the_overrides_lead_the_reply() {
    let roots = Roots::new();
    let daemon = roots.serve(|conn| {
        let (id, method) = conn.request();
        let Method::PromptSend(params) = method else { panic!("expected prompt.send") };
        assert_eq!(
            params.settings,
            TurnSettings {
                mode: Some(Mode::Auto),
                model: Some("gpt-5.4".to_owned()),
                effort: Some("high".to_owned()),
            }
        );
        let turn_id = TURN.parse().unwrap();
        let settings = EffectiveSettings {
            mode: Mode::Auto,
            model: "gpt-5.4".to_owned(),
            effort: Some("high".to_owned()),
            overridden: OverriddenSettings { mode: true, model: true, effort: false },
            fallback: None,
        };
        conn.reply(
            id,
            &PromptSendResult {
                conversation_id: CONVERSATION.parse().unwrap(),
                turn_id,
                seq: Seq::new(3),
                queued: false,
                settings: Some(settings),
            },
        );
        let (sub, _) = conn.request();
        let text = "Done.".to_owned();
        conn.item(sub, &event(4, Event::AssistantMessageCompleted { turn_id, index: 0, text }));
        conn.item(sub, &event(5, Event::TurnCompleted { turn_id, usage: None, changes: None }));
        conn.drain();
    });
    let output = roots
        .efr()
        .arg("send")
        .env("EFR_PROMPT", "tidy up")
        .env("EFR_MODE", "auto")
        .env("EFR_MODEL", "gpt-5.4")
        .args(["--effort", "high"])
        .output()
        .unwrap();
    daemon.join().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    assert_eq!(stdout_of(&output), "Done.\n");
    // The effort came from the config in the daemon's view, so the note leaves it out.
    assert_eq!(stderr_of(&output), "mode auto, model gpt-5.4\n");
}

#[test]
fn an_unknown_mode_in_the_variable_exits_two_before_any_connection() {
    let roots = Roots::new();
    // No daemon listens: a connection would exit with three.
    let output = roots.efr().arg("send").env("EFR_PROMPT", "hi").env("EFR_MODE", "yolo").output();
    let output = output.unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        stderr_of(&output),
        "efr: EFR_MODE names the mode \"yolo\", which does not exist; choose one of: manual, \
         cautious, auto\n"
    );
}

/// A daemon that answers `models.list` with a default model and one other.
fn listing_models(roots: &Roots) -> JoinHandle<()> {
    roots.serve(|conn| {
        let (id, method) = conn.request();
        assert!(matches!(method, Method::ModelsList(_)), "{}", method.name());
        let model = |id: &str, default| ModelInfo {
            id: id.to_owned(),
            efforts: vec!["low".to_owned(), "medium".to_owned(), "high".to_owned()],
            default_effort: Some("medium".to_owned()),
            default,
            source: ModelSource::Builtin,
        };
        conn.reply(
            id,
            &ModelsListResult { models: vec![model("gpt-5.5", true), model("gpt-5.4", false)] },
        );
        conn.drain();
    })
}

#[test]
fn settings_reads_the_config_file_and_the_variables() {
    let roots = Roots::new();
    let config = roots.path("config").join("config.toml");
    std::fs::write(&config, "[permissions]\nmode = \"manual\"\n[model]\neffort = \"low\"\n")
        .unwrap();
    let daemon = listing_models(&roots);
    let output = roots.efr().arg("settings").env("EFR_MODEL", "gpt-5.4").output().unwrap();
    daemon.join().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    assert_eq!(
        stdout_of(&output),
        format!(
            "mode = manual  # {path}; choices: manual, cautious, auto\n\
             model = gpt-5.4  # EFR_MODEL; choices: gpt-5.5, gpt-5.4\n\
             effort = low  # {path}; choices: low, medium, high\n",
            path = config.display()
        )
    );
}

#[test]
fn settings_refuses_an_effort_the_model_does_not_take_with_two() {
    let roots = Roots::new();
    let daemon = listing_models(&roots);
    let output = roots.efr().args(["settings", "--effort=max"]).output().unwrap();
    daemon.join().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stdout_of(&output), "");
    assert_eq!(
        stderr_of(&output),
        "efr: gpt-5.5 does not take the effort \"max\" (--effort); choose one of: low, medium, \
         high\n"
    );
}

#[test]
fn a_failed_turn_exits_one_with_the_reason() {
    let roots = Roots::new();
    let daemon = answering(&roots, Err("the provider is down"));
    let output = roots
        .efr()
        .args(["send", "--last-command", "cargo build", "--", "why", "so", "slow?"])
        .output()
        .unwrap();
    daemon.join().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr_of(&output), "efr: the turn failed with internal: the provider is down\n");
}

#[test]
fn sigterm_while_a_turn_runs_ends_efr_by_the_signal() {
    let roots = Roots::new();
    let (subscribed, waits) = std::sync::mpsc::channel();
    let daemon = roots.serve(move |conn| {
        let (id, _) = conn.request();
        let turn_id = TURN.parse().unwrap();
        conn.reply(
            id,
            &PromptSendResult {
                conversation_id: CONVERSATION.parse().unwrap(),
                turn_id,
                seq: Seq::new(3),
                queued: false,
                settings: None,
            },
        );
        let (sub, _) = conn.request();
        conn.item(
            sub,
            &event(
                4,
                Event::TurnStarted {
                    turn_id,
                    cwd: PathBuf::from("/srv"),
                    scope: efr_protocol::Scope::Machine,
                    settings: None,
                },
            ),
        );
        subscribed.send(()).unwrap();
        conn.drain();
    });
    let child = roots
        .std_efr()
        .args(["send", "--", "why", "so", "slow?"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    waits.recv().unwrap();
    let pid = rustix::process::Pid::from_child(&child);
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    let output = child.wait_with_output().unwrap();
    daemon.join().unwrap();
    // The follow loop took the signal, wrote its last frame, and then let the signal
    // take its default action, so the shell sees SIGTERM and no message.
    assert_eq!(output.status.signal(), Some(15), "{:?}", output.status);
    assert_eq!(stderr_of(&output), "");
}

#[test]
fn config_show_reads_the_environment_and_the_config_file() {
    let roots = Roots::new();
    std::fs::write(roots.path("config").join("config.toml"), "[render]\ntheme = \"nord\"\n")
        .unwrap();
    let output = roots
        .efr()
        .args(["config", "show"])
        .env("NO_COLOR", "1")
        .env("EFR_OPEN_BROWSER", "on")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    let stdout = stdout_of(&output);
    let config_file = Path::new(&roots.path("config")).join("config.toml");
    assert!(
        stdout.contains(&format!("theme = \"nord\"  # {}\n", config_file.display())),
        "{stdout}"
    );
    assert!(stdout.contains("colour = \"none\"  # NO_COLOR is set\n"), "{stdout}");
    assert!(stdout.contains("formatted = false  # stdout is not a terminal; raw markdown\n"));
    assert!(stdout.contains("open_browser = true  # EFR_OPEN_BROWSER\n"));
    assert!(stdout.contains("runtime = "), "{stdout}");
    assert!(stdout.contains("  # EFR_RUNTIME_DIR\n"), "{stdout}");
}

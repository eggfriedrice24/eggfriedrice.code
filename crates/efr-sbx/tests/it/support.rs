//! The fixture of the real-bwrap tests: a fake home with a project, efr's roots, a call
//! dir per call, and the launcher from `EFR_TEST_SBX_BIN`.
//!
//! Every test calls [`sandbox`] first. Without `EFR_TEST_SBX_BIN`, or when the probe
//! says the sandbox cannot run here, it prints `skipped: <reason>` and the test passes;
//! with `EFR_TEST_SBX_REQUIRE=1` (set by `just test-sandbox` once its own probe said
//! ready) a skip fails the test instead. Nothing here touches the user's real efr dirs:
//! every root lies in a temp dir below the target dir, never below `/tmp`, which the
//! sandbox replaces with its private tmp.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use efr_protocol::{CacheMode, CallId, ConversationId, Grant};
use efr_sandbox::{
    CacheOverlay, EnvPlan, Floor, FloorKind, LINE_FILE, Mask, MaskKind, NONCE_FILE, NetworkPlan,
    ProbeReport, RESULT_FILE, RecordLimits, RuntimePaths, SPEC_FILE, SPEC_VERSION, SandboxResult,
    SandboxSpec, SpecLaunch, WriteRoot, WriteRootKind,
};

/// The child script every test call runs: efr-shell's own, which efrd installs, so the
/// suite tests the script that ships. Only the file is shared; there is no crate edge.
const CHILD_SCRIPT: &str = include_str!("../../../efr-shell/assets/zsh/efr-child.zsh");
/// The editor stub that efr-shell installs.
const EDITOR: &str = include_str!("../../../efr-shell/assets/efr-editor");

/// The default promote list of `sandbox.promote_env`.
const PROMOTE: &[&str] = &[
    "RUST_LOG",
    "RUST_BACKTRACE",
    "NODE_ENV",
    "DEBUG",
    "CI",
    "NO_COLOR",
    "FORCE_COLOR",
    "TZ",
    "LANG",
    "LC_*",
];

/// How long one call may take before the test kills it.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// A command for `program`; each caller sets the environment it needs.
#[expect(
    clippy::disallowed_methods,
    reason = "the tests run blocking processes; efr_stdx::process::command is tokio's"
)]
pub(crate) fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    Command::new(program)
}

/// The test's own environment variable `name`.
#[expect(
    clippy::disallowed_methods,
    reason = "the tests read their own PATH and the EFR_TEST_ switches"
)]
pub(crate) fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Prints one line of the test's report, such as `skipped: <reason>`.
pub(crate) fn say(text: &str) {
    let _ = writeln!(std::io::stdout(), "{text}");
}

/// The launcher and the facts of the probe.
#[derive(Debug, Clone)]
pub(crate) struct Ready {
    pub(crate) bin: PathBuf,
    pub(crate) bwrap: PathBuf,
}

/// The launcher, or why the real-bwrap tests skip.
fn ready() -> Result<Ready, String> {
    let bin = env_var("EFR_TEST_SBX_BIN")
        .map(PathBuf::from)
        .ok_or("EFR_TEST_SBX_BIN is not set; just test-sandbox sets it")?;
    let dir = target_tmp().join("probe");
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| e.to_string())?;
    let output = command(&bin)
        .args(["probe", "--json", "--dir"])
        .arg(&dir)
        .env_remove("EFR_TEST_SBX_REQUIRE")
        .output()
        .map_err(|error| format!("efr-sbx probe did not run: {error}"))?;
    let report = ProbeReport::from_json(&output.stdout)
        .map_err(|error| format!("efr-sbx probe printed no report: {error}"))?;
    let status = report.status();
    if let Some(reason) = status.reason {
        return Err(format!("the sandbox is unavailable: {reason}"));
    }
    let bwrap = status.bwrap.ok_or("the probe found no bwrap")?;
    Ok(Ready { bin, bwrap })
}

/// The launcher when the real-bwrap tests can run; prints the reason and returns
/// `None` otherwise.
pub(crate) fn sandbox() -> Option<Ready> {
    match ready() {
        Ok(ready) => Some(ready),
        Err(reason) => {
            let required = env_var("EFR_TEST_SBX_REQUIRE").is_some_and(|value| value == "1");
            assert!(!required, "the sandbox tests must run here, but: {reason}");
            say(&format!("skipped: {reason}"));
            None
        }
    }
}

/// Starts a test on the real sandbox or returns from it with `skipped: <reason>`.
macro_rules! sandbox_or_skip {
    () => {
        match crate::support::sandbox() {
            Some(ready) => ready,
            None => return,
        }
    };
}
pub(crate) use sandbox_or_skip;

/// True when `program` is on the test's `PATH`.
pub(crate) fn have(program: &str) -> bool {
    env_var("PATH").unwrap_or_default().split(':').any(|dir| Path::new(dir).join(program).is_file())
}

/// Skips the rest of a test when a tool it needs is missing.
macro_rules! need {
    ($($program:literal),+) => {
        $(
            if !crate::support::have($program) {
                crate::support::say(&format!("skipped: {} is not installed", $program));
                return;
            }
        )+
    };
}
pub(crate) use need;

/// The target dir's temp dir, where every fixture lives.
pub(crate) fn target_tmp() -> PathBuf {
    // Short: the tests bind sockets below it, and a socket's path holds 107 bytes.
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
}

fn private_dir(path: &Path) {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn private_file(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn new_id() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}

/// A fake home, a project and efr's roots for one conversation.
pub(crate) struct Fixture {
    pub(crate) ready: Ready,
    _temp: tempfile::TempDir,
    pub(crate) base: PathBuf,
    pub(crate) home: PathBuf,
    pub(crate) project: PathBuf,
    pub(crate) xrt: PathBuf,
    pub(crate) runtime: PathBuf,
    pub(crate) data: PathBuf,
    pub(crate) state: PathBuf,
    pub(crate) config: PathBuf,
    pub(crate) conversation: ConversationId,
    /// The spec every call starts from; tests change it before a call.
    pub(crate) spec: SandboxSpec,
    /// The launcher's environment.
    pub(crate) env: BTreeMap<OsString, OsString>,
}

/// One finished call.
#[derive(Debug)]
pub(crate) struct Run {
    pub(crate) status: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) result: Option<SandboxResult>,
    pub(crate) apply: Option<Vec<u8>>,
    pub(crate) call_dir: PathBuf,
}

impl Run {
    /// The result, which every call that passed the call dir check writes.
    pub(crate) fn result(&self) -> &SandboxResult {
        self.result.as_ref().unwrap_or_else(|| panic!("no result.json: {self:#?}"))
    }

    /// The apply file's fields.
    pub(crate) fn apply_fields(&self) -> Vec<String> {
        let bytes = self.apply.clone().unwrap_or_default();
        bytes
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty())
            .map(|field| String::from_utf8_lossy(field).into_owned())
            .collect()
    }

    /// Fails with the whole run when the call did not exit with `status`.
    pub(crate) fn expect_status(&self, status: i32) -> &Run {
        assert_eq!(self.status, status, "{self:#?}");
        self
    }
}

impl Fixture {
    /// A new fixture with a git project at `~/p`.
    pub(crate) fn new(ready: &Ready) -> Fixture {
        let root = target_tmp();
        private_dir(&root);
        let temp = tempfile::Builder::new().prefix("s").rand_bytes(5).tempdir_in(&root).unwrap();
        let base = temp.path().canonicalize().unwrap();
        let conversation = ConversationId::from_uuid(new_id());
        let home = base.join("h");
        let project = home.join("p");
        let xrt = base.join("x");
        let runtime = base.join("r");
        let data = base.join("d");
        let state = base.join("s");
        let config = home.join(".config/efr");
        let conv = conversation.to_string();
        for dir in [
            &project,
            &xrt,
            &runtime.join("sbx").join(&conv),
            &runtime.join("zsh"),
            &data.join("scratch").join(&conv),
            &state.join("sandbox").join(&conv).join("tmp"),
            &config,
        ] {
            private_dir(dir);
        }
        private_file(&runtime.join("zsh/efr-child.zsh"), CHILD_SCRIPT.as_bytes(), 0o600);
        private_file(&runtime.join("zsh/efr-editor"), EDITOR.as_bytes(), 0o700);
        private_file(&config.join("config.toml"), b"[permissions]\nmode = \"auto\"\n", 0o600);
        private_file(&home.join(".zshrc"), b"# the user's rc\n", 0o600);
        let path = env_var("PATH").unwrap_or_else(|| "/usr/bin:/bin".to_owned());
        let spec = SandboxSpec {
            version: SPEC_VERSION,
            conversation,
            call: CallId::from_uuid(new_id()),
            launch: SpecLaunch::Contained,
            write_roots: vec![WriteRoot {
                path: project.clone(),
                kind: WriteRootKind::TurnProject,
            }],
            caches: Vec::new(),
            cache_mode: CacheMode::Overlay,
            masks: Vec::new(),
            floors: vec![
                Floor { path: config.clone(), kind: FloorKind::Config },
                Floor { path: home.join(".zshrc"), kind: FloorKind::ShellStartup },
            ],
            git_dirs: Vec::new(),
            guard_roots: vec![project.clone()],
            protected_names: [".envrc", ".mcp.json", ".claude/", ".vscode/", ".efr/"]
                .map(str::to_owned)
                .to_vec(),
            grants: Vec::new(),
            network: NetworkPlan::None,
            env: EnvPlan {
                promote: PROMOTE.iter().map(|name| (*name).to_owned()).collect(),
                offline_hints: true,
                ..EnvPlan::default()
            },
            shell_path: path.clone(),
            limits: RecordLimits::default(),
            runtime: RuntimePaths {
                home: home.clone(),
                user_runtime: xrt.clone(),
                runtime: runtime.clone(),
                data: data.clone(),
                state: state.clone(),
                config: config.clone(),
                sandbox_dir: state.join("sandbox").join(&conv),
                shell_dir: runtime.join("sbx").join(&conv),
                call_dir: runtime.join("sbx").join(&conv).join("call"),
                scratch: data.join("scratch").join(&conv),
                launcher: ready.bin.clone(),
                child_script: runtime.join("zsh/efr-child.zsh"),
                editor: runtime.join("zsh/efr-editor"),
                zsh: which("zsh"),
                bwrap: ready.bwrap.clone(),
                session_bus: None,
                system_bus: PathBuf::from("/run/dbus/system_bus_socket"),
            },
        };
        let mut env = BTreeMap::new();
        for (name, value) in [
            ("PATH", path.as_str()),
            ("HOME", home.to_str().unwrap_or_default()),
            ("LANG", "C.UTF-8"),
            ("USER", "efr-test"),
            ("GIT_AUTHOR_NAME", "efr test"),
            ("GIT_AUTHOR_EMAIL", "test@example.invalid"),
            ("GIT_COMMITTER_NAME", "efr test"),
            ("GIT_COMMITTER_EMAIL", "test@example.invalid"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("EDITOR", runtime.join("zsh/efr-editor").to_str().unwrap_or_default()),
        ] {
            env.insert(OsString::from(name), OsString::from(value));
        }
        if let Some(rustup) = env_var("RUSTUP_HOME") {
            env.insert("RUSTUP_HOME".into(), rustup.into());
        }
        Fixture {
            ready: ready.clone(),
            _temp: temp,
            base,
            home,
            project,
            xrt,
            runtime,
            data,
            state,
            config,
            conversation,
            spec,
            env,
        }
    }

    /// Makes the project a git repository with one commit.
    pub(crate) fn git_init(&self) {
        let git = |args: &[&str]| {
            let status = command("git")
                .args(args)
                .current_dir(&self.project)
                .envs(&self.env)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        fs::write(self.project.join("README.md"), "hello\n").unwrap();
        git(&["add", "README.md"]);
        git(&["commit", "-q", "-m", "first"]);
    }

    /// Adds a mask.
    pub(crate) fn mask(&mut self, path: &Path, kind: MaskKind) {
        self.spec.masks.push(Mask { path: path.to_path_buf(), kind });
    }

    /// Adds a floor.
    pub(crate) fn floor(&mut self, path: &Path, kind: FloorKind) {
        self.spec.floors.push(Floor { path: path.to_path_buf(), kind });
    }

    /// Adds a cache overlay of `target`.
    pub(crate) fn cache(&mut self, target: &Path) {
        let overlay = CacheOverlay::new(target, &self.spec.runtime.sandbox_dir, &self.home);
        self.spec.caches.push(overlay);
    }

    /// Adds a grant.
    pub(crate) fn grant(&mut self, grant: Grant) {
        self.spec.grants.push(grant);
    }

    /// Moves the fixture to a new conversation of the same home and project: new
    /// shell, sandbox and scratch dirs. Returns the spec of the old one.
    pub(crate) fn switch_conversation(&mut self) -> SandboxSpec {
        let old = self.spec.clone();
        let conversation = ConversationId::from_uuid(new_id());
        let conv = conversation.to_string();
        let runtime = &mut self.spec.runtime;
        runtime.shell_dir = self.runtime.join("sbx").join(&conv);
        runtime.sandbox_dir = self.state.join("sandbox").join(&conv);
        runtime.scratch = self.data.join("scratch").join(&conv);
        private_dir(&runtime.shell_dir);
        private_dir(&runtime.sandbox_dir.join("tmp"));
        private_dir(&runtime.scratch);
        let sandbox_dir = runtime.sandbox_dir.clone();
        for cache in &mut self.spec.caches {
            *cache = CacheOverlay::new(&cache.target, &sandbox_dir, &self.home);
        }
        self.spec.conversation = conversation;
        self.conversation = conversation;
        old
    }

    /// The private tmp's dir outside.
    pub(crate) fn private_tmp(&self) -> PathBuf {
        self.spec.runtime.private_tmp()
    }

    /// Runs `line` contained, from the project.
    pub(crate) fn run(&self, line: &str) -> Run {
        self.call(line, &self.project, |_| {})
    }

    /// Runs `line` with the launcher's working directory `cwd`; `edit` changes the
    /// launcher's command before it starts.
    pub(crate) fn call(&self, line: &str, cwd: &Path, edit: impl FnOnce(&mut Command)) -> Run {
        let call_dir = self.prepare(line);
        let mut command = self.command(&call_dir, cwd);
        edit(&mut command);
        self.collect(&call_dir, run_with_timeout(command))
    }

    /// Makes a call dir with the spec, the line and a nonce.
    pub(crate) fn prepare(&self, line: &str) -> PathBuf {
        let call = CallId::from_uuid(new_id());
        let call_dir = self.spec.runtime.shell_dir.join(call.to_string());
        private_dir(&call_dir);
        let mut spec = self.spec.clone();
        spec.call = call;
        spec.runtime.call_dir.clone_from(&call_dir);
        private_file(&call_dir.join(SPEC_FILE), &spec.to_json().unwrap(), 0o600);
        private_file(&call_dir.join(LINE_FILE), line.as_bytes(), 0o600);
        private_file(&call_dir.join(NONCE_FILE), b"0123456789abcdef0123456789abcdef", 0o600);
        call_dir
    }

    /// The launcher's command for `call_dir`, from `cwd`.
    pub(crate) fn command(&self, call_dir: &Path, cwd: &Path) -> Command {
        let mut command = command(&self.ready.bin);
        command
            .args(["run", "--call-dir"])
            .arg(call_dir)
            .current_dir(cwd)
            .env_clear()
            .envs(&self.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// The run of `call_dir` once the launcher ended.
    pub(crate) fn collect(&self, call_dir: &Path, ended: (i32, String, String)) -> Run {
        let (status, stdout, stderr) = ended;
        let result = fs::read(call_dir.join(RESULT_FILE))
            .ok()
            .map(|bytes| SandboxResult::from_json(&bytes).unwrap());
        let apply = fs::read(call_dir.join("apply")).ok();
        Run { status, stdout, stderr, result, apply, call_dir: call_dir.to_path_buf() }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // An overlay leaves a work dir with mode 0, which the temp dir cannot remove.
        open_up(&self.base);
    }
}

/// Gives the owner every right on every dir below `dir`.
pub(crate) fn open_up(dir: &Path) {
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            open_up(&entry.path());
        }
    }
}

/// Runs `command`, kills it after [`CALL_TIMEOUT`], and returns its status and output.
pub(crate) fn run_with_timeout(mut command: Command) -> (i32, String, String) {
    let mut child = command.spawn().unwrap();
    let pipe = |reader: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = Vec::new();
            if let Some(mut reader) = reader {
                let _ = reader.read_to_end(&mut text);
            }
            String::from_utf8_lossy(&text).into_owned()
        })
    };
    let out = pipe(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = pipe(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    // A waiter thread, so the status arrives the moment the call ends and a timing
    // test measures the call, not a poll interval.
    let pid = child.id();
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(child.wait());
    });
    let status = match receive.recv_timeout(CALL_TIMEOUT) {
        Ok(status) => status.unwrap(),
        Err(_) => {
            if let Some(pid) = rustix::process::Pid::from_raw(i32::try_from(pid).unwrap_or(0)) {
                let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
            }
            panic!("the call took longer than {CALL_TIMEOUT:?}");
        }
    };
    let code = std::os::unix::process::ExitStatusExt::signal(&status)
        .map_or_else(|| status.code().unwrap_or(-1), |signal| 128 + signal);
    (code, out.join().unwrap(), err.join().unwrap())
}

/// The absolute path of `program` on `PATH`.
pub(crate) fn which(program: &str) -> PathBuf {
    env_var("PATH")
        .unwrap_or_default()
        .split(':')
        .map(|dir| Path::new(dir).join(program))
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("/usr/bin").join(program))
}

/// Writes `text` to `path`, making its parents.
pub(crate) fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut file = fs::File::create(path).unwrap();
    file.write_all(text.as_bytes()).unwrap();
}

/// A shell word for `path`, single-quoted.
pub(crate) fn q(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

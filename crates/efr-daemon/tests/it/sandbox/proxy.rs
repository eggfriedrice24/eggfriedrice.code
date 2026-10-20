//! The network proxy (phase D2, efr's auto spec, section 8) end to end: efrd's own
//! probe, engine, proxy and launch, the real `efr-sbx` of `just test-sandbox`
//! (`EFR_TEST_SBX_BIN`) with its bridge, a real hidden zsh (`EFR_TEST_ZSH=1`), and real
//! tools (cargo, curl, git) that reach a local HTTPS server through the proxy.
//!
//! No test reaches a real host. The servers listen on loopback addresses of their own
//! (`127.0.0.2:0` and up) with a leaf of a test CA that `openssl` makes at test time
//! (`registry.rs`). The daemon's proxy runs with the test seams: a test name resolves
//! only to its own server, and every other name and address is refused. A test asserts
//! the server's request log, the exit status and the proxy's records, never a tool's
//! error text, and prints the outputs when it fails.
//!
//! The tests say `skipped:` without the launcher, zsh or a tool, or when the probe says
//! that the sandbox is unavailable; `just test-sandbox` fails on such a line on a ready
//! machine. Their dirs stay below the target dir, not below /tmp, which the sandbox
//! replaces with its own.

mod registry;

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminStatus, AdminStatusResult, ApprovalDecision,
    BlockReason, Blocked, Event, EventEnvelope, Grant, Launch, Method, Mode, NetworkMode,
    SandboxSummary,
};
use efr_provider::Provider;
use efr_test_daemon::TestDaemon;
use efr_test_support::{TestDirs, Wait};
use pretty_assertions::assert_eq;
use serde_json::json;
use sha2::{Digest as _, Sha256};

use self::registry::{Answer, Logged, Server, TestCa};
use super::netview::{assert_routine, launches, outputs, target_dirs};
use super::{REVIEWER_ALLOWS, ScriptedModel, real_launcher, run_turn, skip, with_zsh};
use crate::jobs_wake::{REVIEWER_ASKS, Recorder, turn};
use crate::support::zsh_enabled;

/// The test names and the loopback address of the server behind each.
const REGISTRY: (&str, &str) = ("registry.test", "127.0.0.2");
const GIT: (&str, &str) = ("git.test", "127.0.0.3");

/// The test's own environment variable `name`.
#[expect(clippy::disallowed_methods, reason = "the tests read their own PATH and HOME")]
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// The file `tool` in a dir of the test's own `PATH`.
fn find(tool: &str) -> Option<PathBuf> {
    let path = env_var("PATH").unwrap_or_default();
    path.split(':')
        .chain(["/usr/bin", "/bin"])
        .map(|dir| Path::new(dir).join(tool))
        .find(|file| file.is_file())
}

/// True when `tool` is a file in a dir of the hidden shell's plain `PATH`.
fn on_plain_path(tool: &str) -> bool {
    ["/usr/bin", "/bin"].iter().any(|dir| Path::new(dir).join(tool).is_file())
}

/// Writes `text` to `path`, with its parent dirs.
fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Writes an executable script: a line that runs it shows efr only its name.
fn script(path: &Path, body: &str) {
    write(path, &format!("#!/bin/sh\n{body}\n"));
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// True when `test` can run: zsh and the real launcher are there. Else it says
/// `skipped:` once, before the test makes its CA and servers.
fn can_run(test: &str) -> bool {
    zsh_enabled(test) && real_launcher(test).is_some()
}

/// What a test gives the daemon of [`proxy_daemon`].
struct Options<'a> {
    /// The tools that must be on the hidden shell's `PATH`.
    tools: &'a [&'a str],
    /// The proxy's test names and their servers.
    hosts: Vec<(&'a str, SocketAddr)>,
    /// `network.allow`.
    allow: Vec<String>,
    /// More variables of the hidden shell, which win over the defaults.
    env: BTreeMap<String, String>,
}

impl<'a> Options<'a> {
    fn new(tools: &'a [&'a str]) -> Self {
        Options { tools, hosts: Vec::new(), allow: Vec::new(), env: BTreeMap::new() }
    }
}

/// A daemon over `model` with `network.mode = "proxy"`, the real launcher, efrd's own
/// probe, a real hidden zsh and the project `home/project` registered. `None` after a
/// `skipped:` line without zsh, the launcher or one of the tools, or when the probe
/// says that the sandbox is unavailable. The probe's proxy self-test must pass on a
/// machine where the sandbox is ready.
async fn proxy_daemon(
    test: &str,
    dirs: &Arc<TestDirs>,
    model: Arc<dyn Provider>,
    options: Options<'_>,
) -> Option<TestDaemon> {
    if !zsh_enabled(test) {
        return None;
    }
    let launcher = real_launcher(test)?;
    if let Some(tool) = options.tools.iter().find(|tool| !on_plain_path(tool)) {
        skip(test, &format!("{tool} is not on PATH"));
        return None;
    }
    let project = dirs.home().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let user_runtime = dirs.create_dir("xrt").unwrap();
    let mut env = BTreeMap::from([
        ("HOME".to_owned(), dirs.home().display().to_string()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("XDG_RUNTIME_DIR".to_owned(), user_runtime.display().to_string()),
    ]);
    env.extend(options.env);
    let hosts: BTreeMap<String, SocketAddr> =
        options.hosts.iter().map(|(name, address)| ((*name).to_owned(), *address)).collect();
    // NOTE: the clock starts after the files of the test were written, so no file of
    // the test counts as a program that changed in the turn (which asks the user).
    let start = jiff::Timestamp::now().checked_add(jiff::SignedDuration::from_mins(1)).unwrap();
    // NOTE: `admin.project_add` reloads `config.toml`, so the file holds what the
    // builder sets; else the reload gives the defaults back.
    let words: Vec<String> = options.allow.iter().map(|host| format!("{host:?}")).collect();
    // NOTE: no test here is a wake test: a job's end must not start a turn that takes
    // the scripted answers of the test's next turn.
    write(
        &dirs.dirs().config().join("config.toml"),
        &format!(
            "[permissions]\nmode = \"auto\"\n\n[network]\nmode = \"proxy\"\nallow = [{}]\n\n\
             [jobs]\nmax_wakes_per_hour = 0\n",
            words.join(", ")
        ),
    );
    let allow = options.allow;
    let daemon = with_zsh(TestDaemon::builder())
        .clock(efr_test_support::TestClock::starting_at(start))
        .dirs(Arc::clone(dirs))
        .shell_env(env)
        .sandbox_launcher(&launcher)
        .network_mode(NetworkMode::Proxy)
        .proxy_hosts(hosts)
        .config(move |config| {
            config.permissions.mode = Mode::Auto;
            config.network.allow = allow;
            config.jobs.max_wakes_per_hour = 0;
        })
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
    let sandbox = status.sandbox.unwrap();
    if !sandbox.available {
        // NOTE: just test-sandbox fails when this says skipped on a ready machine.
        skip(test, &format!("the probe says the sandbox is unavailable: {:?}", sandbox.reason));
        drop(client);
        daemon.stop().await.unwrap();
        return None;
    }
    assert!(sandbox.proxy, "the proxy self-test failed: {:?}", sandbox.warnings);
    assert_eq!(sandbox.network_mode, NetworkMode::Proxy, "{:?}", sandbox.warnings);
    let add =
        Method::AdminProjectAdd(AdminProjectAdd { path: project, name: None, git_root: false });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    drop(client);
    Some(daemon)
}

/// The sandbox summary and the exit code of each finished tool call of `events`.
fn summaries(events: &[EventEnvelope]) -> Vec<(Option<SandboxSummary>, Option<i32>)> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallCompleted { sandbox, exit_code, .. } => {
                Some((sandbox.clone(), *exit_code))
            }
            _ => None,
        })
        .collect()
}

/// The one call of `events`: its output, summary and exit code.
fn one_call(events: &[EventEnvelope]) -> (String, SandboxSummary, Option<i32>) {
    let [output] = outputs(events).try_into().unwrap_or_else(|all| panic!("{all:?}"));
    let [(summary, code)] = summaries(events).try_into().unwrap_or_else(|all| panic!("{all:?}"));
    let summary = summary.unwrap_or_else(|| panic!("no sandbox summary: {output}"));
    (output, summary, code)
}

/// A refusal of the proxy, as the summary holds it.
fn refused(host: &str, reason: BlockReason) -> Blocked {
    Blocked { host: host.to_owned(), port: 443, reason, count: 1 }
}

/// The hidden shell's variables that put the toolchain of the test's `cargo` on its
/// `PATH`, with the user's rustup home read-only (spec 16.6; `efr-sbx`'s corpus does
/// the same).
fn toolchain_env(cargo: &Path) -> BTreeMap<String, String> {
    let dir = cargo.parent().unwrap().display().to_string();
    let home = env_var("HOME").unwrap_or_default();
    let rustup = env_var("RUSTUP_HOME").unwrap_or_else(|| format!("{home}/.rustup"));
    let mut env = BTreeMap::from([
        ("PATH".to_owned(), format!("{dir}:/usr/bin:/bin")),
        ("RUSTUP_HOME".to_owned(), rustup),
    ]);
    if let Some(toolchain) = env_var("RUSTUP_TOOLCHAIN") {
        env.insert("RUSTUP_TOOLCHAIN".to_owned(), toolchain);
    }
    env
}

/// The `.crate` file of `efx` 0.1.0, a gzip tar that the system `tar` makes in `dir`.
async fn crate_file(dir: &Path) -> Vec<u8> {
    write(
        &dir.join("efx-0.1.0/Cargo.toml"),
        "[package]\nname = \"efx\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&dir.join("efx-0.1.0/src/lib.rs"), "pub fn efx() {}\n");
    let output = efr_stdx::process::command("tar", dir)
        .args(["--format=ustar", "-czf", "efx-0.1.0.crate", "efx-0.1.0"])
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "tar: {}", String::from_utf8_lossy(&output.stderr));
    std::fs::read(dir.join("efx-0.1.0.crate")).unwrap()
}

/// A sparse cargo registry on `registry.test` with the one crate `efx` 0.1.0.
fn registry_route(file: Vec<u8>) -> impl Fn(&Logged) -> Answer + Send + Sync + 'static {
    let sum: String = Sha256::digest(&file).iter().map(|byte| format!("{byte:02x}")).collect();
    let entry = format!(
        "{{\"name\":\"efx\",\"vers\":\"0.1.0\",\"deps\":[],\"cksum\":\"{sum}\",\
         \"features\":{{}},\"yanked\":false}}\n"
    );
    move |request| match request.path.as_str() {
        "/index/config.json" => {
            Answer::ok("{\"dl\":\"https://registry.test/dl\",\"api\":\"https://registry.test\"}")
        }
        "/index/3/e/efx" => Answer::ok(entry.clone()),
        "/dl/efx/0.1.0/download" => Answer::ok(file.clone()),
        _ => Answer::not_found(),
    }
}

/// The three paths of a `cargo fetch` of `efx`, in their order.
const FETCH_PATHS: [&str; 3] = ["/index/config.json", "/index/3/e/efx", "/dl/efx/0.1.0/download"];

/// The line of the cargo test: a fetch, then a look at the crate in the cache.
const FETCH: &str = "cargo fetch && ls ~/.cargo/registry/cache/*/efx-0.1.0.crate";

/// Runs [`FETCH`] in a project that depends on `efx` from the local registry, with
/// `allow` as `network.allow`. `None` when the test skips.
async fn cargo_fetch(
    test: &str,
    allow: &[&str],
) -> Option<(Vec<EventEnvelope>, Vec<Logged>, usize, String)> {
    if !can_run(test) {
        return None;
    }
    let Some(cargo) = find("cargo") else {
        skip(test, "cargo is not on PATH");
        return None;
    };
    let dirs = target_dirs();
    let ca = TestCa::new(&dirs.root().join("ca"), &[REGISTRY.0]).await;
    let server = Server::start(
        REGISTRY.1,
        &ca,
        registry_route(crate_file(&dirs.root().join("crate")).await),
    );
    let project = dirs.home().join("project");
    write(
        &project.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nefx = \"0.1.0\"\n\n[workspace]\n",
    );
    write(&project.join("src/lib.rs"), "");
    // NOTE: `[http] cainfo` in the home's cargo config, which the hidden shell's
    // environment cannot lose; the cache overlay of `~/.cargo` needs the dir.
    write(
        &dirs.home().join(".cargo/config.toml"),
        &format!(
            "[source.crates-io]\nreplace-with = \"efr-test\"\n\n[source.efr-test]\n\
             registry = \"sparse+https://registry.test/index/\"\n\n[http]\ncainfo = \"{}\"\n\n\
             [net]\nretry = 0\n",
            ca.pem.display()
        ),
    );
    let model =
        ScriptedModel::reviewed(vec![("shell", json!({ "command": FETCH }))], REVIEWER_ASKS);
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let mut options = Options::new(&[]);
    options.hosts = vec![(REGISTRY.0, server.address())];
    options.allow = allow.iter().map(|host| (*host).to_owned()).collect();
    options.env = toolchain_env(&cargo);
    let daemon = proxy_daemon(test, &dirs, provider, options).await?;
    let (_, events) =
        run_turn(&daemon, "fetch the dependencies", Mode::Auto, ApprovalDecision::Deny).await;
    daemon.stop().await.unwrap();
    Some((events, server.log(), model.reviews(), cargo_version(&cargo).await))
}

/// The version of the test's `cargo`, for the report of a failed test.
async fn cargo_version(cargo: &Path) -> String {
    let output = efr_stdx::process::command(cargo, "/").arg("--version").output().await;
    output.map(|out| String::from_utf8_lossy(&out.stdout).into_owned()).unwrap_or_default()
}

/// What a failed cargo test prints: the tool's version, the server's log and the call.
fn cargo_report(version: &str, log: &[Logged], output: &str, summary: &SandboxSummary) -> String {
    format!("{version}\nserver log: {log:#?}\nsummary: {summary:#?}\noutput:\n{output}")
}

#[tokio::test]
async fn shell_cargo_fetch_through_proxy_e2e() {
    let test = "shell_cargo_fetch_through_proxy_e2e";
    let Some((events, log, reviews, version)) = cargo_fetch(test, &[REGISTRY.0]).await else {
        return;
    };
    let (output, summary, code) = one_call(&events);
    let report = cargo_report(&version, &log, &output, &summary);
    assert_routine(&events, reviews);
    assert_eq!(launches(&events), [Launch::Contained { grants: Vec::new() }], "{report}");
    assert_eq!(code, Some(0), "{report}");
    assert!(summary.proxied, "{report}");
    assert_eq!(summary.blocked, Vec::new(), "{report}");
    let paths: Vec<&str> = log.iter().map(|request| request.path.as_str()).collect();
    let mut order = paths.iter().filter(|path| FETCH_PATHS.contains(path));
    for wanted in FETCH_PATHS {
        assert!(order.any(|path| *path == wanted), "{wanted} in order: {report}");
    }
    assert!(
        log.iter().all(|request| request.host == REGISTRY.0 && !request.authorized()),
        "{report}"
    );
    assert!(output.contains("efx-0.1.0.crate"), "{report}");

    // The control: with no allowed host, the proxy refuses the registry.
    let Some((events, log, reviews, version)) = cargo_fetch(test, &[]).await else { return };
    let (output, summary, code) = one_call(&events);
    let report = cargo_report(&version, &log, &output, &summary);
    assert_routine(&events, reviews);
    assert_ne!(code, Some(0), "{report}");
    assert_eq!(summary.blocked, [refused(REGISTRY.0, BlockReason::NotAllowed)], "{report}");
    assert_eq!(log, Vec::new(), "{report}");
}

/// The git server of the push tests: every request without credentials gets 401 and a
/// `WWW-Authenticate` header, every request with them gets 403, so git never erases a
/// stored credential.
fn git_route(request: &Logged) -> Answer {
    let status = if request.authorized() { 403 } else { 401 };
    Answer {
        status,
        headers: vec![("WWW-Authenticate", "Basic realm=\"git.test\"".to_owned())],
        body: b"no\n".to_vec(),
    }
}

/// Where a push test keeps its credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Store {
    /// `~/.git-credentials` and `~/.config/git/credentials`, the store helper's files.
    Files,
    /// `~/.config/git/credentials` only.
    Xdg,
    /// A token in an `insteadOf` URL of the project's git config, which the sandbox
    /// must read. (A test daemon's git ignores `~/.gitconfig`, so the same rule for the
    /// user's git config has its test in efrd's unit tests.)
    InsteadOf,
    /// gh's credential helper (a fake `gh` that prints the token of its `hosts.yml`) in
    /// the project's git config, with the token in `~/.config/gh/hosts.yml`, which the
    /// sandbox hides (v0.0.4, DV15).
    Gh,
    /// [`Store::Gh`] with the hidden shell's `GH_CONFIG_DIR` at a dir that the sandbox
    /// does not hide, which holds the token too.
    GhConfigDir,
}

/// The dir of [`Store::GhConfigDir`] below the home.
const GH_DIR: &str = "ghcfg";

/// The fake `gh`: for `gh auth git-credential get` it prints the token of the
/// `hosts.yml` of gh's config dir, when it can read one.
const FAKE_GH: &str = "\
[ \"$1 $2 $3\" = \"auth git-credential get\" ] || exit 0
cat >/dev/null
dir=\"${GH_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/gh}\"
token=$(sed -n 's/^ *oauth_token: *//p' \"$dir/hosts.yml\" 2>/dev/null)
[ -n \"$token\" ] || exit 0
printf 'username=u\\npassword=%s\\n' \"$token\"";

/// A project that is a git repository with one commit, and `./push.sh`, which pushes
/// to `git.test`.
async fn push_project(project: &Path) {
    write(&project.join("README.md"), "x\n");
    script(
        &project.join("push.sh"),
        "GIT_TERMINAL_PROMPT=0 git push https://git.test/r.git HEAD; echo push=$?",
    );
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["add", "README.md", "push.sh"][..],
        &["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "one"][..],
    ] {
        let status = efr_stdx::process::command("git", project)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .await
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }
}

/// The home's git config and credentials for `store`, and the token of
/// [`Store::InsteadOf`] in the git config of `project`. The credential lines name the
/// host with and without the server's port, because git matches a credential by host
/// and port.
fn push_home(home: &Path, project: &Path, ca: &TestCa, port: u16, store: Store) {
    let mut config = format!("[http]\n\tsslCAInfo = {}\n", ca.pem.display());
    if !matches!(store, Store::Gh | Store::GhConfigDir) {
        config.push_str("[credential]\n\thelper = store\n");
    }
    let lines = format!("https://u:tok@git.test\nhttps://u:tok@git.test:{port}\n");
    match store {
        Store::Gh | Store::GhConfigDir => {
            let gh = home.join("bin/gh");
            script(&gh, FAKE_GH);
            let hosts = "git.test:\n    oauth_token: tok\n    user: u\n";
            write(&home.join(".config/gh/hosts.yml"), hosts);
            if store == Store::GhConfigDir {
                write(&home.join(GH_DIR).join("hosts.yml"), hosts);
            }
            // NOTE: as `gh auth setup-git` writes it, for the host with and without the
            // server's port of the control outside the sandbox.
            let file = project.join(".git/config");
            let mut text = std::fs::read_to_string(&file).unwrap();
            for scope in ["https://git.test".to_owned(), format!("https://git.test:{port}")] {
                text.push_str(&format!(
                    "[credential \"{scope}\"]\n\thelper = \n\thelper = !{} auth git-credential\n",
                    gh.display()
                ));
            }
            std::fs::write(file, text).unwrap();
        }
        Store::Files => {
            write(&home.join(".git-credentials"), &lines);
            write(&home.join(".config/git/credentials"), &lines);
        }
        Store::Xdg => write(&home.join(".config/git/credentials"), &lines),
        Store::InsteadOf => {
            let file = project.join(".git/config");
            let mut text = std::fs::read_to_string(&file).unwrap();
            text.push_str("[url \"https://u:tok@git.test/\"]\n\tinsteadOf = https://git.test/\n");
            // NOTE: the control outside the sandbox needs the token too, on its port.
            text.push_str(&format!(
                "[url \"https://u:tok@git.test:{port}/\"]\n\tinsteadOf = https://git.test:{port}/\n"
            ));
            std::fs::write(file, text).unwrap();
        }
    }
    write(&home.join(".gitconfig"), &config);
}

/// The same push outside the sandbox, with the same home: git reaches the server on its
/// own address and sends the stored credential.
async fn push_outside(project: &Path, home: &Path, server: SocketAddr) {
    let port = server.port();
    let output = efr_stdx::process::command("git", project)
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c".to_owned(),
            format!("http.curloptResolve=git.test:{port}:{}", server.ip()),
            "push".to_owned(),
            format!("https://git.test:{port}/r.git"),
            "HEAD".to_owned(),
        ])
        .output()
        .await
        .unwrap();
    assert!(!output.status.success(), "the server refuses every push");
}

/// Runs `./push.sh` in a sandboxed call with the credentials of `store`, then the same
/// push outside the sandbox. Returns the call, the server's log of the call, and the
/// log of the push outside. `None` when the test skips.
async fn push_case(
    test: &str,
    store: Store,
) -> Option<(String, SandboxSummary, Vec<Logged>, Vec<Logged>)> {
    if !can_run(test) {
        return None;
    }
    let dirs = target_dirs();
    let ca = TestCa::new(&dirs.root().join("ca"), &[GIT.0]).await;
    let server = Server::start(GIT.1, &ca, git_route);
    let project = dirs.home().join("project");
    push_project(&project).await;
    push_home(dirs.home(), &project, &ca, server.address().port(), store);
    let gh = matches!(store, Store::Gh | Store::GhConfigDir);
    let mut calls = vec![("shell", json!({ "command": "./push.sh" }))];
    if gh {
        // NOTE: efrd knows where gh reads its token from the `env` file of a hidden shell,
        // which the first call of the conversation starts; without it, the first call
        // denies the hosts of gh's helper.
        calls.insert(0, ("shell", json!({ "command": "true" })));
    }
    let model = ScriptedModel::reviewed(calls, REVIEWER_ASKS);
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let mut options = Options::new(&["git"]);
    options.hosts = vec![(GIT.0, server.address())];
    options.allow = vec![GIT.0.to_owned()];
    // NOTE: a token in the environment that a script could send; the scrub drops it.
    options.env = BTreeMap::from([("GITHUB_TOKEN".to_owned(), "tok".to_owned())]);
    if store == Store::GhConfigDir {
        let dir = dirs.home().join(GH_DIR).display().to_string();
        options.env.insert("GH_CONFIG_DIR".to_owned(), dir);
    }
    let daemon = proxy_daemon(test, &dirs, provider, options).await?;
    let (_, events) = run_turn(&daemon, "run ./push.sh", Mode::Auto, ApprovalDecision::Deny).await;
    assert_routine(&events, model.reviews());
    daemon.stop().await.unwrap();
    let (output, summary) = if gh {
        let [_, output] = outputs(&events).try_into().unwrap_or_else(|all| panic!("{all:?}"));
        let [_, (summary, _)] =
            summaries(&events).try_into().unwrap_or_else(|all| panic!("{all:?}"));
        let summary = summary.unwrap_or_else(|| panic!("no sandbox summary: {output}"));
        (output, summary)
    } else {
        let (output, summary, _) = one_call(&events);
        (output, summary)
    };
    let inside = server.log();
    push_outside(&project, dirs.home(), server.address()).await;
    let outside = server.log()[inside.len()..].to_vec();
    Some((output, summary, inside, outside))
}

/// Asserts that git in the sandbox reached the listed host and sent no credential, and
/// that the same push outside the sandbox sent one.
fn assert_push_without_credentials(output: &str, inside: &[Logged], outside: &[Logged]) {
    let report = format!("inside: {inside:#?}\noutside: {outside:#?}\noutput:\n{output}");
    assert!(!inside.is_empty(), "git reached the server through the proxy: {report}");
    assert!(inside.iter().all(|request| !request.authorized()), "no credential: {report}");
    assert!(inside.iter().all(|request| request.method != "POST"), "no pack sent: {report}");
    assert!(output.contains("push=") && !output.contains("push=0"), "{report}");
    assert!(outside.iter().any(Logged::authorized), "the control sent the credential: {report}");
}

#[tokio::test]
async fn shell_escape_push_to_allowed_host_fails_without_credentials() {
    let test = "shell_escape_push_to_allowed_host_fails_without_credentials";
    let Some((output, summary, inside, outside)) = push_case(test, Store::Files).await else {
        return;
    };
    assert!(summary.proxied);
    assert_push_without_credentials(&output, &inside, &outside);
}

#[tokio::test]
async fn shell_escape_push_with_xdg_git_credentials_fails() {
    let test = "shell_escape_push_with_xdg_git_credentials_fails";
    let Some((output, _, inside, outside)) = push_case(test, Store::Xdg).await else { return };
    assert_push_without_credentials(&output, &inside, &outside);
}

#[tokio::test]
async fn shell_escape_push_with_repo_insteadof_token_is_denied() {
    let test = "shell_escape_push_with_repo_insteadof_token_is_denied";
    let Some((output, summary, inside, outside)) = push_case(test, Store::InsteadOf).await else {
        return;
    };
    let report = format!("inside: {inside:#?}\noutside: {outside:#?}\noutput:\n{output}");
    assert_eq!(inside, Vec::new(), "efrd denied git.test for the call: {report}");
    assert_eq!(summary.blocked, [refused(GIT.0, BlockReason::Denied)], "{report}");
    assert!(output.contains("efr blocks git.test for this command"), "{report}");
    assert!(outside.iter().any(Logged::authorized), "the control sent the token: {report}");
}

#[tokio::test]
async fn shell_gh_helper_reaches_its_host_without_the_token() {
    let test = "shell_gh_helper_reaches_its_host_without_the_token";
    let Some((output, summary, inside, outside)) = push_case(test, Store::Gh).await else {
        return;
    };
    let report = format!("summary: {summary:#?}\noutput:\n{output}");
    assert!(summary.proxied, "{report}");
    assert_eq!(summary.blocked, Vec::new(), "efrd did not deny git.test: {report}");
    assert!(!output.contains("efr blocks"), "{report}");
    assert_push_without_credentials(&output, &inside, &outside);
}

#[tokio::test]
async fn shell_gh_helper_with_open_config_dir_is_denied() {
    let test = "shell_gh_helper_with_open_config_dir_is_denied";
    let Some((output, summary, inside, outside)) = push_case(test, Store::GhConfigDir).await else {
        return;
    };
    let report = format!("inside: {inside:#?}\noutside: {outside:#?}\noutput:\n{output}");
    assert_eq!(inside, Vec::new(), "efrd denied git.test for the call: {report}");
    assert_eq!(summary.blocked, [refused(GIT.0, BlockReason::Denied)], "{report}");
    assert!(output.contains("which the sandbox does not hide"), "{report}");
    assert!(output.contains("efr denies this host for this command"), "{report}");
    assert!(outside.iter().any(Logged::authorized), "the control sent the token: {report}");
}

#[tokio::test]
async fn shell_job_start_outcome_is_proxied() {
    let test = "shell_job_start_outcome_is_proxied";
    if !can_run(test) {
        return;
    }
    let dirs = target_dirs();
    let job = json!({ "command": "true", "background": true });
    let model = Recorder::new(vec![Some(("shell", job)), None]);
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let Some(daemon) = proxy_daemon(test, &dirs, provider, Options::new(&[])).await else {
        return;
    };
    let (sent, events) = turn(&daemon, 1, "run true", Mode::Auto, ApprovalDecision::Deny).await;
    let [(summary, _)] = summaries(&events).try_into().unwrap_or_else(|all| panic!("{all:?}"));
    let expected = SandboxSummary { confined: true, proxied: true, ..SandboxSummary::default() };
    assert_eq!(summary, Some(expected), "{:?}", outputs(&events));
    let client = daemon.client().await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    efr_test_daemon::events_until(
        &mut follow,
        |event| matches!(event, Event::JobEnded { job_id, .. } if job_id.as_str() == "j1"),
    )
    .await
    .unwrap();
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_escape_publish_with_project_npmrc_fails() {
    let test = "shell_escape_publish_with_project_npmrc_fails";
    if !can_run(test) {
        return;
    }
    let dirs = target_dirs();
    let ca = TestCa::new(&dirs.root().join("ca"), &[REGISTRY.0]).await;
    let server = Server::start(REGISTRY.1, &ca, |_: &Logged| Answer::ok("{}"));
    let project = dirs.home().join("project");
    write(&project.join(".npmrc"), "//registry.test/:_authToken=x\n");
    // NOTE: the script sends the token as `npm publish` would, with curl, which every
    // test machine has; the proxy decides by host, not by tool.
    script(
        &project.join("publish.sh"),
        &format!(
            "token=$(sed -n 's/.*_authToken=//p' .npmrc)\n\
             curl -sS --cacert '{}' -X PUT -H \"Authorization: Bearer $token\" \
             --data '{{}}' https://registry.test/efx; echo publish=$?",
            ca.pem.display()
        ),
    );
    let model = ScriptedModel::reviewed(
        vec![("shell", json!({ "command": "./publish.sh" }))],
        REVIEWER_ASKS,
    );
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let mut options = Options::new(&["curl", "sed"]);
    options.hosts = vec![(REGISTRY.0, server.address())];
    options.allow = vec![REGISTRY.0.to_owned()];
    let Some(daemon) = proxy_daemon(test, &dirs, provider, options).await else { return };
    let (_, events) =
        run_turn(&daemon, "run ./publish.sh", Mode::Auto, ApprovalDecision::Deny).await;
    assert_routine(&events, model.reviews());
    daemon.stop().await.unwrap();
    let (output, summary, _) = one_call(&events);
    let log = server.log();
    let report = format!("log: {log:#?}\nsummary: {summary:#?}\noutput:\n{output}");
    assert_eq!(log, Vec::new(), "the server saw no request: {report}");
    assert_eq!(summary.blocked, [refused(REGISTRY.0, BlockReason::Denied)], "{report}");
    assert!(output.contains(".npmrc holds a credential"), "the note names the file: {report}");
    assert!(output.contains("publish=") && !output.contains("publish=0"), "{report}");
}

#[tokio::test]
async fn shell_proxy_denial_reaches_the_model() {
    let test = "shell_proxy_denial_reaches_the_model";
    if !can_run(test) {
        return;
    }
    let dirs = target_dirs();
    let ca = TestCa::new(&dirs.root().join("ca"), &[REGISTRY.0]).await;
    let server = Server::start(REGISTRY.1, &ca, |_: &Logged| Answer::ok("ok\n"));
    // NOTE: efr reads `./get.sh` as a name, so it predicts no host exit for the host
    // inside; only the proxy can refuse it.
    script(
        &dirs.home().join("project/get.sh"),
        &format!("curl -sS --cacert '{}' https://other.test/; echo get=$?", ca.pem.display()),
    );
    let model =
        ScriptedModel::reviewed(vec![("shell", json!({ "command": "./get.sh" }))], REVIEWER_ASKS);
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let mut options = Options::new(&["curl"]);
    options.hosts = vec![(REGISTRY.0, server.address())];
    options.allow = vec![REGISTRY.0.to_owned()];
    let Some(daemon) = proxy_daemon(test, &dirs, provider, options).await else { return };
    let (_, events) = run_turn(&daemon, "run ./get.sh", Mode::Auto, ApprovalDecision::Deny).await;
    assert_routine(&events, model.reviews());
    daemon.stop().await.unwrap();
    let (output, summary, _) = one_call(&events);
    let report = format!("summary: {summary:#?}\noutput:\n{output}");
    assert!(summary.proxied, "{report}");
    assert_eq!(summary.blocked, [refused("other.test", BlockReason::NotAllowed)], "{report}");
    assert!(output.contains("refused other.test:443: not on the allow list"), "{report}");
    assert!(output.contains("get=") && !output.contains("get=0"), "{report}");
    assert_eq!(server.log(), Vec::new(), "{report}");
}

/// A free port on the host's loopback, for a server that a job starts.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[tokio::test]
async fn shell_dev_server_job_is_reachable_on_host_loopback() {
    let test = "shell_dev_server_job_is_reachable_on_host_loopback";
    if !can_run(test) {
        return;
    }
    let dirs = target_dirs();
    let port = free_port();
    // NOTE: the port is free only when the test looks; a marker of this test's own
    // tells its job's server from another process that took the port since.
    let marker = format!("dev server {}", dirs.root().display());
    write(&dirs.home().join("project/index.html"), &format!("{marker}\n"));
    let line = format!("python3 -m http.server --bind 127.0.0.1 {port}");
    let job = json!({
        "command": line,
        "background": true,
        "max_minutes": 1,
        "needs": { "hosts": [format!("localhost:{port}")] },
    });
    let model = Recorder::reviewed(
        vec![Some(("shell", job)), None, Some(("job_stop", json!({ "id": "j1" }))), None],
        REVIEWER_ALLOWS,
    );
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let Some(daemon) = proxy_daemon(test, &dirs, provider, Options::new(&["python3"])).await else {
        return;
    };
    let text = format!("start the dev server on localhost:{port} so I can open it in my browser");
    let (_, events) = turn(&daemon, 1, &text, Mode::Auto, ApprovalDecision::Deny).await;
    let info = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::JobStarted { job, .. } => Some(job.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no job started: {:?}", outputs(&events)));
    assert_eq!(info.launch, Launch::Contained { grants: vec![Grant::OpenNetwork] });
    // The test's own connection from the host reaches the job's server.
    let mut stream = Wait::new("the job's server")
        .until_some(|| TcpStream::connect(("127.0.0.1", port)).ok())
        .await
        .unwrap();
    stream.write_all(b"GET /index.html HTTP/1.0\r\n\r\n").unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    assert!(answer.starts_with("HTTP/1.0 200"), "{answer}");
    assert!(answer.contains(&marker), "the job's own server: {answer}");
    let (_, events) =
        turn(&daemon, 2, "stop the dev server", Mode::Auto, ApprovalDecision::Deny).await;
    assert!(
        events.iter().any(|envelope| matches!(&envelope.event, Event::ToolCallCompleted { .. })),
        "{:?}",
        outputs(&events)
    );
    let client = daemon.client().await.unwrap();
    let conversation = info.conversation_id;
    let mut follow = daemon.follow(&client, conversation).await.unwrap();
    efr_test_daemon::events_until(
        &mut follow,
        |event| matches!(event, Event::JobEnded { job_id, .. } if job_id.as_str() == "j1"),
    )
    .await
    .unwrap();
    drop(client);
    daemon.stop().await.unwrap();
}

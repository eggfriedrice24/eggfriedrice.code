use std::path::{Path, PathBuf};

use efr_config::FileState;
use efr_protocol::{AdminStatusResult, DaemonRoots, Method, RootDir, RootSource};
use jiff::SignedDuration;
use pretty_assertions::assert_eq;

use super::{Facts, Root, differences, text};
use crate::context::Context;
use crate::error::Exit;
use crate::run;
use crate::testing::{TestEnv, capture, command, now};

fn root(name: &'static str, path: &str, source: &str) -> Root {
    Root { name, path: PathBuf::from(path), source: source.to_owned(), exists: true }
}

fn daemon_roots(data: &str) -> DaemonRoots {
    let dir = |path: &str, source| RootDir { path: PathBuf::from(path), source };
    DaemonRoots {
        config: dir("/h/efr/config", RootSource::EfrHome),
        data: dir(data, RootSource::EfrHome),
        state: dir("/h/efr/state", RootSource::EfrHome),
        runtime: dir("/run/user/1000/efr", RootSource::Xdg),
    }
}

fn facts(daemon: Result<DaemonRoots, String>) -> Facts {
    Facts {
        roots: vec![
            root("config", "/h/efr/config", "EFR_HOME"),
            root("data", "/h/efr/data", "EFR_HOME"),
            Root { exists: false, ..root("state", "/h/efr/state", "EFR_HOME") },
            root("runtime", "/run/user/1000/efr", "XDG"),
        ],
        config: FileState::of(Path::new("/nonexistent/efr/config.toml")),
        database: PathBuf::from("/h/efr/data/efr.sqlite"),
        database_exists: true,
        secrets: PathBuf::from("/h/efr/data/secrets"),
        socket: PathBuf::from("/run/user/1000/efr/daemon.sock"),
        socket_exists: false,
        daemon,
    }
}

#[test]
fn each_root_shows_its_source_and_the_daemon_its_own() {
    let shown = text(&facts(Ok(daemon_roots("/h/efr/data"))));
    assert_eq!(
        shown,
        "\
config         /h/efr/config  (EFR_HOME, exists)
data           /h/efr/data  (EFR_HOME, exists)
state          /h/efr/state  (EFR_HOME, missing)
runtime        /run/user/1000/efr  (XDG, exists)
config.toml    /nonexistent/efr/config.toml  (absent)
efr.sqlite     /h/efr/data/efr.sqlite  (exists)
secrets/       /h/efr/data/secrets
daemon.sock    /run/user/1000/efr/daemon.sock  (absent)
daemon config  /h/efr/config  (EFR_HOME)
daemon data    /h/efr/data  (EFR_HOME)
daemon state   /h/efr/state  (EFR_HOME)
daemon runtime /run/user/1000/efr  (XDG)
"
    );
    assert_eq!(differences(&facts(Ok(daemon_roots("/h/efr/data")))), Vec::<String>::new());
}

#[test]
fn a_daemon_root_elsewhere_is_a_warning_with_the_fix() {
    let warnings = differences(&facts(Ok(daemon_roots("/home/u/.local/share/efr"))));
    assert_eq!(
        warnings,
        [
            "the daemon uses /home/u/.local/share/efr for the data root, this shell uses /h/efr/data; give efrd.service the same EFR_HOME with systemctl --user edit efrd"
        ]
    );
    assert_eq!(differences(&facts(Err("not running".to_owned()))), Vec::<String>::new());
    assert!(text(&facts(Err("not running".to_owned()))).ends_with("daemon         not running\n"));
}

fn status(roots: DaemonRoots) -> AdminStatusResult {
    AdminStatusResult {
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        version: "0.1.0".to_owned(),
        protocol: efr_protocol::PROTOCOL_VERSION,
        pid: 777,
        started_at: now() - SignedDuration::from_secs(90),
        screen_backend: "vt100".to_owned(),
        conversations: 0,
        shells: 0,
        providers: vec![],
        roots: Some(roots),
        config: None,
    }
}

#[tokio::test]
async fn json_names_the_same_facts_and_the_warnings_go_to_stderr() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx: Context = env.context();
    let mut roots = daemon_roots("/elsewhere/data");
    roots.config.path = env.dirs.config().to_path_buf();
    roots.state.path = env.dirs.state().to_path_buf();
    roots.runtime.path = env.dirs.runtime().to_path_buf();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::AdminStatus(_)), "{}", method.name());
        conn.reply(id, &status(roots)).await;
        conn.until_closed().await;
    };
    let (mut out, captured) = capture();
    let line = command(&["paths", "--json"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);

    assert_eq!(exit, Exit::Success);
    let json: serde_json::Value = serde_json::from_str(&captured.stdout()).unwrap();
    assert_eq!(json["roots"]["data"]["path"], env.dirs.data().display().to_string());
    assert_eq!(json["roots"]["data"]["source"], "XDG");
    assert_eq!(json["roots"]["data"]["exists"], true);
    assert_eq!(json["files"]["config"]["exists"], false);
    assert_eq!(json["files"]["socket"]["exists"], true);
    assert_eq!(json["daemon"]["roots"]["data"]["path"], "/elsewhere/data");
    assert_eq!(json["daemon"]["roots"]["data"]["source"], "EFR_HOME");
    assert_eq!(json["warnings"].as_array().unwrap().len(), 1);
    let stderr = captured.stderr();
    assert!(
        stderr.starts_with("efr: warning: the daemon uses /elsewhere/data for the data root"),
        "{stderr}"
    );
}

#[tokio::test]
async fn without_a_daemon_paths_still_shows_this_shells_roots() {
    let env = TestEnv::new();
    let (mut out, captured) = capture();
    let exit = run::run(&command(&["paths"]), &env.context(), &mut out).await;

    assert_eq!(exit, Exit::Success);
    let stdout = captured.stdout();
    assert!(
        stdout.starts_with(&format!(
            "config         {}  (XDG, exists)\n",
            env.dirs.config().display()
        )),
        "{stdout}"
    );
    assert!(stdout.ends_with("daemon         not running\n"), "{stdout}");
    assert_eq!(captured.stderr(), "");
}

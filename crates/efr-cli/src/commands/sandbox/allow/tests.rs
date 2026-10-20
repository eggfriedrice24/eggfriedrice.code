use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{
    AdminConfigReloadResult, AdminStatusResult, ConfigStatus, Method, SandboxStatus,
};
use pretty_assertions::assert_eq;

use crate::context::Context;
use crate::error::Exit;
use crate::run;
use crate::testing::{ScriptedKeys, TestEnv, capture, command, now};

/// A commented `config.toml` with three rules; the second names a floor, so efrd
/// disables it.
const FILE: &str = "\
# my rules
[sandbox]
# notes are mine
[[sandbox.allow]]
kind = \"write\"
under = \"~/notes\"

[[sandbox.allow]]
kind = \"write\"
under = \"~/.zshrc\"

# downloads
[[sandbox.allow]]
kind = \"host\"
host = \"download.example.org\"
";

/// The warning of efrd for the disabled rule of [`FILE`].
const DISABLED: &str = "sandbox.allow[1] is disabled: it names a shell startup file (floor)";

async fn efr(ctx: &Context, args: &[&str]) -> (Exit, String, String) {
    let (mut out, captured) = capture();
    let exit = run::run(&command(args), ctx, &mut out).await;
    (exit, captured.stdout(), captured.stderr())
}

/// What `admin.status` says of a daemon that reads `path`, with `warnings`.
fn status(path: &Path, warnings: &[&str]) -> AdminStatusResult {
    AdminStatusResult {
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        version: "0.1.0".to_owned(),
        protocol: efr_protocol::PROTOCOL_VERSION,
        pid: 777,
        started_at: now(),
        screen_backend: "ghostty".to_owned(),
        conversations: 0,
        shells: 0,
        providers: Vec::new(),
        catalog: None,
        roots: None,
        config: Some(ConfigStatus {
            path: path.to_path_buf(),
            exists: true,
            symlink_target: None,
            reload_error: None,
            restart_needed: Vec::new(),
            held: None,
        }),
        sandbox: Some(SandboxStatus {
            available: true,
            reason: None,
            fix: None,
            warnings: warnings.iter().map(|warning| (*warning).to_owned()).collect(),
            ..SandboxStatus::unavailable("not used")
        }),
        sandbox_paths: None,
        full_mode: None,
    }
}

/// Lists the rules against a fake daemon that answers `admin.status` with `answer`.
async fn list_with(env: &TestEnv, answer: AdminStatusResult) -> (Exit, String, String) {
    // NOTE: the socket of an earlier fake daemon of the test stays behind.
    let _ = std::fs::remove_file(env.socket());
    let daemon = env.listen();
    let ctx = env.context();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::AdminStatus(_)), "{}", method.name());
        conn.reply(id, &answer).await;
        conn.until_closed().await;
    };
    let (shown, ()) = tokio::join!(efr(&ctx, &["sandbox", "allow"]), script);
    shown
}

#[tokio::test]
async fn sandbox_allow_lists_rules_and_disabled() {
    let env = TestEnv::new();
    let path = env.dirs.config().join("config.toml");
    let ctx = env.context();
    // No file, no rules: the daemon is not asked.
    let (exit, stdout, stderr) = efr(&ctx, &["sandbox", "allow"]).await;
    assert_eq!((exit, stderr.as_str()), (Exit::Success, ""));
    assert_eq!(stdout, format!("no always rules in {}\n", path.display()));

    std::fs::write(&path, FILE).unwrap();
    let (exit, stdout, stderr) = Box::pin(list_with(&env, status(&path, &[DISABLED]))).await;
    assert_eq!((exit, stderr.as_str()), (Exit::Success, ""));
    insta::assert_snapshot!(stdout);

    // Without a daemon the rules show, and a note says that efr cannot tell more.
    let (exit, stdout, stderr) = efr(&ctx, &["sandbox", "allow"]).await;
    assert_eq!(exit, Exit::Success);
    assert!(!stdout.contains("disabled"), "{stdout}");
    assert_eq!(stdout.lines().count(), 3, "{stdout}");
    assert_eq!(stderr, "the daemon is not running, so efr cannot say which rules are disabled\n");

    // A daemon that reads another file says nothing about this one.
    let other = PathBuf::from("/elsewhere/config.toml");
    let (exit, stdout, stderr) = Box::pin(list_with(&env, status(&other, &[DISABLED]))).await;
    assert_eq!(exit, Exit::Success);
    assert!(!stdout.contains("disabled"), "{stdout}");
    assert!(stderr.starts_with("the daemon reads /elsewhere/config.toml"), "{stderr}");

    // A rule of a bad shape is the file's error, with its place.
    std::fs::write(&path, "[[sandbox.allow]]\nkind = \"write\"\nunder = 5\n").unwrap();
    let (exit, stdout, stderr) = efr(&ctx, &["sandbox", "allow"]).await;
    assert_eq!((exit, stdout.as_str()), (Exit::Invalid, ""));
    assert!(stderr.starts_with(&format!("efr: {}: ", path.display())), "{stderr}");
}

#[tokio::test]
async fn sandbox_allow_remove_confirms_and_checks_the_entry() {
    let env = TestEnv::new();
    let path = env.dirs.config().join("config.toml");
    std::fs::write(&path, FILE).unwrap();

    // Without a terminal there is no question, and nothing changes.
    let ctx = env.context();
    let (exit, _, stderr) = efr(&ctx, &["sandbox", "allow", "remove", "1"]).await;
    assert_eq!(exit, Exit::Usage);
    assert!(stderr.contains("needs a terminal for its question; nothing changed"), "{stderr}");
    // A number that the file does not have.
    let (exit, _, stderr) = efr(&ctx, &["sandbox", "allow", "remove", "3"]).await;
    assert_eq!(exit, Exit::Usage);
    assert!(stderr.contains("sandbox.allow has no rule 3; config.toml has 3"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), FILE);

    // `n` keeps the rule.
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let ((exit, stdout, stderr), ()) =
        tokio::join!(efr(&ctx, &["sandbox", "allow", "remove", "1"]), presser.press(b'n'));
    assert_eq!(exit, Exit::Success);
    assert_eq!(stdout, "remove sandbox.allow[1]: write under ~/.zshrc? y/n\n");
    assert_eq!(stderr, "nothing changed\n");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), FILE);

    // The file changed after the question: entry 1 is another rule now, so it stays.
    let moved = FILE.replacen("under = \"~/.zshrc\"", "under = \"~/music\"", 1);
    let change = async {
        // NOTE: a key that answers nothing comes once the question reads keys.
        presser.press(b'x').await;
        std::fs::write(&path, &moved).unwrap();
        presser.press(b'y').await;
    };
    let ((exit, _, stderr), ()) =
        tokio::join!(efr(&ctx, &["sandbox", "allow", "remove", "1"]), change);
    assert_eq!(exit, Exit::Invalid);
    assert!(stderr.contains("sandbox.allow[1] changed after the question"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), moved);

    // `y` removes it, keeps every comment, and asks the daemon to reload.
    std::fs::write(&path, FILE).unwrap();
    let daemon = env.listen();
    let script = async {
        presser.press(b'y').await;
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::AdminConfigReload(_)), "{}", method.name());
        let reloaded =
            AdminConfigReloadResult { applied: true, error: None, restart_needed: Vec::new() };
        conn.reply(id, &reloaded).await;
        conn.until_closed().await;
    };
    let ((exit, stdout, stderr), ()) =
        tokio::join!(efr(&ctx, &["sandbox", "allow", "remove", "1"]), script);
    assert_eq!((exit, stderr.as_str()), (Exit::Success, ""));
    assert!(
        stdout.contains(&format!(
            "{}: removed sandbox.allow[1]: write under ~/.zshrc\n",
            path.display()
        )),
        "{stdout}"
    );
    let left = std::fs::read_to_string(&path).unwrap();
    insta::assert_snapshot!(left);
    assert!(!left.contains(".zshrc"), "{left}");
    assert!(left.contains("# notes are mine") && left.contains("# downloads"), "{left}");
}

use std::path::{Path, PathBuf};

use efr_client::{ClientError, DaemonInfo, Discovered};
use efr_config::FileState;
use efr_protocol::{
    AdminConfigReloadResult, AdminStatusResult, ConfigFileError, ConfigStatus, Method,
};
use efr_stdx::env::{Env, Var};
use efr_stdx::paths::{Dirs, RootSource, RootSources};
use jiff::SignedDuration;
use pretty_assertions::assert_eq;

use super::{View, effective};
use crate::context::Context;
use crate::error::Exit;
use crate::run;
use crate::settings::Settings;
use crate::terminal::TermFacts;
use crate::testing::{TestEnv, capture, command, now, terminal_facts};

const PATH: &str = "/c/efr/config.toml";

fn fixed_dirs() -> Dirs {
    Dirs::new("/c/efr", "/d/efr", "/s/efr", "/run/user/1000/efr").unwrap()
}

fn absent() -> FileState {
    FileState::of(Path::new("/nonexistent/efr/config.toml"))
}

fn view(file: &str, daemon: Result<AdminStatusResult, String>) -> View {
    View {
        discovered: Ok(Discovered {
            socket: PathBuf::from("/run/user/1000/efr/daemon.sock"),
            info: None,
        }),
        file: efr_config::Settings::parse(Path::new(PATH), Some(file))
            .map_err(|error| crate::settings::describe(&error)),
        file_state: absent(),
        daemon,
    }
}

fn not_running() -> Result<AdminStatusResult, String> {
    Err("the daemon is not running".to_owned())
}

fn status(config: ConfigStatus) -> AdminStatusResult {
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
        catalog: None,
        roots: None,
        config: Some(config),
        sandbox: None,
        sandbox_paths: None,
    }
}

fn config_status(path: &str) -> ConfigStatus {
    ConfigStatus {
        path: PathBuf::from(path),
        exists: true,
        symlink_target: None,
        reload_error: None,
        restart_needed: vec![],
    }
}

#[test]
fn defaults_with_their_sources() {
    let env = TestEnv::new();
    let ctx = Context { dirs: fixed_dirs(), term: terminal_facts(), ..env.context() };
    insta::assert_snapshot!(effective(&ctx, &view("", not_running())));
}

#[test]
fn overrides_name_where_they_come_from() {
    let env = TestEnv::new();
    let file = "[render]\ntheme = \"gruvbox-dark\"\n[shell]\nidle_minutes = 5\n";
    let settings = Settings::parse(Path::new(PATH), file);
    let ctx = Context {
        dirs: fixed_dirs(),
        sources: RootSources::all(RootSource::EfrHome),
        env: Env::fixed([(Var::OpenBrowser, "yes"), (Var::Log, "debug")]),
        term: TermFacts { colorterm: Some("truecolor".to_owned()), ..terminal_facts() },
        settings,
        ..env.context()
    };
    let mut daemon = config_status(PATH);
    daemon.restart_needed = vec!["screen".to_owned()];
    daemon.reload_error = Some(ConfigFileError {
        message: "the config file is not valid: unknown field `idle_minuets`".to_owned(),
        line: Some(4),
        column: Some(1),
        key: Some("shell.idle_minuets".to_owned()),
    });
    let mut view = view(file, Ok(status(daemon)));
    view.discovered = Ok(Discovered {
        socket: PathBuf::from("/tmp/elsewhere/daemon.sock"),
        info: Some(DaemonInfo {
            pid: 1,
            socket: PathBuf::from("/tmp/elsewhere/daemon.sock"),
            protocol: efr_protocol::PROTOCOL_VERSION,
            daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
            tailnet_endpoint: None,
        }),
    });
    insta::assert_snapshot!(effective(&ctx, &view));
}

#[test]
fn an_auto_theme_says_which_background_chose_it() {
    let env = TestEnv::new();
    let file = "[render]\ntheme = \"auto\"\ntheme_light = \"github\"\n";
    let shown = |background: Option<&str>| {
        let settings = Settings::parse_on(Path::new(PATH), Some(file), background);
        let ctx = Context { dirs: fixed_dirs(), settings, ..env.context() };
        let text = effective(&ctx, &view(file, not_running()));
        text.lines().find(|line| line.starts_with("theme = ")).unwrap().to_owned()
    };
    assert_eq!(
        shown(Some("light")),
        format!("theme = \"github\"  # {PATH}; auto, EFR_TERMINAL_BG is light")
    );
    assert_eq!(
        shown(None),
        format!("theme = \"catppuccin-mocha\"  # {PATH}; auto, the background is not known")
    );
}

#[test]
fn pipes_no_color_and_problems_are_explained() {
    let env = TestEnv::new();
    let settings = Settings::parse(Path::new(PATH), "[render]\ntheme = \"neon\"\n");
    let ctx = Context {
        dirs: fixed_dirs(),
        env: Env::fixed([(Var::OpenBrowser, "maybe")]),
        term: TermFacts { no_color: true, ..TermFacts::default() },
        settings,
        ..env.context()
    };
    let mut view = view("[shell\n", Ok(status(config_status("/elsewhere/efr/config.toml"))));
    view.discovered = Err(ClientError::ProtocolMismatch { daemon: 9, client: 1 });
    insta::assert_snapshot!(effective(&ctx, &view));
}

#[test]
fn a_dumb_terminal_gets_raw_markdown() {
    let env = TestEnv::new();
    let ctx = Context {
        dirs: fixed_dirs(),
        term: TermFacts { term: Some("dumb".to_owned()), ..terminal_facts() },
        ..env.context()
    };
    let text = effective(&ctx, &view("", not_running()));
    assert!(text.contains("formatted = false  # TERM is dumb; raw markdown\n"), "{text}");
}

#[test]
fn every_key_of_the_file_is_shown_once() {
    let env = TestEnv::new();
    let ctx = Context { dirs: fixed_dirs(), ..env.context() };
    let text = effective(&ctx, &view("", not_running()));
    // The file's keys come before the first table of efr's own facts.
    let (file, _) = text.split_once("\n[efr]\n").unwrap();
    for key in efr_config::keys() {
        let lines = file.lines().filter(|line| line.starts_with(&format!("{key} = "))).count();
        assert_eq!(lines, 1, "{key}");
    }
}

#[tokio::test]
async fn config_show_needs_no_daemon_and_skips_the_warning_preamble() {
    let env = TestEnv::new();
    let settings = Settings::parse(Path::new("/c/efr/config.toml"), "[render\n");
    let ctx = Context { settings, ..env.context() };
    let (mut out, captured) = capture();
    let exit = run::run(&command(&["config", "show"]), &ctx, &mut out).await;
    assert_eq!(exit, Exit::Success);
    assert_eq!(captured.stderr(), "");
    assert!(captured.stdout().contains("# warning: /c/efr/config.toml is not valid"));
    assert!(captured.stdout().contains("# the daemon is not running\n"));
}

/// Runs `efr <args>` in `ctx` and returns the exit, stdout and stderr.
async fn efr(ctx: &Context, args: &[&str]) -> (Exit, String, String) {
    let (mut out, captured) = capture();
    let exit = run::run(&command(args), ctx, &mut out).await;
    (exit, captured.stdout(), captured.stderr())
}

#[tokio::test]
async fn check_says_ok_or_names_the_place_and_the_key_of_an_error() {
    let env = TestEnv::new();
    let ctx = env.context();
    let path = env.dirs.config().join("config.toml");
    let shown = path.display().to_string();

    let (exit, stdout, _) = efr(&ctx, &["config", "check"]).await;
    assert_eq!(
        (exit, stdout),
        (Exit::Success, format!("{shown}: ok, absent; every value is its default\n"))
    );

    std::fs::write(&path, "[shell]\nidle_minutes = 5\n").unwrap();
    let (exit, stdout, _) = efr(&ctx, &["config", "check"]).await;
    assert_eq!((exit, stdout), (Exit::Success, format!("{shown}: ok\n")));

    std::fs::write(&path, "[shell]\nlogin = true\nidle_minuets = 5\n").unwrap();
    let (exit, stdout, _) = efr(&ctx, &["config", "check"]).await;
    assert_eq!(exit, Exit::Invalid);
    assert_eq!(Exit::Invalid.code(), 1);
    assert!(stdout.starts_with(&format!("{shown}: unknown field `idle_minuets`")), "{stdout}");
    assert!(stdout.ends_with("(shell.idle_minuets, line 3, column 1)\n"), "{stdout}");

    let other = env.dirs.data().join("other.toml");
    std::fs::write(&other, "[render]\ntheme = \"neon\"\n").unwrap();
    let (exit, stdout, _) = efr(&ctx, &["config", "check", other.to_str().unwrap()]).await;
    assert_eq!(exit, Exit::Invalid);
    assert!(stdout.contains("the theme \"neon\", which does not exist"), "{stdout}");

    let missing = env.dirs.data().join("missing.toml");
    let (exit, stdout, _) = efr(&ctx, &["config", "check", missing.to_str().unwrap()]).await;
    assert_eq!(
        (exit, stdout),
        (Exit::Invalid, format!("{}: it does not exist\n", missing.display()))
    );
}

#[tokio::test]
async fn check_notes_a_window_above_the_largest_of_its_model() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let path = env.dirs.config().join("config.toml");
    let shown = path.display().to_string();
    std::fs::write(
        &path,
        "[openai]\nmodels = [{ id = \"gpt-5.5\", context_window = 900000 }, \
         { id = \"gpt-5.4\", context_window = 100000 }, { id = \"new\", context_window = 5000000 }]\n",
    )
    .unwrap();
    let script = async {
        let mut conn = daemon.accept().await;
        conn.answer_models(&crate::testing::models()).await;
        conn.until_closed().await;
    };

    let ((exit, stdout, _), ()) = tokio::join!(efr(&ctx, &["config", "check"]), script);

    assert_eq!(exit, Exit::Success, "efrd uses the largest window, so the file is valid");
    assert_eq!(
        stdout,
        format!(
            "{shown}: ok\n{shown}: note: openai.models: the context_window 900000 of gpt-5.5 is \
             above the largest window that the model takes (872000), so efrd uses 872000\n"
        )
    );
}

#[test]
fn only_a_window_above_a_known_largest_one_gets_a_note() {
    let list = crate::testing::models();
    let asked = [("gpt-5.5", 872_000), ("gpt-5.4", 128_001), ("my-model", 9_000_000)];
    let notes = super::windows_above_the_largest(&asked, &list);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("128001 of gpt-5.4"), "{notes:?}");
}

#[tokio::test]
async fn check_names_a_bad_colour_a_bad_theme_file_and_accepts_auto() {
    let env = TestEnv::new();
    let ctx = env.context();
    let path = env.dirs.config().join("config.toml");

    std::fs::write(&path, "[render]\ntheme = \"auto\"\ntheme_light = \"github\"\n").unwrap();
    let (exit, stdout, _) = efr(&ctx, &["config", "check"]).await;
    assert_eq!(exit, Exit::Success, "{stdout}");

    std::fs::write(&path, "[render.colors]\nwarning = \"#12345\"\n").unwrap();
    let (exit, stdout, _) = efr(&ctx, &["config", "check"]).await;
    assert_eq!(exit, Exit::Invalid);
    assert!(stdout.contains("render.colors.warning"), "{stdout}");

    let theme = env.dirs.data().join("theme.toml");
    std::fs::write(&theme, "[colors]\nmuted = \"grey\"\n").unwrap();
    std::fs::write(&path, format!("[render]\npalette = {:?}\n", theme.display().to_string()))
        .unwrap();
    let (exit, stdout, _) = efr(&ctx, &["config", "check"]).await;
    assert_eq!(exit, Exit::Invalid);
    assert!(stdout.contains("render.palette: ") && stdout.contains("colors.muted"), "{stdout}");
}

#[tokio::test]
async fn schema_prints_the_json_schema() {
    let env = TestEnv::new();
    let (exit, stdout, _) = efr(&env.context(), &["config", "schema"]).await;
    assert_eq!(exit, Exit::Success);
    assert_eq!(stdout, efr_config::schema_text());
}

#[tokio::test]
async fn set_and_unset_keep_the_comments_and_say_the_daemon_reads_the_file_when_it_starts() {
    let env = TestEnv::new();
    let ctx = env.context();
    let path = env.dirs.config().join("config.toml");
    std::fs::write(&path, "# mine\n[model]\n# the default\nname = \"gpt-5.5\"  # pinned\n")
        .unwrap();

    let (exit, stdout, stderr) = efr(&ctx, &["config", "set", "model.name", "gpt-5.4"]).await;
    assert_eq!((exit, stderr.as_str()), (Exit::Success, ""));
    assert_eq!(
        stdout,
        format!(
            "{}: set model.name = gpt-5.4\nthe daemon is not running; it reads the file when it starts\n",
            path.display()
        )
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, "# mine\n[model]\n# the default\nname = \"gpt-5.4\"  # pinned\n");

    let (exit, _, _) = efr(&ctx, &["config", "set", "conversation.max_queued", "4"]).await;
    assert_eq!(exit, Exit::Success);
    let (exit, stdout, _) = efr(&ctx, &["config", "unset", "model.name"]).await;
    assert_eq!(exit, Exit::Success);
    assert!(stdout.starts_with(&format!("{}: removed model.name", path.display())), "{stdout}");
    let (_, stdout, _) = efr(&ctx, &["config", "unset", "model.name"]).await;
    assert!(stdout.starts_with(&format!("{}: model.name was not set", path.display())), "{stdout}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# mine\n"), "{text}");
    assert!(text.contains("max_queued = 4"), "{text}");
}

#[tokio::test]
async fn a_value_the_daemon_would_refuse_is_never_written() {
    let env = TestEnv::new();
    let ctx = env.context();
    let path = env.dirs.config().join("config.toml");
    std::fs::write(&path, "[shell]\nlogin = true\n").unwrap();

    for (args, expected) in [
        (&["config", "set", "shell.login", "maybe"][..], "true or false"),
        (&["config", "set", "conversation.max_queued", "0"][..], "conversation.max_queued"),
        (&["config", "set", "shell.colour", "red"][..], "shell.colour"),
        (&["config", "set", "render.theme", "neon"][..], "\"neon\" does not exist"),
        (&["config", "set", "render.theme_dark", "auto"][..], "\"auto\" does not exist"),
        (&["config", "set", "render.colors.accent", "purple"][..], "render.colors.accent"),
        (&["config", "set", "render.colors.diff.add", "16"][..], "render.colors.diff.add"),
        (&["config", "unset", "permissions.rules"][..], "permissions.rules"),
    ] {
        let (exit, _, stderr) = efr(&ctx, args).await;
        assert_eq!(exit, Exit::Invalid, "{args:?}");
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[shell]\nlogin = true\n");
}

#[tokio::test]
async fn set_writes_the_file_behind_a_link_and_creates_a_missing_one_from_the_example() {
    let env = TestEnv::new();
    let ctx = env.context();
    let path = env.dirs.config().join("config.toml");

    let (exit, _, _) = efr(&ctx, &["config", "set", "shell.idle_minutes", "15"]).await;
    assert_eq!(exit, Exit::Success);
    let (exit, _, stderr) = efr(&ctx, &["config", "set", "render.theme", "auto"]).await;
    assert_eq!(exit, Exit::Success, "{stderr}");
    let created = std::fs::read_to_string(&path).unwrap();
    assert!(created.starts_with("#:schema "), "{created}");
    assert!(created.contains("idle_minutes = 15"), "{created}");

    let target = env.dirs.data().join("dotfiles.toml");
    std::fs::rename(&path, &target).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let (exit, stdout, _) = efr(&ctx, &["config", "set", "shell.idle_minutes", "20"]).await;
    assert_eq!(exit, Exit::Success);
    assert!(stdout.starts_with(&format!("{}: set", target.canonicalize().unwrap().display())));
    assert!(std::fs::symlink_metadata(&path).unwrap().is_symlink());
    assert!(std::fs::read_to_string(&target).unwrap().contains("idle_minutes = 20"));
}

#[tokio::test]
async fn reload_prints_the_daemons_outcome_and_a_refused_file_exits_1() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let answers = [
        AdminConfigReloadResult {
            applied: true,
            error: None,
            restart_needed: vec!["screen".to_owned()],
        },
        AdminConfigReloadResult {
            applied: false,
            error: Some(ConfigFileError {
                message: "the config file is not valid: expected `]`".to_owned(),
                line: Some(1),
                column: Some(7),
                key: None,
            }),
            restart_needed: vec![],
        },
    ];
    let script = async {
        for answer in &answers {
            let mut conn = daemon.accept().await;
            let (id, method) = conn.request().await;
            assert!(matches!(method, Method::AdminConfigReload(_)), "{}", method.name());
            conn.reply(id, answer).await;
            conn.until_closed().await;
        }
    };
    let run = async {
        let first = efr(&ctx, &["config", "reload"]).await;
        let second = efr(&ctx, &["config", "reload"]).await;
        (first, second)
    };
    let (((first, applied, _), (second, refused, _)), ()) = tokio::join!(run, script);

    assert_eq!(first, Exit::Success);
    assert_eq!(
        applied,
        "config.toml reloaded; new turns use it\nrestart efrd to apply: screen (systemctl --user restart efrd)\n"
    );
    assert_eq!(second, Exit::Invalid);
    assert_eq!(
        refused,
        "config.toml has an error; the old settings stay: the config file is not valid: expected `]` (line 1, column 7)\n"
    );
}

#[tokio::test]
async fn edit_creates_the_file_from_the_example_and_checks_what_the_editor_left() {
    let env = TestEnv::new();
    let path = env.dirs.config().join("config.toml");
    // The editor finds the example, then saves a file of its own.
    let editor = "sh -c 'grep -q \"^#:schema \" \"$1\" && printf \"[conversation]\\nmax_queued = 3\\n\" > \"$1\"' editor";
    let ctx = Context {
        editor: Some(editor.to_owned()),
        cwd: Some(env.dirs.data().to_path_buf()),
        ..env.context()
    };

    let (exit, stdout, stderr) = efr(&ctx, &["config", "edit"]).await;

    assert_eq!(exit, Exit::Success, "{stderr}");
    assert_eq!(stderr, format!("efr: created {} from the example\n", path.display()));
    assert_eq!(stdout, "the daemon is not running; it reads the file when it starts\n");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[conversation]\nmax_queued = 3\n");
}

#[tokio::test]
async fn an_edit_that_leaves_an_error_says_so_and_exits_1_without_a_terminal_to_ask() {
    let env = TestEnv::new();
    let path = env.dirs.config().join("config.toml");
    std::fs::write(&path, "[shell]\n").unwrap();
    let editor = "sh -c 'printf \"idle_minuets = 5\\n\" >> \"$1\"' editor";
    let ctx = Context {
        editor: Some(editor.to_owned()),
        cwd: Some(env.dirs.data().to_path_buf()),
        ..env.context()
    };

    let (exit, stdout, stderr) = efr(&ctx, &["config", "edit"]).await;

    assert_eq!(exit, Exit::Invalid);
    assert_eq!(stdout, "");
    assert!(
        stderr.starts_with(&format!("efr: {}: unknown field `idle_minuets`", path.display())),
        "{stderr}"
    );
    assert!(
        stderr.ends_with("efr: the daemon keeps its old settings until the file is fixed\n"),
        "{stderr}"
    );
}

#[tokio::test]
async fn an_editor_that_fails_leaves_the_file_unchecked() {
    let env = TestEnv::new();
    std::fs::write(env.dirs.config().join("config.toml"), "").unwrap();
    let ctx = Context {
        editor: Some("false".to_owned()),
        cwd: Some(env.dirs.data().to_path_buf()),
        ..env.context()
    };

    let (exit, _, stderr) = efr(&ctx, &["config", "edit"]).await;

    assert_eq!(exit, Exit::DaemonError);
    assert!(stderr.starts_with("efr: the editor \"false\" exited with exit status: 1"), "{stderr}");
}

#[tokio::test]
async fn edit_opens_the_file_behind_a_link_so_an_editor_that_replaces_files_keeps_the_link() {
    let env = TestEnv::new();
    let path = env.dirs.config().join("config.toml");
    let dotfiles = env.dirs.data().join("dotfiles");
    std::fs::create_dir_all(&dotfiles).unwrap();
    let real = dotfiles.join("config.toml");
    std::fs::write(&real, "# mine\n").unwrap();
    std::os::unix::fs::symlink(&real, &path).unwrap();
    let opened = env.dirs.data().join("opened");
    // Saves as many editors do: a new file renamed over the one it was given.
    let editor = format!(
        "sh -c 'printf \"%s\" \"$1\" > {opened} && printf \"[conversation]\\nmax_queued = 3\\n\" > \"$1.new\" && mv \"$1.new\" \"$1\"' editor",
        opened = opened.display()
    );
    let ctx =
        Context { editor: Some(editor), cwd: Some(env.dirs.data().to_path_buf()), ..env.context() };

    let (exit, _, stderr) = efr(&ctx, &["config", "edit"]).await;

    assert_eq!(exit, Exit::Success, "{stderr}");
    let resolved = std::fs::canonicalize(&real).unwrap();
    assert_eq!(std::fs::read_to_string(&opened).unwrap(), resolved.display().to_string());
    assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read_link(&path).unwrap(), real);
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "[conversation]\nmax_queued = 3\n");
}

#[tokio::test]
async fn edit_refuses_a_link_to_nothing() {
    let env = TestEnv::new();
    let path = env.dirs.config().join("config.toml");
    std::os::unix::fs::symlink(env.dirs.data().join("gone.toml"), &path).unwrap();
    let ctx = Context { editor: Some("true".to_owned()), ..env.context() };

    let (exit, _, stderr) = efr(&ctx, &["config", "edit"]).await;

    assert_eq!(exit, Exit::DaemonError);
    assert!(stderr.contains("which does not exist"), "{stderr}");
    assert!(!env.dirs.data().join("gone.toml").exists());
}

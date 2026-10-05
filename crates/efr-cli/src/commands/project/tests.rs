use std::path::PathBuf;

use efr_protocol::{
    AdminConfigReloadResult, AdminProjectAdd, AdminProjectAddResult, AdminProjectRemove,
    AdminProjectRemoveResult, ConfigFileError, ErrorBody, ErrorCode, Method, Origin, ProjectInfo,
    ProjectsListResult,
};
use pretty_assertions::assert_eq;

use super::listing;
use crate::context::Context;
use crate::error::Exit;
use crate::run;
use crate::testing::{TestEnv, capture, command};

fn app() -> ProjectInfo {
    ProjectInfo {
        id: "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap(),
        root: "/home/user/project".into(),
        name: Some("project".to_owned()),
    }
}

fn applied() -> AdminConfigReloadResult {
    AdminConfigReloadResult { applied: true, error: None, restart_needed: Vec::new() }
}

/// Runs `efr <args>` against a fake daemon that expects `expected` and answers with
/// `answer`, and returns the exit code, stdout and stderr.
async fn against<T: serde::Serialize>(
    ctx: &Context,
    env: &TestEnv,
    args: &[&str],
    expected: Method,
    answer: Result<T, ErrorBody>,
) -> (Exit, String, String) {
    let daemon = env.listen();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        assert_eq!(conn.hello().origin, Origin::Cli);
        let (id, method) = conn.request().await;
        assert_eq!(method, expected);
        match &answer {
            Ok(value) => conn.reply(id, value).await,
            Err(body) => conn.fail(id, body.clone()).await,
        }
        conn.until_closed().await;
    };
    let line = command(args);
    let (exit, ()) = tokio::join!(run::run(&line, ctx, &mut out), script);
    (exit, captured.stdout(), captured.stderr())
}

#[test]
fn the_listing_aligns_the_roots_and_marks_a_project_without_a_name() {
    let list = ProjectsListResult {
        file: "/home/user/.config/efr/projects.toml".into(),
        projects: vec![
            app(),
            ProjectInfo {
                id: "0192f0c1-7a00-7000-8000-000000000002".parse().unwrap(),
                root: "/etc/nixos".into(),
                name: None,
            },
            ProjectInfo {
                id: "0192f0c1-7a00-7000-8000-000000000003".parse().unwrap(),
                root: "/srv/site".into(),
                name: Some("site\x1b]52;c;aGk=\x07".to_owned()),
            },
        ],
    };
    let text = listing(&list);
    assert!(!text.contains('\x1b'));
    insta::assert_snapshot!(text);
}

#[test]
fn an_empty_listing_says_how_to_add_one() {
    let list = ProjectsListResult {
        file: "/home/user/.config/efr/projects.toml".into(),
        projects: Vec::new(),
    };
    assert_eq!(
        listing(&list),
        "no project is registered in /home/user/.config/efr/projects.toml; efr project add registers the one you are in\n"
    );
}

#[tokio::test]
async fn add_without_a_path_asks_for_the_work_tree_of_the_current_directory() {
    let env = TestEnv::new();
    let ctx = env.context();
    let expected = Method::AdminProjectAdd(AdminProjectAdd {
        path: "/home/user/project".into(),
        name: None,
        git_root: true,
    });
    let answer = AdminProjectAddResult {
        project: app(),
        file: "/home/user/.config/efr/projects.toml".into(),
        reload: applied(),
    };

    let (exit, stdout, stderr) =
        against(&ctx, &env, &["project", "add"], expected, Ok(answer)).await;

    assert_eq!(exit, Exit::Success, "{stderr}");
    assert_eq!(stdout, "registered the project project (/home/user/project)\n");
    assert_eq!(stderr, "");
}

#[tokio::test]
async fn add_with_a_path_registers_it_against_the_current_directory_with_its_name() {
    let env = TestEnv::new();
    let ctx = env.context();
    let expected = Method::AdminProjectAdd(AdminProjectAdd {
        path: "/home/user/project/../site".into(),
        name: Some("site".to_owned()),
        git_root: false,
    });
    let answer = AdminProjectAddResult {
        project: ProjectInfo { root: "/home/user/site".into(), name: Some("site".into()), ..app() },
        file: "/home/user/.config/efr/projects.toml".into(),
        reload: AdminConfigReloadResult {
            applied: false,
            error: Some(ConfigFileError {
                message: "unknown field `idle_minuets`".to_owned(),
                line: Some(2),
                column: Some(1),
                key: None,
            }),
            restart_needed: Vec::new(),
        },
    };

    let args = ["project", "add", "../site", "--name", "site"];
    let (exit, stdout, stderr) = against(&ctx, &env, &args, expected, Ok(answer)).await;

    assert_eq!(exit, Exit::Success, "the registry was written: {stderr}");
    assert_eq!(stdout, "registered the project site (/home/user/site)\n");
    assert!(
        stderr.starts_with("efr: config.toml has an error, so the permissions keep the old projects until a reload succeeds: unknown field `idle_minuets`"),
        "{stderr}"
    );
}

#[tokio::test]
async fn remove_names_the_project_it_took_out() {
    let env = TestEnv::new();
    let ctx = env.context();
    let expected =
        Method::AdminProjectRemove(AdminProjectRemove { path: "/home/user/project/sub".into() });
    let answer = AdminProjectRemoveResult {
        project: ProjectInfo { name: None, ..app() },
        file: "/home/user/.config/efr/projects.toml".into(),
        reload: applied(),
    };

    let (exit, stdout, stderr) =
        against(&ctx, &env, &["project", "remove", "sub"], expected, Ok(answer)).await;

    assert_eq!(exit, Exit::Success, "{stderr}");
    assert_eq!(stdout, "removed the project at /home/user/project\n");
}

#[tokio::test]
async fn a_refusal_of_the_daemon_is_its_message_and_exit_one() {
    let env = TestEnv::new();
    let ctx = env.context();
    let expected =
        Method::AdminProjectRemove(AdminProjectRemove { path: "/home/user/elsewhere".into() });
    let body = ErrorBody::new(ErrorCode::NotFound, "no project has the root /home/user/elsewhere");

    let args = ["project", "remove", "/home/user/elsewhere"];
    let (exit, stdout, stderr) =
        against::<AdminProjectRemoveResult>(&ctx, &env, &args, expected, Err(body)).await;

    assert_eq!(exit, Exit::DaemonError);
    assert_eq!(stdout, "");
    assert!(stderr.contains("no project has the root /home/user/elsewhere"), "{stderr}");
}

#[tokio::test]
async fn without_a_current_directory_a_relative_path_is_a_usage_error() {
    let env = TestEnv::new();
    let ctx = Context { cwd: None, ..env.context() };
    for args in [&["project", "add"][..], &["project", "remove", "sub"]] {
        let (mut out, captured) = capture();
        let exit = run::run(&command(args), &ctx, &mut out).await;
        assert_eq!(exit, Exit::Usage, "{args:?}");
        assert!(captured.stderr().contains("the current directory is unknown"), "{args:?}");
    }
}

#[tokio::test]
async fn without_a_daemon_project_exits_with_three() {
    let env = TestEnv::new();
    let (mut out, captured) = capture();
    let exit = run::run(&command(&["project", "list"]), &env.context(), &mut out).await;
    assert_eq!(exit, Exit::NotRunning);
    assert!(captured.stderr().contains("systemctl --user start efrd"), "{}", captured.stderr());
}

#[test]
fn an_absolute_path_is_sent_as_given() {
    let env = TestEnv::new();
    let ctx = env.context();
    assert_eq!(
        super::absolute(&ctx, std::path::Path::new("/etc/nixos")).unwrap(),
        PathBuf::from("/etc/nixos")
    );
}

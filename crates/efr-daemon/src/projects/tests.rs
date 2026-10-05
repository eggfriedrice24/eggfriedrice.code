use std::path::{Path, PathBuf};

use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminProjectRemove, AdminProjectRemoveResult,
    ErrorCode, Method, ProjectsList, ProjectsListResult,
};
use efr_test_support::{TestClock, TestDirs};
use pretty_assertions::assert_eq;

use crate::testing::{RawClient, Running, serve};

fn add(path: &Path) -> Method {
    Method::AdminProjectAdd(AdminProjectAdd {
        path: path.to_path_buf(),
        name: None,
        git_root: false,
    })
}

fn add_here(path: &Path) -> Method {
    Method::AdminProjectAdd(AdminProjectAdd {
        path: path.to_path_buf(),
        name: None,
        git_root: true,
    })
}

fn remove(path: &Path) -> Method {
    Method::AdminProjectRemove(AdminProjectRemove { path: path.to_path_buf() })
}

fn registry(dirs: &TestDirs) -> PathBuf {
    dirs.dirs().config().join("projects.toml")
}

fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap()
}

async fn list(client: &mut RawClient) -> Vec<PathBuf> {
    let listed: ProjectsListResult =
        client.call(Method::ProjectsList(ProjectsList {})).await.unwrap();
    listed.projects.into_iter().map(|project| project.root).collect()
}

async fn stop(daemon: Running, client: RawClient) {
    drop(client);
    daemon.stop().await;
}

/// Makes `dir` a git work tree, with the system and global configuration off.
async fn git_init(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let status = efr_stdx::process::command("git", dir)
        .args(["init", "-q"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .await
        .unwrap();
    assert!(status.success());
}

#[tokio::test]
async fn a_registered_project_is_in_the_file_the_list_and_the_engine() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = serve(&dirs, &clock).await;
    let (mut client, _) = RawClient::hello(&daemon.socket, None).await;
    let app = dirs.home().join("p").join("app");
    std::fs::create_dir_all(&app).unwrap();

    let added: AdminProjectAddResult = client.call(add(&app)).await.unwrap();

    assert_eq!(added.project.root, real(&app));
    assert_eq!(added.project.name.as_deref(), Some("app"));
    assert_eq!(added.file, registry(&dirs));
    assert!(added.reload.applied);
    let text = std::fs::read_to_string(registry(&dirs)).unwrap();
    assert!(text.starts_with("# The projects that efr knows."), "{text}");
    assert_eq!(list(&mut client).await, [real(&app)]);
    let engine = daemon.engine.borrow().clone();
    assert_eq!(engine.locations().project_root(&added.project.id), Some(real(&app).as_path()));

    stop(daemon, client).await;
}

#[tokio::test]
async fn a_removed_project_leaves_the_file_with_its_comments_and_the_engine() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let app = dirs.home().join("app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        registry(&dirs),
        format!(
            "# Mine.\n\n[[project]]\nid = \"0192f0c1-7a00-7000-8000-000000000001\"\nroot = \"{}\"\n",
            real(&app).display()
        ),
    )
    .unwrap();
    let daemon = serve(&dirs, &clock).await;
    let (mut client, _) = RawClient::hello(&daemon.socket, None).await;
    let id = "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap();
    assert!(daemon.engine.borrow().locations().project_root(&id).is_some());

    let removed: AdminProjectRemoveResult = client.call(remove(&app)).await.unwrap();

    assert_eq!(removed.project.id, id);
    assert!(removed.reload.applied);
    assert_eq!(list(&mut client).await, Vec::<PathBuf>::new());
    assert_eq!(std::fs::read_to_string(registry(&dirs)).unwrap(), "# Mine.\n\n");
    assert_eq!(daemon.engine.borrow().locations().project_root(&id), None);
    let again = client.call::<AdminProjectRemoveResult>(remove(&app)).await.unwrap_err();
    assert_eq!(again.code, ErrorCode::NotFound);
    assert_eq!(again.message, format!("no project has the root {}", app.display()));

    stop(daemon, client).await;
}

#[tokio::test]
async fn from_a_directory_inside_a_work_tree_the_root_of_the_work_tree_is_registered() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let repo = dirs.home().join("p").join("repo");
    git_init(&repo).await;
    let inside = repo.join("crates").join("x");
    std::fs::create_dir_all(&inside).unwrap();
    let plain = dirs.home().join("notes");
    std::fs::create_dir_all(&plain).unwrap();
    let daemon = serve(&dirs, &clock).await;
    let (mut client, _) = RawClient::hello(&daemon.socket, None).await;

    let from_inside: AdminProjectAddResult = client.call(add_here(&inside)).await.unwrap();
    let from_plain: AdminProjectAddResult = client.call(add_here(&plain)).await.unwrap();

    assert_eq!(from_inside.project.root, real(&repo));
    assert_eq!(from_inside.project.name.as_deref(), Some("repo"));
    assert_eq!(from_plain.project.root, real(&plain), "no work tree: the directory itself");

    stop(daemon, client).await;
}

#[tokio::test]
async fn the_home_directory_is_registered_only_when_it_is_named() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = serve(&dirs, &clock).await;
    let (mut client, _) = RawClient::hello(&daemon.socket, None).await;

    let refused = client.call::<AdminProjectAddResult>(add_here(dirs.home())).await.unwrap_err();
    assert_eq!(refused.code, ErrorCode::Invalid);
    assert!(refused.message.contains("name it to register it"), "{}", refused.message);
    assert!(!registry(&dirs).exists());

    let named: AdminProjectAddResult = client.call(add(dirs.home())).await.unwrap();
    assert_eq!(named.project.root, real(dirs.home()));

    stop(daemon, client).await;
}

#[tokio::test]
async fn a_missing_directory_a_relative_path_and_a_second_registration_are_refused() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = serve(&dirs, &clock).await;
    let (mut client, _) = RawClient::hello(&daemon.socket, None).await;
    let app = dirs.home().join("app");

    let missing = client.call::<AdminProjectAddResult>(add(&app)).await.unwrap_err();
    assert_eq!(missing.code, ErrorCode::Invalid);
    assert_eq!(missing.message, format!("{} is not a directory", app.display()));

    let relative = client.call::<AdminProjectAddResult>(add(Path::new("app"))).await.unwrap_err();
    assert_eq!(relative.code, ErrorCode::Invalid);

    std::fs::create_dir_all(&app).unwrap();
    client.call::<AdminProjectAddResult>(add(&app)).await.unwrap();
    let twice = client.call::<AdminProjectAddResult>(add(&app)).await.unwrap_err();
    assert_eq!(twice.code, ErrorCode::Conflict);
    assert!(twice.message.contains("registered twice"), "{}", twice.message);

    stop(daemon, client).await;
}

#[tokio::test]
async fn a_registry_with_an_error_is_left_alone_and_the_error_says_what_to_fix() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let text = "[[project]]\nid = \"0192f0c1-7a00-7000-8000-000000000001\"\nroot = \"/a\"\ntitle = \"a\"\n";
    std::fs::write(registry(&dirs), text).unwrap();
    let daemon = serve(&dirs, &clock).await;
    let (mut client, _) = RawClient::hello(&daemon.socket, None).await;

    let refused = client.call::<AdminProjectAddResult>(add(dirs.home())).await.unwrap_err();

    assert_eq!(refused.code, ErrorCode::Invalid);
    assert!(
        refused.message.starts_with("could not parse the project registry"),
        "{}",
        refused.message
    );
    assert!(refused.message.contains("unknown field `title`"), "{}", refused.message);
    assert_eq!(std::fs::read_to_string(registry(&dirs)).unwrap(), text);

    stop(daemon, client).await;
}

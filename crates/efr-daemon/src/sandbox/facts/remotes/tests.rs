//! The git remote facts: the parse of git's config output and of remote URLs, the URL
//! rewrites, and the read of real repositories with the project snapshot. The
//! repositories use `.test` hosts; no git command reaches a remote.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_permissions::{RemoteFact, RemoteUrl};
use efr_protocol::{ConversationId, ProjectId};
use efr_scope::{Git, Home, Registry};
use efr_stdx::paths::Dirs;
use efr_test_support::{TestClock, TestRng, TestStore};
use pretty_assertions::assert_eq;

use super::{Entry, parse_entries, remote_url, remotes};
use crate::sandbox::{HostFacts, SandboxService, Seams, ServiceParts};

fn url(host: &str, port: u16, ssh: bool, credentials: bool) -> RemoteUrl {
    RemoteUrl { host: host.to_owned(), port, ssh, credentials }
}

fn entry(key: &str, value: &str) -> Entry {
    ("file:.git/config".to_owned(), key.to_owned(), value.to_owned())
}

#[test]
fn git_config_output_parses_with_and_without_values() {
    let bytes = b"file:.git/config\0remote.origin.url\nhttps://git.test/r.git\0file:/h/.gitconfig\0credential.helper\nstore\0file:.git/config\0core.bare\0";
    assert_eq!(
        parse_entries(bytes),
        [
            entry("remote.origin.url", "https://git.test/r.git"),
            ("file:/h/.gitconfig".to_owned(), "credential.helper".to_owned(), "store".to_owned()),
            entry("core.bare", ""),
        ]
    );
    assert!(parse_entries(b"").is_empty());
}

#[test]
fn remote_urls_read_as_git_reads_them() {
    assert_eq!(remote_url("https://git.test/r.git"), Some(url("git.test", 443, false, false)));
    assert_eq!(remote_url("http://git.test:8080/r"), Some(url("git.test", 8080, false, false)));
    assert_eq!(remote_url("https://u:tok@git.test/r"), Some(url("git.test", 443, false, true)));
    assert_eq!(remote_url("https://tok@GIT.test/r"), Some(url("git.test", 443, false, true)));
    assert_eq!(remote_url("ssh://git@git.test/r.git"), Some(url("git.test", 22, true, false)));
    assert_eq!(remote_url("ssh://git.test:2222/r"), Some(url("git.test", 2222, true, false)));
    assert_eq!(remote_url("git+ssh://git.test/r"), Some(url("git.test", 22, true, false)));
    assert_eq!(remote_url("git@git.test:o/r.git"), Some(url("git.test", 22, true, false)));
    assert_eq!(remote_url("git.test:r.git"), Some(url("git.test", 22, true, false)));
    assert_eq!(remote_url("git://git.test/r"), Some(url("git.test", 9418, false, false)));
    assert_eq!(remote_url("/srv/git/r.git"), None, "a local path");
    assert_eq!(remote_url("./r/x:y"), None, "a local path with a colon");
    assert_eq!(remote_url("file:///srv/r.git"), None);
    assert_eq!(remote_url("ext::sh -c x"), None, "an unknown transport");
}

#[test]
fn remotes_follow_insteadof_and_push_rules() {
    let entries = [
        entry("remote.origin.url", "gh:o/r.git"),
        entry("remote.mirror.url", "https://one.test/r"),
        entry("remote.mirror.url", "https://two.test/r"),
        entry("remote.both.url", "https://fetch.test/r"),
        entry("remote.both.pushurl", "gh:o/push.git"),
        entry("url.https://github.test/.insteadof", "gh:"),
        entry("url.ssh://git@github.test/.pushinsteadof", "https://one.test/"),
        entry("branch.main.remote", "mirror"),
        entry("branch.other.remote", "origin"),
    ];
    let found = remotes(&entries, Some("main"), &[]);
    let names: Vec<&str> = found.iter().map(|remote| remote.name.as_str()).collect();
    assert_eq!(names, ["origin", "mirror", "both"]);
    let origin = &found[0];
    assert_eq!(origin.fetch, [url("github.test", 443, false, false)], "after insteadOf");
    assert_eq!(origin.push, origin.fetch);
    assert!(!origin.upstream);
    let mirror = &found[1];
    assert!(mirror.upstream, "the remote of the current branch");
    assert_eq!(
        mirror.fetch,
        [url("one.test", 443, false, false), url("two.test", 443, false, false)]
    );
    assert_eq!(
        mirror.push,
        [url("github.test", 22, true, false), url("two.test", 443, false, false)],
        "pushInsteadOf, else insteadOf"
    );
    let both = &found[2];
    assert_eq!(both.fetch, [url("fetch.test", 443, false, false)]);
    assert_eq!(both.push, [url("github.test", 443, false, false)], "each pushurl");
    assert!(found.iter().all(|remote| !remote.project && !remote.submodule));
}

#[test]
fn submodule_urls_are_remotes_of_their_own() {
    let gitmodules = [(
        "file:.gitmodules".to_owned(),
        "submodule.lib.url".to_owned(),
        "git@sub.test:o/lib.git".to_owned(),
    )];
    let entries = [entry("submodule.lib.url", "https://sub.test/o/lib.git")];
    let found = remotes(&entries, None, &gitmodules);
    let [lib] = found.as_slice() else { panic!("{found:?}") };
    assert!(lib.submodule);
    assert_eq!(lib.fetch, [url("sub.test", 443, false, false), url("sub.test", 22, true, false)]);
}

/// Runs git in `dir` without the user's configuration.
async fn git(dir: &Path, args: &[&str]) {
    let status = efr_stdx::process::command("git", dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .await
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// A repository at `dir` with the remote `origin` at `url`.
async fn repo(dir: &Path, url: &str) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]).await;
    git(dir, &["remote", "add", "origin", url]).await;
}

struct World {
    _temp: tempfile::TempDir,
    root: PathBuf,
    service: SandboxService,
    _store: TestStore,
}

/// A sandbox service whose registry holds the project `home/p/app`.
async fn world() -> World {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let home = root.join("home");
    let dirs = Dirs::new(root.join("c"), root.join("d"), root.join("s"), root.join("r")).unwrap();
    std::fs::create_dir_all(home.join("p/app")).unwrap();
    std::fs::create_dir_all(dirs.config()).unwrap();
    let mut registry = Registry::empty();
    registry
        .register(ProjectId::from_uuid(uuid::Uuid::from_u128(9)), home.join("p/app"), None)
        .unwrap();
    registry.save(&dirs.config().join("projects.toml")).unwrap();
    let clock = TestClock::new().shared();
    let store = TestStore::open(Arc::clone(&clock)).await.unwrap();
    let service = SandboxService::new(ServiceParts {
        dirs: dirs.clone(),
        home: Home::new(&home).unwrap(),
        host: HostFacts::default(),
        source: None,
        seams: Seams::default(),
        registry: dirs.config().join("projects.toml"),
        writer: store.writer().clone(),
        clock: Arc::clone(&clock),
        rng: Arc::new(TestRng::new(4)),
        git: Git::new(clock).isolated(),
    })
    .await;
    World { _temp: temp, root, service, _store: store }
}

fn conversation(n: u128) -> ConversationId {
    ConversationId::from_uuid(uuid::Uuid::from_u128(n))
}

fn remote<'a>(found: &'a [(PathBuf, Vec<RemoteFact>)], dir: &Path) -> &'a RemoteFact {
    let (_, remotes) = found.iter().find(|(known, _)| known == dir).unwrap();
    &remotes[0]
}

#[tokio::test]
async fn remote_facts_read_once_per_repository() {
    let world = world().await;
    let app = world.root.join("home/p/app");
    repo(&app, "https://git.test/app.git").await;
    std::fs::create_dir_all(app.join("src")).unwrap();
    let dirs = [app.clone(), app.join("src")];
    let found = world.service.remote_facts(conversation(1), &dirs).await;
    assert_eq!(found.len(), 2, "a fact for each directory");
    assert_eq!(found[0].1, found[1].1, "one read of the repository serves both");
    let origin = remote(&found, &app);
    assert_eq!(origin.name, "origin");
    assert_eq!(origin.fetch, [url("git.test", 443, false, false)]);
    assert!(origin.project, "the remote of a registered project");
}

#[tokio::test]
async fn remote_snapshot_taken_at_first_call() {
    let world = world().await;
    let app = world.root.join("home/p/app");
    repo(&app, "https://git.test/app.git").await;
    let dirs = [app.clone()];
    // NOTE: the first fact request of the conversation names no remote; it takes the
    // stamps of the project's config.
    assert!(world.service.remote_facts(conversation(1), &[]).await.is_empty());
    git(&app, &["remote", "set-url", "origin", "https://other.test/app.git"]).await;
    let changed = world.service.remote_facts(conversation(1), &dirs).await;
    let origin = remote(&changed, &app);
    assert_eq!(origin.fetch, [url("other.test", 443, false, false)]);
    assert!(!origin.project, "the config changed after the conversation's first call");
    let fresh = world.service.remote_facts(conversation(2), &dirs).await;
    assert!(remote(&fresh, &app).project, "a new conversation takes a new snapshot");
    git(&app, &["remote", "set-url", "origin", "https://third.test/app.git"]).await;
    let later = world.service.remote_facts(conversation(2), &dirs).await;
    assert!(!remote(&later, &app).project, "the URLs differ from the snapshot");
}

#[tokio::test]
async fn nested_repo_remote_is_not_project() {
    let world = world().await;
    let app = world.root.join("home/p/app");
    repo(&app, "https://git.test/app.git").await;
    let nested = app.join("vendor/lib");
    repo(&nested, "https://git.test/lib.git").await;
    let found = world.service.remote_facts(conversation(1), &[nested.clone(), app.clone()]).await;
    let lib = remote(&found, &nested);
    assert_eq!(lib.fetch, [url("git.test", 443, false, false)]);
    assert!(!lib.project, "a nested repository is not the project's git dir");
    assert!(remote(&found, &app).project);
}

#[tokio::test]
async fn a_directory_outside_a_repository_has_no_remote() {
    let world = world().await;
    let plain = world.root.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let found = world.service.remote_facts(conversation(1), std::slice::from_ref(&plain)).await;
    assert!(found.iter().all(|(_, remotes)| remotes.is_empty()), "{found:?}");
}

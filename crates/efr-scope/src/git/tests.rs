use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pretty_assertions::assert_eq;

use super::{DEFAULT_GIT_TIMEOUT, Discovery, Git, Repo};
use crate::testing::{InstantClock, Sandbox, StoppedClock, git};
use crate::{Home, ScopeError};

#[tokio::test]
async fn a_repository_is_found_from_inside() {
    let sandbox = Sandbox::new();
    let app = sandbox.init(&sandbox.in_home("p/app")).await;
    let src = sandbox.mkdir(&app.join("src/bin"));
    let expected = Discovery::WorkTree(Repo { root: app.clone(), branch: Some("main".into()) });
    assert_eq!(git().discover(&app, sandbox.home()).await.unwrap(), expected);
    assert_eq!(git().discover(&src, sandbox.home()).await.unwrap(), expected);
}

#[tokio::test]
async fn a_repository_outside_home_is_found() {
    let sandbox = Sandbox::new();
    let site = sandbox.init(&sandbox.root().join("srv/site")).await;
    let discovery = git().discover(&site.join("."), sandbox.home()).await.unwrap();
    assert_eq!(discovery.work_tree().map(|repo| repo.root.as_path()), Some(site.as_path()));
}

#[tokio::test]
async fn a_dotfiles_home_is_not_found_from_below() {
    let sandbox = Sandbox::new();
    sandbox.init(sandbox.home().path()).await;
    let nvim = sandbox.mkdir(&sandbox.in_home(".config/nvim"));
    assert_eq!(git().discover(&nvim, sandbox.home()).await.unwrap(), Discovery::NotARepository);
}

#[tokio::test]
async fn a_dotfiles_home_is_guarded_from_home_itself() {
    let sandbox = Sandbox::new();
    sandbox.init(sandbox.home().path()).await;
    assert_eq!(
        git().discover(sandbox.home().path(), sandbox.home()).await.unwrap(),
        Discovery::Guarded { root: sandbox.home().path().to_path_buf() }
    );
}

#[tokio::test]
async fn a_repository_above_home_is_guarded_and_not_climbed_into() {
    let sandbox = Sandbox::new();
    // The sandbox root holds the home directory, like a repository at /home would.
    sandbox.init(sandbox.root()).await;
    assert_eq!(
        git().discover(sandbox.root(), sandbox.home()).await.unwrap(),
        Discovery::Guarded { root: sandbox.root().to_path_buf() }
    );
    let notes = sandbox.mkdir(&sandbox.in_home("notes"));
    assert_eq!(git().discover(&notes, sandbox.home()).await.unwrap(), Discovery::NotARepository);
}

#[tokio::test]
async fn a_repository_inside_a_dotfiles_home_is_its_own_work_tree() {
    let sandbox = Sandbox::new();
    sandbox.init(sandbox.home().path()).await;
    let app = sandbox.init(&sandbox.in_home("p/app")).await;
    let discovery = git().discover(&app, sandbox.home()).await.unwrap();
    assert_eq!(discovery.work_tree().map(|repo| repo.root.as_path()), Some(app.as_path()));
}

#[tokio::test]
async fn a_detached_head_has_no_branch() {
    let sandbox = Sandbox::new();
    let app = sandbox.init(&sandbox.in_home("app")).await;
    sandbox.git(&app, &["commit", "--quiet", "--allow-empty", "-m", "first"]).await;
    sandbox.git(&app, &["checkout", "--quiet", "--detach"]).await;
    assert_eq!(
        git().discover(&app, sandbox.home()).await.unwrap(),
        Discovery::WorkTree(Repo { root: app.clone(), branch: None })
    );
}

#[tokio::test]
async fn a_linked_directory_reports_the_resolved_root() {
    let sandbox = Sandbox::new();
    let app = sandbox.init(&sandbox.in_home("p/app")).await;
    let link = sandbox.in_home("app-link");
    std::os::unix::fs::symlink(&app, &link).unwrap();
    let discovery = git().discover(&link, sandbox.home()).await.unwrap();
    assert_eq!(discovery.work_tree().map(|repo| repo.root.as_path()), Some(app.as_path()));
}

#[tokio::test]
async fn plain_and_missing_directories_are_no_repository() {
    let sandbox = Sandbox::new();
    let plain = sandbox.mkdir(&sandbox.in_home("Documents"));
    assert_eq!(git().discover(&plain, sandbox.home()).await.unwrap(), Discovery::NotARepository);
    let missing = sandbox.in_home("gone");
    assert_eq!(git().discover(&missing, sandbox.home()).await.unwrap(), Discovery::NotARepository);
}

#[tokio::test]
async fn inside_a_git_directory_is_no_work_tree() {
    let sandbox = Sandbox::new();
    let app = sandbox.init(&sandbox.in_home("app")).await;
    assert_eq!(
        git().discover(&app.join(".git"), sandbox.home()).await.unwrap(),
        Discovery::NotARepository
    );
}

#[tokio::test]
async fn a_relative_directory_is_an_error() {
    let sandbox = Sandbox::new();
    let error = git().discover(Path::new("p/app"), sandbox.home()).await.unwrap_err();
    assert!(matches!(error, ScopeError::NotAbsolute { path } if path == Path::new("p/app")));
}

#[tokio::test]
async fn git_that_cannot_start_is_an_error() {
    let sandbox = Sandbox::new();
    let git = git().with_program("/nonexistent/efr-test/git");
    let error = git.discover(sandbox.home().path(), sandbox.home()).await.unwrap_err();
    assert!(
        matches!(error, ScopeError::RunGit { program, .. } if program == "/nonexistent/efr-test/git")
    );
}

#[tokio::test]
async fn git_that_hangs_times_out_without_waiting() {
    let sandbox = Sandbox::new();
    let script = sandbox.root().join("hanging-git");
    std::fs::write(&script, "#!/bin/sh\nexec sleep 600\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cwd = sandbox.mkdir(&sandbox.in_home("p"));
    // `run` and not `discover`: with a clock that fires at once, the look at `cwd` that
    // comes first in `discover` would time out before git starts.
    let git = Git::new(Arc::new(InstantClock)).isolated().with_program(&script);
    let error = git.run(&cwd, sandbox.home(), ["rev-parse"]).await.unwrap_err();
    assert!(matches!(error, ScopeError::GitTimedOut { after } if after == DEFAULT_GIT_TIMEOUT));
}

#[tokio::test]
async fn a_probe_runs_on_the_blocking_pool_and_not_on_the_calling_worker() {
    let caller = std::thread::current().id();
    let ran_on = git().probe(Path::new("/"), || std::thread::current().id()).await.unwrap();
    assert_ne!(ran_on, caller);
}

#[tokio::test]
async fn a_probe_that_hangs_times_out_and_lets_the_turn_go_on() {
    let (release, hold) = std::sync::mpsc::channel::<()>();
    let git = Git::new(Arc::new(InstantClock));
    let error = git.probe(Path::new("/mnt/hung"), move || hold.recv()).await.unwrap_err();
    assert!(
        matches!(
            &error,
            ScopeError::InspectTimedOut { path, after }
                if path == Path::new("/mnt/hung") && *after == DEFAULT_GIT_TIMEOUT
        ),
        "{error:?}"
    );
    release.send(()).unwrap();
}

#[test]
fn the_command_runs_in_the_directory_with_a_guarded_environment() {
    let home = Home::new("/home/u").unwrap();
    let git = Git::new(Arc::new(StoppedClock));
    let command = git.command(Path::new("/home/u/p"), &home, ["status"]);
    let command = command.as_std();
    assert_eq!(command.get_program(), "git");
    assert_eq!(command.get_current_dir(), Some(Path::new("/home/u/p")));
    let env = |name: &str| {
        command
            .get_envs()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.map(OsStr::to_owned))
    };
    assert_eq!(env("GIT_CEILING_DIRECTORIES"), Some(Some("/home/u:/".into())));
    assert_eq!(env("HOME"), Some(Some("/home/u".into())));
    assert_eq!(env("GIT_OPTIONAL_LOCKS"), Some(Some("0".into())));
    assert_eq!(env("GIT_TERMINAL_PROMPT"), Some(Some("0".into())));
    for name in
        ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT"]
    {
        assert_eq!(env(name), Some(None), "{name} is removed");
    }
    assert_eq!(env("GIT_CONFIG_GLOBAL"), None);
}

#[test]
fn an_isolated_command_ignores_the_user_configuration() {
    let home = Home::new("/home/u").unwrap();
    let git = Git::new(Arc::new(StoppedClock)).isolated();
    let command = git.command(Path::new("/home/u"), &home, ["status"]);
    let envs: Vec<_> = command.as_std().get_envs().collect();
    assert!(envs.contains(&(OsStr::new("GIT_CONFIG_NOSYSTEM"), Some(OsStr::new("1")))));
    assert!(envs.contains(&(OsStr::new("GIT_CONFIG_GLOBAL"), Some(OsStr::new("/dev/null")))));
    assert!(envs.contains(&(OsStr::new("XDG_CONFIG_HOME"), Some(OsStr::new("/home/u/.config")))));
}

#[test]
fn ceilings_list_both_forms_of_home_and_skip_unlistable_ones() {
    let home = Home::linked("/home/u", "/var/home/u");
    assert_eq!(super::ceilings(&home), "/home/u:/var/home/u:/");
    let colon = Home::linked("/home/a:b", "/home/a:b");
    assert_eq!(super::ceilings(&colon), "/");
}

/// The discovery of v0.0.4, the reference of the one git run: `rev-parse
/// --show-toplevel`, then `symbolic-ref` for the branch of a work tree that is not
/// guarded.
async fn two_run_discovery(git: &Git, cwd: &Path, home: &Home) -> Result<Discovery, ScopeError> {
    let Some(cwd) = crate::home::normalize(cwd) else {
        return Err(ScopeError::NotAbsolute { path: cwd.to_path_buf() });
    };
    if !cwd.is_dir() {
        return Ok(Discovery::NotARepository);
    }
    let Some(root) = git.run(&cwd, home, ["rev-parse", "--show-toplevel"]).await? else {
        return Ok(Discovery::NotARepository);
    };
    let root = PathBuf::from(OsString::from_vec(root));
    if !root.is_absolute() {
        return Ok(Discovery::NotARepository);
    }
    if home.is_at_or_above(&root) {
        return Ok(Discovery::Guarded { root });
    }
    let branch = git
        .run(&cwd, home, ["symbolic-ref", "--quiet", "--short", "HEAD"])
        .await?
        .and_then(|name| String::from_utf8(name).ok());
    Ok(Discovery::WorkTree(Repo { root, branch }))
}

/// A git runner whose program logs each run to a file and then runs the real git.
struct CountingGit {
    git: Git,
    log: PathBuf,
}

impl CountingGit {
    fn new(sandbox: &Sandbox) -> Self {
        let log = sandbox.root().join("git-runs.log");
        let script = sandbox.root().join("counting-git");
        let text = format!("#!/bin/sh\necho \"$1\" >> '{}'\nexec git \"$@\"\n", log.display());
        std::fs::write(&script, text).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        CountingGit { git: git().with_program(&script), log }
    }

    fn runs(&self) -> usize {
        std::fs::read_to_string(&self.log).map_or(0, |log| log.lines().count())
    }

    /// Discovers `cwd` with the one run and with the reference, asserts that both give
    /// the same, and returns the discovery and the git runs of the one run.
    async fn same_as_before(&self, cwd: &Path, home: &Home) -> (Discovery, usize) {
        let before = self.runs();
        let discovery = self.git.discover(cwd, home).await.unwrap();
        let runs = self.runs() - before;
        let reference = two_run_discovery(&git(), cwd, home).await.unwrap();
        assert_eq!(discovery, reference, "in {}", cwd.display());
        (discovery, runs)
    }
}

fn work_tree(root: &Path, branch: Option<&str>) -> Discovery {
    Discovery::WorkTree(Repo { root: root.to_path_buf(), branch: branch.map(str::to_owned) })
}

#[tokio::test]
async fn one_git_run_tells_the_root_and_the_branch() {
    let sandbox = Sandbox::new();
    let counting = CountingGit::new(&sandbox);
    let app = sandbox.init(&sandbox.in_home("p/app")).await;
    sandbox.git(&app, &["commit", "--quiet", "--allow-empty", "-m", "first"]).await;
    let src = sandbox.mkdir(&app.join("src"));
    for cwd in [&app, &src] {
        let found = counting.same_as_before(cwd, sandbox.home()).await;
        assert_eq!(found, (work_tree(&app, Some("main")), 1));
    }
}

#[tokio::test]
async fn a_branch_switch_and_a_detached_head_show_at_the_next_discovery() {
    let sandbox = Sandbox::new();
    let counting = CountingGit::new(&sandbox);
    let app = sandbox.init(&sandbox.in_home("app")).await;
    sandbox.git(&app, &["commit", "--quiet", "--allow-empty", "-m", "first"]).await;
    let home = sandbox.home();
    assert_eq!(counting.same_as_before(&app, home).await, (work_tree(&app, Some("main")), 1));
    sandbox.git(&app, &["checkout", "--quiet", "-b", "feature/x"]).await;
    let found = counting.same_as_before(&app, home).await;
    assert_eq!(found, (work_tree(&app, Some("feature/x")), 1));
    sandbox.git(&app, &["checkout", "--quiet", "--detach"]).await;
    assert_eq!(counting.same_as_before(&app, home).await, (work_tree(&app, None), 1));
    sandbox.git(&app, &["checkout", "--quiet", "main"]).await;
    assert_eq!(counting.same_as_before(&app, home).await, (work_tree(&app, Some("main")), 1));
}

#[tokio::test]
async fn a_new_repository_and_a_removed_one_show_at_the_next_discovery() {
    let sandbox = Sandbox::new();
    let counting = CountingGit::new(&sandbox);
    let home = sandbox.home();
    let dir = sandbox.mkdir(&sandbox.in_home("later"));
    assert_eq!(counting.same_as_before(&dir, home).await, (Discovery::NotARepository, 1));
    sandbox.init(&dir).await;
    // NOTE: with no commit, `HEAD` names no commit, so a second run asks for the
    // branch, as before.
    assert_eq!(counting.same_as_before(&dir, home).await, (work_tree(&dir, Some("main")), 2));
    std::fs::write(dir.join("HEAD"), "a file named HEAD").unwrap();
    assert_eq!(counting.same_as_before(&dir, home).await, (work_tree(&dir, Some("main")), 2));
    sandbox.git(&dir, &["commit", "--quiet", "--allow-empty", "-m", "first"]).await;
    assert_eq!(counting.same_as_before(&dir, home).await, (work_tree(&dir, Some("main")), 1));
    std::fs::remove_dir_all(dir.join(".git")).unwrap();
    assert_eq!(counting.same_as_before(&dir, home).await, (Discovery::NotARepository, 1));
}

#[tokio::test]
async fn a_linked_work_tree_and_a_linked_directory_give_the_same_as_before() {
    let sandbox = Sandbox::new();
    let counting = CountingGit::new(&sandbox);
    let home = sandbox.home();
    let app = sandbox.init(&sandbox.in_home("p/app")).await;
    sandbox.git(&app, &["commit", "--quiet", "--allow-empty", "-m", "first"]).await;
    let linked = sandbox.in_home("p/app-wt");
    let worktree = linked.to_str().unwrap();
    sandbox.git(&app, &["worktree", "add", "--quiet", "-b", "wt", worktree]).await;
    assert!(linked.join(".git").is_file());
    assert_eq!(counting.same_as_before(&linked, home).await, (work_tree(&linked, Some("wt")), 1));
    let link = sandbox.in_home("app-link");
    std::os::unix::fs::symlink(&app, &link).unwrap();
    let below = sandbox.mkdir(&app.join("src"));
    assert_eq!(counting.same_as_before(&link, home).await, (work_tree(&app, Some("main")), 1));
    for cwd in [link.join("src"), below] {
        assert_eq!(counting.same_as_before(&cwd, home).await.0, work_tree(&app, Some("main")));
    }
}

#[tokio::test]
async fn ambiguous_names_give_the_same_as_before() {
    let sandbox = Sandbox::new();
    let counting = CountingGit::new(&sandbox);
    let home = sandbox.home();
    let app = sandbox.init(&sandbox.in_home("app")).await;
    sandbox.git(&app, &["commit", "--quiet", "--allow-empty", "-m", "first"]).await;
    // A tag with the name of the branch: both runs spell the branch `heads/main`.
    sandbox.git(&app, &["tag", "main"]).await;
    let found = counting.same_as_before(&app, home).await;
    assert_eq!(found, (work_tree(&app, Some("heads/main")), 1));
    sandbox.git(&app, &["tag", "-d", "main"]).await;
    // A remote with the name of the branch: `symbolic-ref --short` still says `main`.
    sandbox.git(&app, &["update-ref", "refs/remotes/main/HEAD", "HEAD"]).await;
    assert_eq!(counting.same_as_before(&app, home).await, (work_tree(&app, Some("main")), 1));
    // A tag named HEAD: git names no branch for `HEAD`, so a second run asks for it.
    sandbox.git(&app, &["update-ref", "refs/tags/HEAD", "HEAD"]).await;
    assert_eq!(counting.same_as_before(&app, home).await, (work_tree(&app, Some("main")), 2));
    sandbox.git(&app, &["checkout", "--quiet", "--detach"]).await;
    assert_eq!(counting.same_as_before(&app, home).await, (work_tree(&app, None), 2));
}

#[tokio::test]
async fn a_root_with_a_newline_is_never_split_into_a_root_and_a_branch() {
    let sandbox = Sandbox::new();
    let counting = CountingGit::new(&sandbox);
    let home = sandbox.home();
    let app = sandbox.init(&sandbox.in_home("p/two\nlines")).await;
    sandbox.git(&app, &["commit", "--quiet", "--allow-empty", "-m", "first"]).await;
    assert_eq!(counting.same_as_before(&app, home).await.0, work_tree(&app, Some("main")));
    // NOTE: with a tag named HEAD, git prints the root and no branch. Read one line each,
    // `p/two` would be the root and `lines` the branch.
    sandbox.git(&app, &["update-ref", "refs/tags/HEAD", "HEAD"]).await;
    assert_eq!(counting.same_as_before(&app, home).await.0, work_tree(&app, Some("main")));
    let unborn = sandbox.init(&sandbox.in_home("p/new\n")).await;
    let found = counting.same_as_before(&unborn, home).await.0;
    assert_eq!(found, work_tree(&unborn, Some("main")));
}

#[tokio::test]
async fn a_branch_name_that_is_not_utf8_is_no_branch_as_before() {
    let sandbox = Sandbox::new();
    let counting = CountingGit::new(&sandbox);
    let app = sandbox.init(&sandbox.in_home("app")).await;
    sandbox.git(&app, &["commit", "--quiet", "--allow-empty", "-m", "first"]).await;
    let commit = std::fs::read(app.join(".git/refs/heads/main")).unwrap();
    let name = OsString::from_vec(b"br\xff".to_vec());
    std::fs::write(app.join(".git/refs/heads").join(&name), commit).unwrap();
    std::fs::write(app.join(".git/HEAD"), b"ref: refs/heads/br\xff\n").unwrap();
    assert_eq!(counting.same_as_before(&app, sandbox.home()).await, (work_tree(&app, None), 1));
}

#[tokio::test]
async fn guarded_bare_and_missing_places_give_the_same_as_before() {
    let sandbox = Sandbox::new();
    let counting = CountingGit::new(&sandbox);
    let home = sandbox.home();
    let app = sandbox.init(&sandbox.in_home("app")).await;
    let bare = sandbox.init_bare(&sandbox.in_home("dots.git"), None).await;
    let elsewhere = sandbox.mkdir(&sandbox.in_home("elsewhere"));
    let pointed = sandbox.init_bare(&sandbox.in_home("pointed.git"), elsewhere.to_str()).await;
    for cwd in [app.join(".git"), bare, pointed, elsewhere, sandbox.in_home("gone")] {
        counting.same_as_before(&cwd, home).await;
    }
    sandbox.init(home.path()).await;
    let guarded = counting.same_as_before(home.path(), home).await;
    assert_eq!(guarded, (Discovery::Guarded { root: home.path().to_path_buf() }, 1));
    sandbox.init(sandbox.root()).await;
    let above = counting.same_as_before(sandbox.root(), home).await.0;
    assert_eq!(above, Discovery::Guarded { root: sandbox.root().to_path_buf() });
}

#[test]
fn the_answer_of_the_one_run_is_read_only_in_its_expected_forms() {
    use super::{Answer, Head, read_answer};
    let root = |head| Answer::Root { root: b"/r".to_vec(), head };
    assert_eq!(read_answer(true, b"/r\n/r\nmain\n"), root(Head::Branch(b"main".to_vec())));
    assert_eq!(read_answer(true, b"/r\n/r\nHEAD\n"), root(Head::Detached));
    assert_eq!(read_answer(true, b"/r\n/r\n"), root(Head::Ask));
    assert_eq!(read_answer(false, b"/r\n/r\n"), root(Head::Ask));
    assert_eq!(read_answer(false, b""), Answer::NoRepository);
    let unclear: [&[u8]; 5] =
        [b"", b"/r\n", b"/r\n/s\nmain\n", b"/a\nb\n/a\nb\n", b"/r\n/r\nm\nx\n"];
    for answer in unclear {
        assert_eq!(read_answer(true, answer), Answer::Unclear, "{answer:?}");
    }
    assert_eq!(read_answer(false, b"/r\n/r\nmain\n"), Answer::Unclear);
}

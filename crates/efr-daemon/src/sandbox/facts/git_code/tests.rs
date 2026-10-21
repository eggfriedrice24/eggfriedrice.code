//! The git code facts of a line: what git runs outside the sandbox, from real git runs
//! on repositories in a temp dir, and whether a contained call can write each part.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use efr_permissions::GitCodeAsk;
use efr_protocol::{GitCodeFact, GitCodeFacts};
use efr_scope::{Git, Home};
use efr_stdx::time::Clock;
use efr_test_support::TestClock;
use pretty_assertions::assert_eq;

use super::{CodeInput, Places, collect};

/// A home with a registered project `p` and the `$SCRATCH` base `scratch`.
struct Machine {
    _dir: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    scratch: PathBuf,
}

impl Machine {
    fn new() -> Machine {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        let project = home.join("p");
        let scratch = home.join(".local/share/efr/scratch");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&scratch).unwrap();
        Machine { _dir: dir, home, project, scratch }
    }

    fn places(&self) -> Places {
        Places { open: vec![self.scratch.clone()], projects: vec![self.project.clone()] }
    }

    /// The git code of `asks` with the hidden shell's git variables `env`.
    #[expect(clippy::print_stderr, reason = "the cost of one check is a report of the test")]
    async fn check(
        &self,
        asks: &[GitCodeAsk],
        env: Option<&BTreeMap<String, String>>,
    ) -> GitCodeFacts {
        let clock: Arc<dyn Clock> = TestClock::new().shared();
        let git = Git::new(Arc::clone(&clock)).isolated();
        let home = Home::new(&self.home).unwrap();
        let places = self.places();
        let input = CodeInput {
            git: &git,
            clock: &clock,
            home: &home,
            env,
            shell_path: "/usr/bin:/bin",
            places: &places,
        };
        let begin = Instant::now();
        let facts = collect(asks, &input).await.unwrap();
        // NOTE: check VG1-3 of the v0.0.4 plan: the cost of one check.
        eprintln!("git code check: {} ms", begin.elapsed().as_millis());
        facts
    }
}

/// An ask for the repository in `dir`.
fn ask(dir: &Path, recurse: bool) -> GitCodeAsk {
    GitCodeAsk { dir: dir.to_path_buf(), git_dir: None, work_tree: None, recurse }
}

async fn git(dir: &Path, args: &[&str]) {
    let status = efr_stdx::process::command("git", dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .await
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// A new repository at `dir`.
async fn init(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]).await;
}

/// An executable hook `name` in `hooks`.
fn hook(hooks: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(hooks).unwrap();
    let path = hooks.join(name);
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Appends `text` to the config file `file`.
fn append(file: &Path, text: &str) {
    let mut config = std::fs::read_to_string(file).unwrap_or_default();
    config.push_str(text);
    std::fs::write(file, config).unwrap();
}

/// The part of `facts` with `item`.
fn item<'f>(facts: &'f GitCodeFacts, item: &str) -> Option<&'f GitCodeFact> {
    facts.items.iter().find(|fact| fact.item == item)
}

#[tokio::test]
async fn git_code_follows_a_dot_git_file_into_scratch() {
    let machine = Machine::new();
    let git_dir = machine.scratch.join("c1/r.git");
    init(&git_dir.join("tree")).await;
    // NOTE: the work tree lies in the project, its git dir in a `$SCRATCH`.
    std::fs::rename(git_dir.join("tree/.git"), git_dir.join("store")).unwrap();
    let work = machine.project.join("app");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join(".git"), format!("gitdir: {}\n", git_dir.join("store").display()))
        .unwrap();
    git(&work, &["config", "core.worktree", &work.display().to_string()]).await;
    let planted = hook(&git_dir.join("store/hooks"), "pre-push");

    let facts = machine.check(&[ask(&work, false)], None).await;

    assert!(facts.checked, "{facts:?}");
    let found = item(&facts, "hooks/pre-push").unwrap();
    assert_eq!(found.file, planted);
    assert!(found.writable, "{found:?}");
    assert_eq!(found.repo, work);
}

#[tokio::test]
async fn git_code_dot_git_file_into_another_conversations_scratch_is_writable() {
    let machine = Machine::new();
    // The line runs in one conversation's `$SCRATCH`; the git dir lies in another's.
    let other = machine.scratch.join("c2/x");
    init(&other).await;
    let work = machine.scratch.join("c1/work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join(".git"), format!("gitdir: {}\n", other.join(".git").display()))
        .unwrap();
    let planted = hook(&other.join(".git/hooks"), "pre-push");

    let facts = machine.check(&[ask(&work, false)], None).await;

    assert!(facts.checked, "{facts:?}");
    let found = item(&facts, "hooks/pre-push").unwrap();
    assert_eq!(found.file, planted);
    assert!(found.writable, "{found:?}");
}

#[tokio::test]
async fn git_code_reads_hooks_path_in_the_work_tree() {
    let machine = Machine::new();
    let repo = machine.scratch.join("c1/r");
    init(&repo).await;
    git(&repo, &["config", "core.hooksPath", ".githooks"]).await;
    let planted = hook(&repo.join(".githooks"), "pre-push");
    // A sample of git's own hooks dir runs nothing; that dir is not the hooks dir now.
    hook(&repo.join(".git/hooks"), "pre-commit");

    let facts = machine.check(&[ask(&repo, false)], None).await;

    assert!(facts.checked, "{facts:?}");
    let hooks: Vec<&GitCodeFact> =
        facts.items.iter().filter(|fact| fact.item.starts_with("hooks/")).collect();
    assert_eq!(hooks.len(), 1, "{hooks:?}");
    assert_eq!(hooks[0].file, planted);
    assert!(hooks[0].writable);
    // The key itself is a part: it lies in a config that a contained call can write.
    assert!(item(&facts, "core.hookspath").is_some_and(|fact| fact.writable), "{facts:?}");
}

#[tokio::test]
async fn git_code_pinned_project_hooks_path_is_not_writable() {
    let machine = Machine::new();
    let repo = machine.project.clone();
    init(&repo).await;
    git(&repo, &["config", "core.hooksPath", ".githooks"]).await;
    hook(&repo.join(".githooks"), "pre-push");
    hook(&repo.join(".git/hooks"), "pre-commit");

    let facts = machine.check(&[ask(&repo, false)], None).await;

    // The project's pinned config names the hooks path, so every plan of the project
    // keeps it read-only.
    assert!(facts.clean(), "{facts:?}");
    // The project's own hook dir is pinned as well.
    let pinned = machine.project.join("sub");
    std::fs::create_dir_all(&pinned).unwrap();
    git(&repo, &["config", "--unset", "core.hooksPath"]).await;
    let facts = machine.check(&[ask(&pinned, false)], None).await;
    assert!(item(&facts, "hooks/pre-commit").is_some_and(|fact| !fact.writable), "{facts:?}");
    assert!(facts.clean(), "{facts:?}");
}

#[tokio::test]
async fn git_code_reads_include_path_targets() {
    let machine = Machine::new();
    let repo = machine.project.clone();
    init(&repo).await;
    // NOTE: check VG1-2: git lists the keys of an include file and of an `includeIf`
    // file with `--show-origin`, in the hardened run.
    let include = machine.scratch.join("c1/inc.cfg");
    std::fs::create_dir_all(include.parent().unwrap()).unwrap();
    std::fs::write(&include, "[core]\n\tsshCommand = ssh -i key\n").unwrap();
    let conditional = machine.project.join("cond.cfg");
    std::fs::write(&conditional, "[hook \"lint\"]\n\tcommand = lint\n\tevent = pre-push\n")
        .unwrap();
    append(
        &repo.join(".git/config"),
        &format!(
            "[include]\n\tpath = {}\n[includeIf \"gitdir:{}/\"]\n\tpath = ../cond.cfg\n",
            include.display(),
            repo.join(".git").display()
        ),
    );

    let facts = machine.check(&[ask(&repo, false)], None).await;

    assert!(facts.checked, "{facts:?}");
    // The include named by the pinned config lies in a `$SCRATCH`: a floor of no plan.
    let target =
        facts.items.iter().find(|fact| fact.item == "include.path" && fact.file == include);
    assert!(target.is_some_and(|fact| fact.writable), "{facts:?}");
    let ssh = item(&facts, "core.sshcommand").unwrap();
    assert_eq!(ssh.file, include);
    assert!(ssh.writable);
    // The conditional include lies in the project; the pinned config names it.
    let hook_key = item(&facts, "hook.lint.command").unwrap_or_else(|| panic!("{facts:?}"));
    assert_eq!(hook_key.file, conditional);
    assert!(!hook_key.writable, "{hook_key:?}");
}

#[tokio::test]
async fn git_code_follows_commondir() {
    let machine = Machine::new();
    let repo = machine.scratch.join("c1/r");
    init(&repo).await;
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    git(&repo, &["add", "a.txt"]).await;
    git(&repo, &["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qm", "a"])
        .await;
    let tree = machine.scratch.join("c1/wt");
    git(&repo, &["worktree", "add", "-q", &tree.display().to_string()]).await;
    let planted = hook(&repo.join(".git/hooks"), "pre-push");

    let facts = machine.check(&[ask(&tree, false)], None).await;

    assert!(facts.checked, "{facts:?}");
    let found = item(&facts, "hooks/pre-push").unwrap();
    assert_eq!(found.file, planted, "the hooks of the common dir run");
    assert!(found.writable);
}

#[tokio::test]
async fn git_code_checks_each_module_on_a_recursing_line() {
    let machine = Machine::new();
    let repo = machine.scratch.join("c1/r");
    init(&repo).await;
    let module = repo.join(".git/modules/a/modules/b");
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(module.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(
        module.join("config"),
        "[core]\n\tbare = false\n\tworktree = ../../../../../a/b\n",
    )
    .unwrap();
    let planted = hook(&module.join("hooks"), "pre-push");

    let facts = machine.check(&[ask(&repo, true)], None).await;
    assert!(facts.checked, "{facts:?}");
    let found = item(&facts, "hooks/pre-push").unwrap();
    assert_eq!(found.file, planted);
    assert_eq!(found.repo, module);
    assert!(found.writable);

    // A line that reaches no submodule does not look at them.
    let facts = machine.check(&[ask(&repo, false)], None).await;
    assert_eq!(item(&facts, "hooks/pre-push"), None);

    // A walk too deep is not checked.
    let mut deep = repo.join(".git/modules");
    for n in 0..10 {
        deep = deep.join(format!("m{n}"));
    }
    std::fs::create_dir_all(&deep).unwrap();
    let facts = machine.check(&[ask(&repo, true)], None).await;
    assert!(!facts.checked, "{facts:?}");
}

#[tokio::test]
async fn git_code_uses_the_hidden_shell_config_env() {
    let machine = Machine::new();
    let repo = machine.project.clone();
    init(&repo).await;
    let global = machine.scratch.join("c1/global.cfg");
    std::fs::create_dir_all(global.parent().unwrap()).unwrap();
    std::fs::write(&global, "[core]\n\tsshCommand = evil\n").unwrap();
    // With efrd's own environment (here: isolated), the file is not read.
    let facts = machine.check(&[ask(&repo, false)], None).await;
    assert_eq!(item(&facts, "core.sshcommand"), None, "{facts:?}");

    let env = BTreeMap::from([
        ("HOME".to_owned(), machine.home.display().to_string()),
        ("GIT_CONFIG_GLOBAL".to_owned(), global.display().to_string()),
        ("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()),
    ]);
    let facts = machine.check(&[ask(&repo, false)], Some(&env)).await;
    let found = item(&facts, "core.sshcommand").unwrap();
    assert_eq!(found.file, global);
    assert!(found.writable);
    // A shell that sets GIT_SSH_COMMAND runs no `ssh` from its PATH for git.
    assert!(facts.items.iter().all(|fact| fact.item != "ssh (PATH)"), "{facts:?}");
}

#[tokio::test]
async fn git_code_drops_command_line_entries() {
    let machine = Machine::new();
    let repo = machine.scratch.join("c1/r");
    init(&repo).await;
    let planted = hook(&repo.join(".git/hooks"), "pre-push");

    let facts = machine.check(&[ask(&repo, false)], None).await;

    // efrd's own `-c core.hooksPath=/dev/null` is no part, and it does not hide the
    // hooks dir that the exit child's git uses.
    assert!(facts.items.iter().all(|fact| !fact.item.eq_ignore_ascii_case("core.hooksPath")));
    assert!(facts.items.iter().all(|fact| !fact.file.starts_with("/dev/null")));
    assert_eq!(item(&facts, "hooks/pre-push").map(|fact| &fact.file), Some(&planted));
    assert!(facts.items.iter().all(|fact| fact.item != "hooks/pre-commit.sample"));
}

#[tokio::test]
async fn git_code_names_the_program_of_a_key_and_of_a_hook() {
    let machine = Machine::new();
    let repo = machine.scratch.join("c1/r");
    init(&repo).await;
    let tool = machine.project.join("bin/tool");
    std::fs::create_dir_all(tool.parent().unwrap()).unwrap();
    std::fs::write(&tool, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(&repo, &["config", "core.sshCommand", &format!("{} -v", tool.display())]).await;
    let pre_push = hook(&repo.join(".git/hooks"), "pre-push");
    std::fs::write(&pre_push, format!("#!{}\n", tool.display())).unwrap();

    let facts = machine.check(&[ask(&repo, false)], None).await;

    let key = facts.items.iter().find(|fact| fact.item == "tool (core.sshcommand)").unwrap();
    assert_eq!(key.file, tool);
    assert!(key.writable, "a program in the project");
    let shebang = facts.items.iter().find(|fact| fact.item == "tool (hooks/pre-push)");
    assert!(shebang.is_some_and(|fact| fact.file == tool), "{facts:?}");
}

#[tokio::test]
async fn git_code_reads_the_programs_of_send_email() {
    let machine = Machine::new();
    let repo = machine.scratch.join("c1/r");
    init(&repo).await;
    let tool = machine.project.join("bin/to-cmd");
    std::fs::create_dir_all(tool.parent().unwrap()).unwrap();
    std::fs::write(&tool, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(&repo, &["config", "sendemail.toCmd", &tool.display().to_string()]).await;
    git(&repo, &["config", "sendemail.smtpServer", &tool.display().to_string()]).await;
    // NOTE: a host name is no program.
    git(&repo, &["config", "sendemail.work.smtpServer", "smtp.example.org"]).await;
    hook(&repo.join(".git/hooks"), "sendemail-validate");

    let facts = machine.check(&[ask(&repo, false)], None).await;

    for key in ["sendemail.tocmd", "sendemail.smtpserver"] {
        assert!(item(&facts, key).is_some_and(|fact| fact.writable), "{key}: {facts:?}");
        let program = item(&facts, &format!("to-cmd ({key})"));
        assert!(program.is_some_and(|fact| fact.file == tool && fact.writable), "{facts:?}");
    }
    assert!(item(&facts, "sendemail.work.smtpserver").is_none(), "{facts:?}");
    assert!(item(&facts, "hooks/sendemail-validate").is_some(), "{facts:?}");
}

#[test]
fn git_code_items_leave_out_the_userinfo_of_a_url() {
    use super::without_userinfo;
    assert_eq!(
        without_userinfo("credential.https://user:token@host.test.helper"),
        "credential.https://host.test.helper"
    );
    assert_eq!(
        without_userinfo("url.https://user:token@host.test/o/.insteadof"),
        "url.https://host.test/o/.insteadof"
    );
    assert_eq!(
        without_userinfo("git-credential-x (credential.https://u@a.test.helper)"),
        "git-credential-x (credential.https://a.test.helper)"
    );
    // NOTE: an `@` after the host is part of the path.
    assert_eq!(
        without_userinfo("url.https://h.test/a@b.insteadof"),
        "url.https://h.test/a@b.insteadof"
    );
    for plain in ["hooks/pre-push", "core.sshcommand", "ssh (PATH)"] {
        assert_eq!(without_userinfo(plain), plain);
    }
}

#[tokio::test]
async fn git_code_reads_a_credential_url_key_without_its_userinfo() {
    let machine = Machine::new();
    let repo = machine.scratch.join("c1/r");
    init(&repo).await;
    git(&repo, &["config", "credential.https://user:token@host.test.helper", "store"]).await;

    let facts = machine.check(&[ask(&repo, false)], None).await;

    assert!(facts.items.iter().all(|fact| !fact.item.contains("token")), "{facts:?}");
    assert!(item(&facts, "credential.https://host.test.helper").is_some(), "{facts:?}");
}

#[tokio::test]
async fn git_code_of_no_repository_is_not_checked() {
    let machine = Machine::new();
    let lone = machine.home.join("lone");
    std::fs::create_dir_all(&lone).unwrap();
    let facts = machine.check(&[ask(&lone, false)], None).await;
    assert!(!facts.checked, "{facts:?}");
}

#[tokio::test]
async fn git_code_slow_git_is_not_checked() {
    let machine = Machine::new();
    let repo = machine.scratch.join("c1/r");
    init(&repo).await;
    // A git that waits on a fifo nobody opens, in place of a slow mount: it never ends
    // on its own, and the test moves the clock only once its timeout sleeps.
    let fifo = machine.home.join("fifo");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    let slow = machine.home.join("slow-git");
    std::fs::write(&slow, format!("#!/bin/sh\nexec cat {}\n", fifo.display())).unwrap();
    std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o755)).unwrap();
    let clock = TestClock::new();
    let shared = clock.shared();
    let git = Git::new(Arc::clone(&shared)).isolated().with_program(&slow);
    let home = Home::new(&machine.home).unwrap();
    let places = machine.places();
    let asks = [ask(&repo, false)];
    let checked = async {
        let input = CodeInput {
            git: &git,
            clock: &shared,
            home: &home,
            env: None,
            shell_path: "/usr/bin:/bin",
            places: &places,
        };
        collect(&asks, &input).await.unwrap()
    };
    let advance = async {
        // NOTE: the timeout of the first git run sleeps on the test clock.
        clock.wait_for_sleeps(1).await;
        clock.advance(super::GIT_TIMEOUT);
    };
    let (facts, ()) = tokio::join!(checked, advance);
    assert!(!facts.checked, "{facts:?}");
    assert_eq!(facts.items, Vec::new());
}

use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use efr_sandbox::CONFIG_LISTINGS_FILE;

use crate::call_dir::PrivateDir;
use crate::guard::{Guard, JobPlaces};
use crate::testing::{TestCall, private_dir, private_file};

use super::*;

/// A contained job whose start the guard recorded, with a counting `git` and
/// `configs` repositories in its guard roots.
struct Job {
    call: TestCall,
    real_git: PathBuf,
    git: PathBuf,
    runs: PathBuf,
    state: PathBuf,
    configs: usize,
}

/// The job, or `None` when this machine has no git.
fn job(configs: usize) -> Option<Job> {
    let real_git =
        ["/usr/bin/git", "/bin/git"].map(PathBuf::from).into_iter().find(|git| git.is_file())?;
    let mut call = TestCall::new();
    let bin = call.root.join("bin");
    private_dir(&bin);
    // The guard takes the first git on the shell's PATH: the counting one.
    call.spec.shell_path = bin.display().to_string();
    call.spec.guard_roots = vec![call.project().to_path_buf()];
    let job = Job {
        real_git,
        git: bin.join("git"),
        runs: bin.join("runs"),
        state: call.spec.runtime.job_state_dir(call.spec.call),
        configs,
        call,
    };
    job.write_git("");
    for n in 0..configs {
        let repo = job.repo(n);
        fs::create_dir_all(&repo).unwrap();
        let status = crate::os::command(&job.real_git)
            .args(["init", "-q"])
            .current_dir(&repo)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap();
        assert!(status.success());
        job.append(n, &format!("[user]\n\tname = repo {n}\n"));
    }
    private_dir(&job.state);
    let plan = job.call.plan();
    let shell_dir = PrivateDir::open(&job.call.spec.runtime.shell_dir).unwrap();
    let start = job.call.project().to_path_buf();
    let guard = Guard::before(&job.call.spec, &plan, &start, &shell_dir);
    let places = JobPlaces { start, quarantine: job.call.root.join("quarantine") };
    guard.save(&PrivateDir::open(&job.state).unwrap(), &places).unwrap();
    assert_eq!(job.take_runs(), configs, "the job's start lists each config once");
    Some(job)
}

impl Job {
    fn repo(&self, n: usize) -> PathBuf {
        self.call.project().join(format!("repo-{n}"))
    }

    /// The counting `git`; `extra` changes its file, and so its identity.
    fn write_git(&self, extra: &str) {
        let script = format!(
            "#!/bin/sh\n{extra}printf 'x\\n' >> '{}'\nexec '{}' \"$@\"\n",
            self.runs.display(),
            self.real_git.display()
        );
        fs::write(&self.git, script).unwrap();
        fs::set_permissions(&self.git, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn append(&self, n: usize, text: &str) {
        let config = self.repo(n).join(".git/config");
        let mut content = fs::read_to_string(&config).unwrap();
        content.push_str(text);
        fs::write(&config, content).unwrap();
    }

    /// The runs of the counting git since the last call.
    fn take_runs(&self) -> usize {
        let count = fs::read(&self.runs).map(|bytes| bytes.iter().filter(|b| **b == b'\n').count());
        let _ = fs::remove_file(&self.runs);
        count.unwrap_or(0)
    }

    /// One `guard-check`, as after a reboot: the call dir is gone, the record is there.
    fn check(&self) -> GuardReport {
        check(&self.call.root.join("r/gone"), &self.state).unwrap()
    }

    fn listings(&self) -> PathBuf {
        self.state.join(CONFIG_LISTINGS_FILE)
    }
}

fn inode(path: &Path) -> u64 {
    fs::metadata(path).unwrap().ino()
}

#[test]
fn a_check_runs_no_git_for_configs_that_did_not_change() {
    let Some(job) = job(3) else { return };
    let written = inode(&job.listings());
    for _ in 0..3 {
        assert_eq!(job.check().changes, []);
        assert_eq!(job.take_runs(), 0);
    }
    // Listings that did not change are not written again.
    assert_eq!(inode(&job.listings()), written);
}

#[test]
fn a_check_runs_git_once_for_a_changed_config() {
    let Some(job) = job(3) else { return };
    job.append(1, "[user]\n\temail = repo@example.invalid\n");
    assert_eq!(job.check().changes, []);
    assert_eq!(job.take_runs(), 1);
    // The check kept the new listing for the next one.
    assert_eq!(job.check().changes, []);
    assert_eq!(job.take_runs(), 0);
}

#[test]
fn a_planted_code_key_is_found_with_the_listings_kept() {
    let Some(job) = job(2) else { return };
    job.append(0, "[core]\n\tfsmonitor = ./x\n");
    let report = job.check();
    assert_eq!(job.take_runs(), 1);
    assert_eq!(report.changes.len(), 1, "{report:?}");
    assert_eq!(report.changes[0].key.as_deref(), Some("core.fsmonitor"));
    assert_eq!(report.changes[0].path, job.repo(0).join(".git/config"));
    assert!(report.quarantined);
}

#[test]
fn a_check_with_another_git_lists_every_config_again() {
    let Some(job) = job(3) else { return };
    // The same path after an update: its size and times changed.
    job.write_git(": updated\n");
    assert_eq!(job.check().changes, []);
    assert_eq!(job.take_runs(), job.configs);
    assert_eq!(job.check().changes, []);
    assert_eq!(job.take_runs(), 0);
}

#[test]
fn a_bad_listings_file_is_an_empty_cache_and_the_check_still_passes() {
    let Some(job) = job(2) else { return };
    let listings = job.listings();
    let other_git = br#"{"git":"/elsewhere/git 1:2 3 4.5 6.7","configs":[]}"#;
    let bad: [&dyn Fn(); 5] = [
        &|| fs::remove_file(&listings).unwrap(),
        &|| private_file(&listings, b"{not json"),
        &|| private_file(&listings, other_git),
        &|| private_file(&listings, &vec![b' '; efr_sandbox::MAX_CONFIG_LISTINGS_BYTES + 1]),
        &|| {
            fs::remove_file(&listings).unwrap();
            std::os::unix::fs::symlink("/nonexistent", &listings).unwrap();
        },
    ];
    for (n, make_bad) in bad.iter().enumerate() {
        make_bad();
        assert_eq!(job.check().changes, [], "case {n}");
        assert_eq!(job.take_runs(), job.configs, "case {n}");
        // The check wrote good listings back, so the next one runs no git.
        assert_eq!(job.check().changes, [], "case {n}");
        assert_eq!(job.take_runs(), 0, "case {n}");
    }
}

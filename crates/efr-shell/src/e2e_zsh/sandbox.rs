//! The auto mode's zsh side in a real zsh, with a fake launcher in place of `efr-sbx`
//! (efr's auto spec, section 16.1): the wrapper line, its check, the apply file, the
//! snapshot, the hooks and the child script. The fake records its arguments and runs
//! `zsh -f efr-child.zsh` directly, as the fake bwrap does, so these tests need no
//! kernel sandbox and run wherever zsh does, `just test-shell-ubuntu` included.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use efr_protocol::CallId;
use efr_sandbox::{SpecLaunch, nonce_hex};

use super::Zsh;
use crate::{CommandResult, NoProgress, RunRequest, SandboxRun};

/// What the fake launcher does, from files that a test puts in the call dir:
/// `fake-no-start` (it fails before it starts), `fake-apply` (it copies that file to
/// `apply`), `fake-cwd` (the cwd of `result.json`) and `fake-no-result` (it dies before
/// its result).
const FAKE_LAUNCHER: &str = r#"#!/bin/sh
# The fake efr-sbx of efr-shell's zsh tests. It records its arguments, runs the child
# script directly as the fake bwrap does, and writes the launcher's files.
dir=$3
printf '%s\n' "$@" > "$dir/fake-args"
if [ -e "$dir/fake-no-start" ]; then exit 125; fi
: > "$dir/started"
# The child's dir holds what the real launcher binds for it: the snapshot and the
# line; the state of earlier calls is not under test here.
mkdir -p "$dir/fake-child"
cp "${dir%/*}/snapshot.zsh" "$dir/line" "$dir/fake-child/"
zsh -f "@CHILD@" "$dir/fake-child" 3> "$dir/records"
status=$?
if [ -e "$dir/fake-apply" ]; then cp "$dir/fake-apply" "$dir/apply"; fi
if [ -e "$dir/fake-no-result" ]; then exit "$status"; fi
cwd=$(pwd)
if [ -e "$dir/fake-cwd" ]; then cwd=$(cat "$dir/fake-cwd"); fi
printf '{"started":true,"exit_code":%s,"cwd":"%s","state_kept":true}' "$status" "$cwd" > "$dir/result.tmp"
mv "$dir/result.tmp" "$dir/result.json"
exit "$status"
"#;

/// The nonce of every call in these tests.
pub(crate) const NONCE: [u8; 16] = [0x5a; 16];

impl Zsh {
    /// A zsh whose sandbox dir is `<root>/sbx` and whose launcher is the fake, or
    /// `None` when `EFR_TEST_ZSH` is off.
    pub(crate) fn start_sandboxed(test: &str) -> Option<Self> {
        Self::start_at(test, Some(&tree_parent()), |config, root| {
            let launcher = root.join("bin/efr-sbx");
            std::fs::create_dir_all(root.join("bin")).unwrap();
            let child = config.integration_dir.join(crate::integration::CHILD_FILE);
            let script = FAKE_LAUNCHER.replace("@CHILD@", &child.to_string_lossy());
            std::fs::write(&launcher, script).unwrap();
            std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
            config.sandbox_dir = Some(root.join("sbx"));
            config.sandbox_launcher = Some(launcher);
        })
    }

    /// The conversation's sandbox dir.
    pub(crate) fn sandbox_dir(&self) -> PathBuf {
        self.root.path().join("sbx").join(self.conversation.to_string())
    }

    /// Call `n`'s dir as efrd prepares it: mode 0700, with the nonce.
    pub(crate) fn prepare(&self, n: u16) -> SandboxRun {
        let call: CallId = format!("01920000-0000-7000-8000-0000000ca{n:03}").parse().unwrap();
        let dir = self.sandbox_dir().join(call.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(dir.join("nonce"), nonce_hex(&NONCE)).unwrap();
        SandboxRun::new(dir, call, NONCE, SpecLaunch::Contained)
    }

    /// Runs `command` as the sandboxed call `run`.
    pub(crate) async fn run_sandboxed(&self, run: &SandboxRun, command: &str) -> CommandResult {
        let request = RunRequest::new(command, self.start_dir())
            .with_call(run.call)
            .with_sandbox(Some(run.clone()))
            .with_timeout(Duration::from_secs(600));
        self.sessions.run_command(self.conversation, request, &mut NoProgress).await.unwrap()
    }

    /// Runs `command` in the shell itself, as another mode does.
    pub(crate) async fn run_plain(&self, command: &str) -> CommandResult {
        let request = RunRequest::new(command, self.start_dir());
        self.sessions.run_command(self.conversation, request, &mut NoProgress).await.unwrap()
    }
}

/// Where the throwaway trees of the sandbox tests go: below the build's target dir,
/// not below `/tmp`. The wrapper keeps the shell out of `/tmp` and `/var/tmp`, which a
/// contained call sees as its private tmp.
pub(crate) fn tree_parent() -> PathBuf {
    let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/efr-shell");
    std::fs::create_dir_all(&parent).unwrap();
    // The shell reports the resolved path of its directory.
    std::fs::canonicalize(parent).unwrap()
}

/// The text of `dir/name`, or `None` when there is no such file.
pub(crate) fn read(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(name)).ok()
}

#[cfg(test)]
mod tests;

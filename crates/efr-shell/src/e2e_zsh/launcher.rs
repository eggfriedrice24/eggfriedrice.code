//! The behaviour tests of efr's auto spec, section 16.2, that need the real launcher:
//! a real zsh, a call dir with a `spec.json` as efrd writes it, and the `efr-sbx` that
//! `EFR_TEST_SBX_BIN` names (`just test-sandbox` builds it and sets the variable).
//!
//! A test skips, and says why, when `EFR_TEST_ZSH` is off, when `EFR_TEST_SBX_BIN` is
//! not set, or when the launcher's probe says that this machine cannot run the
//! sandbox. `just test-sandbox` fails when they skip on a machine whose probe says
//! ready.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use efr_protocol::{CacheMode, CallId};
use efr_sandbox::{
    EnvPlan, NetworkPlan, ProbeReport, RecordLimits, RuntimePaths, SPEC_VERSION, SandboxSpec,
    SpecLaunch, WriteRoot, WriteRootKind, nonce_hex,
};
use efr_stdx::env::Var;

use super::Zsh;
use super::sandbox::{NONCE, tree_parent};
use crate::SandboxRun;

/// A zsh whose sandbox runs the real launcher.
pub(crate) struct Launcher {
    pub(crate) zsh: Zsh,
    /// The built `efr-sbx`.
    pub(crate) bin: PathBuf,
    /// The project of the calls: the one write root besides scratch.
    pub(crate) project: PathBuf,
}

impl Launcher {
    /// The harness, or `None` with a message when it cannot run here.
    pub(crate) async fn start(test: &str) -> Option<Self> {
        let bin = launcher(test)?;
        // NOTE: the tree lies outside the host's /tmp, which the sandbox replaces with
        // a private one.
        let zsh = Zsh::start_at(test, Some(&tree_parent()), |config, root| {
            config.sandbox_dir = Some(root.join("r/sbx"));
            config.sandbox_launcher = Some(bin.clone());
        })?;
        let probe_dir = zsh.dir("probe");
        std::fs::set_permissions(&probe_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        if !probe_ready(test, &bin, &probe_dir).await {
            return None;
        }
        let project = zsh.dir("project");
        Some(Launcher { zsh, bin, project })
    }

    /// Call `n`'s dir with its `spec.json` and nonce, as efrd prepares it for a
    /// contained call (or the exit child with `launch`).
    pub(crate) fn prepare(&self, n: u16, launch: SpecLaunch) -> SandboxRun {
        let root = self.zsh.root.path();
        let call: CallId = format!("01920000-0000-7000-8000-0000000cb{n:03}").parse().unwrap();
        let shell_dir = self.zsh.sandbox_dir_in("r/sbx");
        let dir = shell_dir.join(call.to_string());
        for made in [&shell_dir, &dir] {
            std::fs::create_dir_all(made).unwrap();
            std::fs::set_permissions(made, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::fs::write(dir.join("nonce"), nonce_hex(&NONCE)).unwrap();
        let scratch = self.zsh.dir("d/scratch/conversation");
        let sandbox_dir = self.zsh.dir("s/sandbox/conversation");
        let runtime = RuntimePaths {
            home: self.zsh.home(),
            user_runtime: self.zsh.dir("xrt"),
            runtime: root.join("r"),
            data: self.zsh.dir("d"),
            state: self.zsh.dir("s"),
            config: self.zsh.dir("c"),
            sandbox_dir,
            shell_dir,
            call_dir: dir.clone(),
            scratch,
            launcher: self.bin.clone(),
            child_script: root.join("zsh").join(crate::integration::CHILD_FILE),
            editor: root.join("zsh").join(crate::integration::EDITOR_FILE),
            zsh: which::which_in("zsh", Some("/usr/bin:/bin"), "/").unwrap(),
            bwrap: PathBuf::from("/usr/bin/bwrap"),
            session_bus: None,
            system_bus: PathBuf::from("/run/dbus/system_bus_socket"),
        };
        let spec = SandboxSpec {
            version: SPEC_VERSION,
            conversation: self.zsh.conversation,
            call,
            launch,
            write_roots: vec![WriteRoot {
                path: self.project.clone(),
                kind: WriteRootKind::TurnProject,
            }],
            caches: Vec::new(),
            cache_mode: CacheMode::default(),
            masks: Vec::new(),
            floors: Vec::new(),
            git_dirs: Vec::new(),
            guard_roots: vec![self.project.clone()],
            protected_names: Vec::new(),
            grants: Vec::new(),
            network: NetworkPlan::None,
            // The daemon passes sandbox.promote_env; these tests need one name of it.
            env: EnvPlan { promote: vec!["RUST_LOG".to_owned()], ..EnvPlan::default() },
            shell_path: "/usr/bin:/bin".to_owned(),
            limits: RecordLimits::default(),
            runtime,
        };
        std::fs::write(dir.join("spec.json"), spec.to_json().unwrap()).unwrap();
        // The launcher refuses a call file that another user could read or write.
        for file in ["nonce", "spec.json"] {
            std::fs::set_permissions(dir.join(file), std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        SandboxRun::new(dir, call, NONCE, launch)
    }
}

impl Zsh {
    /// The conversation's sandbox dir below `<root>/<root_dir>`.
    pub(crate) fn sandbox_dir_in(&self, root_dir: &str) -> PathBuf {
        self.root.path().join(root_dir).join(self.conversation.to_string())
    }
}

/// The launcher from `EFR_TEST_SBX_BIN`, or `None` with a message.
#[expect(clippy::print_stderr, reason = "a skipped test says why")]
fn launcher(test: &str) -> Option<PathBuf> {
    match efr_stdx::env::path(Var::TestSbxBin) {
        Ok(Some(bin)) => Some(bin),
        Ok(None) => {
            eprintln!(
                "skipped: {test}: set EFR_TEST_SBX_BIN to a built efr-sbx (just test-sandbox)"
            );
            None
        }
        Err(error) => {
            eprintln!("skipped: {test}: EFR_TEST_SBX_BIN: {error}");
            None
        }
    }
}

/// True when `efr-sbx probe --json` says that this machine runs the sandbox.
#[expect(clippy::print_stderr, reason = "a skipped test says why")]
async fn probe_ready(test: &str, bin: &Path, dir: &Path) -> bool {
    // NOTE: the probe's fake call needs a private dir of its own, never below /tmp,
    // which the sandbox replaces with its private tmp.
    let output = efr_stdx::process::command(bin, Path::new("/"))
        .args(["probe", "--json", "--dir"])
        .arg(dir)
        .output()
        .await;
    let report = output.ok().and_then(|output| ProbeReport::from_json(&output.stdout).ok());
    match report.map(|report| report.status()) {
        Some(status) if status.available => true,
        Some(status) => {
            let reason = status.reason.unwrap_or_default();
            eprintln!(
                "skipped: {test}: the launcher's probe says the sandbox cannot run: {reason}"
            );
            false
        }
        None => {
            eprintln!("skipped: {test}: the launcher's probe gave no report");
            false
        }
    }
}

#[cfg(test)]
mod tests;

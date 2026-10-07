//! A spec and a call dir in a temp dir, for the unit tests.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use efr_protocol::{CacheMode, CallId, ConversationId};
use efr_sandbox::{
    EnvPlan, LINE_FILE, NetworkPlan, RecordLimits, RuntimePaths, SPEC_FILE, SPEC_VERSION,
    SandboxSpec, SpecLaunch, WriteRoot, WriteRootKind,
};

pub(crate) const CONVERSATION: &str = "0192f0c1-0000-7000-8000-000000000001";
pub(crate) const CALL: &str = "0192f0c1-0000-7000-8000-000000000002";

/// A temp dir with `home/p/proj`, efr's roots, a call dir with `spec.json` and `line`.
pub(crate) struct TestCall {
    pub(crate) _temp: tempfile::TempDir,
    pub(crate) root: PathBuf,
    pub(crate) spec: SandboxSpec,
}

/// A temp dir next to the test binary, in the target dir: the sandbox treats paths
/// below /tmp as its private tmp, so a test call must not live there.
pub(crate) fn temp_dir() -> tempfile::TempDir {
    let exe = std::env::current_exe().unwrap();
    let base = exe.parent().and_then(Path::parent).unwrap().join("unit-tmp");
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new().prefix("efr-sbx").tempdir_in(base).unwrap()
}

pub(crate) fn private_dir(path: &Path) {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(path).unwrap();
}

pub(crate) fn private_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

impl TestCall {
    pub(crate) fn new() -> TestCall {
        let temp = temp_dir();
        let root = temp.path().canonicalize().unwrap();
        let conversation: ConversationId = CONVERSATION.parse().unwrap();
        let call: CallId = CALL.parse().unwrap();
        let home = root.join("home");
        let project = home.join("p/proj");
        let runtime = root.join("r");
        let shell_dir = runtime.join("sbx").join(CONVERSATION);
        let call_dir = shell_dir.join(CALL);
        for dir in [&project, &call_dir, &root.join("s/sandbox").join(CONVERSATION).join("tmp")] {
            private_dir(dir);
        }
        let spec = SandboxSpec {
            version: SPEC_VERSION,
            conversation,
            call,
            launch: SpecLaunch::Contained,
            write_roots: vec![WriteRoot { path: project, kind: WriteRootKind::TurnProject }],
            caches: Vec::new(),
            cache_mode: CacheMode::Overlay,
            masks: Vec::new(),
            floors: Vec::new(),
            git_dirs: Vec::new(),
            guard_roots: Vec::new(),
            protected_names: Vec::new(),
            grants: Vec::new(),
            network: NetworkPlan::None,
            env: EnvPlan::default(),
            shell_path: "/usr/bin:/bin".to_owned(),
            limits: RecordLimits::default(),
            runtime: RuntimePaths {
                home,
                user_runtime: root.join("xrt"),
                runtime: runtime.clone(),
                data: root.join("d"),
                state: root.join("s"),
                config: root.join("c"),
                sandbox_dir: root.join("s/sandbox").join(CONVERSATION),
                shell_dir,
                call_dir: call_dir.clone(),
                scratch: root.join("d/scratch").join(CONVERSATION),
                launcher: runtime.join("bin/efr-sbx"),
                child_script: runtime.join("zsh/efr-child.zsh"),
                editor: runtime.join("zsh/efr-editor"),
                zsh: PathBuf::from("/usr/bin/zsh"),
                bwrap: PathBuf::from("/usr/bin/bwrap"),
                session_bus: None,
                system_bus: PathBuf::from("/run/dbus/system_bus_socket"),
            },
        };
        private_dir(&runtime.join("bin"));
        private_dir(&runtime.join("zsh"));
        private_file(&spec.runtime.launcher, b"");
        private_file(&spec.runtime.child_script, b"exit 0\n");
        private_file(&call_dir.join(SPEC_FILE), &spec.to_json().unwrap());
        private_file(&call_dir.join(LINE_FILE), b"true\n");
        TestCall { _temp: temp, root, spec }
    }

    pub(crate) fn plan(&self) -> efr_sandbox::MountPlan {
        efr_sandbox::MountPlan::build(&self.spec, &crate::real_fs::RealFs).unwrap()
    }

    pub(crate) fn project(&self) -> &Path {
        &self.spec.write_roots[0].path
    }

    pub(crate) fn call_dir(&self) -> &Path {
        &self.spec.runtime.call_dir
    }
}

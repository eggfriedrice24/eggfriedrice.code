//! A file system in memory for the tests: directories, files, links and sockets.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use crate::fs_view::{FileKind, FsView, resolve};

#[derive(Debug, Clone)]
pub(crate) enum Node {
    Dir,
    File(Vec<u8>),
    Link(PathBuf),
    Socket,
}

/// Paths are absolute; parents of every entry exist as directories.
#[derive(Debug, Default, Clone)]
pub(crate) struct FakeFs {
    nodes: BTreeMap<PathBuf, Node>,
}

impl FakeFs {
    pub(crate) fn new() -> Self {
        let mut fs = FakeFs::default();
        fs.nodes.insert(PathBuf::from("/"), Node::Dir);
        fs
    }

    fn parents(&mut self, path: &Path) {
        for ancestor in path.ancestors().skip(1) {
            self.nodes.entry(ancestor.to_path_buf()).or_insert(Node::Dir);
        }
    }

    pub(crate) fn dir(&mut self, path: &str) -> &mut Self {
        let path = PathBuf::from(path);
        self.parents(&path);
        self.nodes.insert(path, Node::Dir);
        self
    }

    pub(crate) fn file(&mut self, path: &str, content: &str) -> &mut Self {
        let path = PathBuf::from(path);
        self.parents(&path);
        self.nodes.insert(path, Node::File(content.as_bytes().to_vec()));
        self
    }

    pub(crate) fn link(&mut self, path: &str, target: &str) -> &mut Self {
        let path = PathBuf::from(path);
        self.parents(&path);
        self.nodes.insert(path, Node::Link(PathBuf::from(target)));
        self
    }

    pub(crate) fn socket(&mut self, path: &str) -> &mut Self {
        let path = PathBuf::from(path);
        self.parents(&path);
        self.nodes.insert(path, Node::Socket);
        self
    }

    pub(crate) fn remove(&mut self, path: &str) -> &mut Self {
        let path = PathBuf::from(path);
        self.nodes.retain(|known, _| !known.starts_with(&path));
        self
    }

    fn node(&self, path: &Path) -> io::Result<&Node> {
        self.nodes.get(path).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
}

impl FsView for FakeFs {
    fn lstat(&self, path: &Path) -> io::Result<FileKind> {
        Ok(match self.node(path)? {
            Node::Dir => FileKind::Dir,
            Node::File(_) => FileKind::File,
            Node::Link(_) => FileKind::Symlink,
            Node::Socket => FileKind::Other,
        })
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        match self.node(path)? {
            Node::Link(target) => Ok(target.clone()),
            _ => Err(io::Error::from(io::ErrorKind::InvalidInput)),
        }
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<OsString>> {
        let resolved = resolve(self, path).map_err(|_| io::Error::from(io::ErrorKind::NotFound))?;
        match self.node(&resolved.path)? {
            Node::Dir => Ok(self
                .nodes
                .keys()
                .filter(|known| known.parent() == Some(resolved.path.as_path()))
                .filter_map(|known| known.file_name().map(|name| name.to_os_string()))
                .collect()),
            _ => Err(io::Error::from(io::ErrorKind::NotADirectory)),
        }
    }

    fn read_file(&self, path: &Path, limit: usize) -> io::Result<Vec<u8>> {
        let resolved = resolve(self, path).map_err(|_| io::Error::from(io::ErrorKind::NotFound))?;
        match self.node(&resolved.path)? {
            Node::File(content) if content.len() > limit => {
                Err(io::Error::from(io::ErrorKind::FileTooLarge))
            }
            Node::File(content) => Ok(content.clone()),
            _ => Err(io::Error::from(io::ErrorKind::InvalidInput)),
        }
    }

    fn open_no_symlinks(&self, path: &Path) -> io::Result<OwnedFd> {
        for ancestor in path.ancestors() {
            if matches!(self.node(ancestor), Ok(Node::Link(_))) {
                return Err(io::Error::other("ELOOP"));
            }
        }
        self.node(path)?;
        Ok(OwnedFd::from(std::fs::File::open("/dev/null")?))
    }

    fn open_empty(&self) -> io::Result<OwnedFd> {
        Ok(OwnedFd::from(std::fs::File::open("/dev/null")?))
    }
}

pub(crate) const HOME: &str = "/home/u";
pub(crate) const PROJECT: &str = "/home/u/p/app";
pub(crate) const CONVERSATION: &str = "019a9b1c-3d00-7a10-8b20-000000000001";
pub(crate) const CALL: &str = "019a9b1c-3d00-7a10-8b20-000000000004";

pub(crate) fn runtime() -> crate::RuntimePaths {
    let sbx = format!("/home/u/.local/state/efr/sandbox/{CONVERSATION}");
    let shell = format!("/run/user/1000/efr/sbx/{CONVERSATION}");
    crate::RuntimePaths {
        home: HOME.into(),
        user_runtime: "/run/user/1000".into(),
        runtime: "/run/user/1000/efr".into(),
        data: "/home/u/.local/share/efr".into(),
        state: "/home/u/.local/state/efr".into(),
        config: "/home/u/.config/efr".into(),
        sandbox_dir: sbx.into(),
        call_dir: format!("{shell}/{CALL}").into(),
        shell_dir: shell.into(),
        scratch: format!("/home/u/.local/share/efr/scratch/{CONVERSATION}").into(),
        launcher: "/run/user/1000/efr/bin/efr-sbx".into(),
        child_script: "/run/user/1000/efr/zsh/efr-child.zsh".into(),
        editor: "/run/user/1000/efr/zsh/efr-editor".into(),
        zsh: "/usr/bin/zsh".into(),
        bwrap: "/usr/bin/bwrap".into(),
        session_bus: Some("/run/user/1000/bus".into()),
        system_bus: "/run/dbus/system_bus_socket".into(),
    }
}

/// A turn in `~/p/app` with two engine secrets, the config root and `.zshrc` as
/// floors, the cargo cache, and `PATH=/usr/bin`.
pub(crate) fn spec() -> crate::SandboxSpec {
    use crate::{
        CacheOverlay, EnvPlan, Floor, FloorKind, Mask, MaskKind, NetworkPlan, RecordLimits,
        SPEC_VERSION, SandboxSpec, SpecLaunch, WriteRoot, WriteRootKind,
    };
    let runtime = runtime();
    let cargo = CacheOverlay::new(Path::new("/home/u/.cargo"), &runtime.sandbox_dir, &runtime.home);
    SandboxSpec {
        version: SPEC_VERSION,
        conversation: CONVERSATION.parse().unwrap(),
        call: CALL.parse().unwrap(),
        launch: SpecLaunch::Contained,
        write_roots: vec![WriteRoot { path: PROJECT.into(), kind: WriteRootKind::TurnProject }],
        caches: vec![cargo],
        cache_mode: efr_protocol::CacheMode::Overlay,
        masks: vec![
            Mask { path: "/home/u/.ssh".into(), kind: MaskKind::EngineSecret },
            Mask { path: "/home/u/.npmrc".into(), kind: MaskKind::EngineSecret },
        ],
        floors: vec![
            Floor { path: "/home/u/.config/efr".into(), kind: FloorKind::Config },
            Floor { path: "/home/u/.zshrc".into(), kind: FloorKind::ShellStartup },
        ],
        git_dirs: Vec::new(),
        guard_roots: vec![PROJECT.into()],
        protected_names: vec![".envrc".to_owned(), ".claude/".to_owned()],
        grants: Vec::new(),
        network: NetworkPlan::None,
        env: EnvPlan {
            deny: Vec::new(),
            keep: Vec::new(),
            promote: vec!["RUST_LOG".to_owned(), "LC_*".to_owned(), "PATH".to_owned()],
            export_deny: Vec::new(),
            offline_hints: true,
        },
        shell_path: "/usr/bin".to_owned(),
        limits: RecordLimits::default(),
        runtime,
    }
}

/// The machine of [`spec`]: the project with a git dir, the secrets, the cache, efr's
/// roots and the launcher's files.
pub(crate) fn world() -> FakeFs {
    let runtime = runtime();
    let mut fs = FakeFs::new();
    fs.dir(PROJECT)
        .dir("/home/u/p/app/src")
        .dir("/home/u/p/app/.git/hooks")
        .file("/home/u/p/app/.git/config", "[core]\n\tbare = false\n")
        .dir("/home/u/.ssh")
        .file("/home/u/.ssh/id_ed25519", "key")
        .file("/home/u/.zshrc", "x")
        .file("/home/u/.config/efr/config.toml", "")
        .dir("/home/u/.cargo/bin")
        .file("/home/u/.cargo/config.toml", "")
        .dir("/home/u/.local/share/efr/scratch")
        .dir(&runtime.scratch.to_string_lossy())
        .dir(&runtime.private_tmp().to_string_lossy())
        .file(&runtime.launcher.to_string_lossy(), "elf")
        .file(&runtime.child_script.to_string_lossy(), "zsh")
        .file(&runtime.editor.to_string_lossy(), "zsh")
        .file(&runtime.shell_dir.join("snapshot.zsh").to_string_lossy(), "")
        .file(&runtime.call_dir.join("line").to_string_lossy(), "ls")
        .dir("/usr/bin")
        .file("/usr/bin/zsh", "elf")
        .socket("/run/user/1000/bus")
        .socket("/run/dbus/system_bus_socket")
        .file("/dev/nvme0n1", "")
        .dir("/tmp")
        .dir("/var/tmp");
    fs
}

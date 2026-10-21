//! The probe's fake call: a project, a secret, a cache, a socket and a terminal outside,
//! and the spec of one contained call against them.

use std::ffi::OsString;
use std::fs;
use std::os::fd::OwnedFd;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{SocketAddr, UnixDatagram, UnixListener};
use std::path::{Path, PathBuf};

use efr_protocol::{CacheMode, CallId, ConversationId, Grant};
use efr_sandbox::{
    CacheOverlay, EnvPlan, LINE_FILE, Mask, MaskKind, NetworkPlan, RecordLimits, RuntimePaths,
    SPEC_VERSION, SandboxSpec, SpecLaunch, WriteRoot, WriteRootKind, is_within, normalize,
};

use crate::error::SbxError;
use crate::probe::ProbeArgs;

/// The fake call's ids; every probe uses its own directory, so they never collide.
const CONVERSATION: &str = "00000000-0000-4000-8000-0000000000c0";
const CALL: &str = "00000000-0000-4000-8000-0000000000ca";

/// The programs the probe found.
#[derive(Debug, Clone)]
pub(crate) struct Programs {
    pub(crate) bwrap: PathBuf,
    pub(crate) zsh: PathBuf,
    pub(crate) launcher: PathBuf,
    pub(crate) shell_path: String,
}

/// Everything outside that the self-test pokes at.
#[derive(Debug)]
pub(crate) struct Fixture {
    root: PathBuf,
    pub(crate) project: PathBuf,
    outside: PathBuf,
    secret: PathBuf,
    cache: PathBuf,
    socket: PathBuf,
    abstract_name: String,
    abstract_dgram_name: String,
    pts: Option<PathBuf>,
    landlock_only: PathBuf,
    socket_bound: bool,
    unavailable: Vec<(&'static str, String)>,
    spec: SandboxSpec,
    // Held open for the life of the fixture: the self-test must see them exist.
    _listeners: Vec<UnixListener>,
    _datagram: UnixDatagram,
    _pty: Option<OwnedFd>,
}

fn private_dir(path: &Path) -> Result<(), SbxError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|error| SbxError::io("create", path, error))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), SbxError> {
    fs::write(path, bytes).map_err(|error| SbxError::io("write", path, error))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| SbxError::io("protect", path, error))
}

fn absolute(path: PathBuf) -> Result<PathBuf, SbxError> {
    let real = path.canonicalize().map_err(|error| SbxError::io("resolve", &path, error))?;
    normalize(&real)
        .ok_or_else(|| SbxError::io("resolve", path, std::io::ErrorKind::InvalidInput.into()))
}

impl Fixture {
    /// Makes the fake call in a new directory below `args.dir`.
    pub(crate) fn make(args: &ProbeArgs, programs: &Programs) -> Result<Fixture, SbxError> {
        private_dir(&args.dir)?;
        let stamp = crate::os::since_epoch().as_nanos();
        // Short names: the socket's path must fit in a sockaddr_un.
        let root = absolute(args.dir.clone())?.join(format!("p{}", std::process::id()));
        if root.exists() {
            open_up(&root);
            let _ = fs::remove_dir_all(&root);
        }
        if is_within(&root, Path::new("/tmp")) || is_within(&root, Path::new("/var/tmp")) {
            // The private tmp replaces the host's /tmp inside, so a fake call there
            // would see its own outside paths as writable.
            return Err(SbxError::io("use", root, std::io::ErrorKind::InvalidInput.into()));
        }
        let conversation: ConversationId = CONVERSATION.parse().map_err(|_| {
            SbxError::os("make the probe ids", std::io::ErrorKind::InvalidData.into())
        })?;
        let call: CallId = CALL.parse().map_err(|_| {
            SbxError::os("make the probe ids", std::io::ErrorKind::InvalidData.into())
        })?;
        let home =
            args.home.clone().or_else(|| crate::os::var("HOME").map(PathBuf::from)).ok_or_else(
                || SbxError::os("find the home directory", std::io::ErrorKind::NotFound.into()),
            )?;
        let home = absolute(home)?;
        let user_runtime = args
            .user_runtime
            .clone()
            .or_else(|| crate::os::var("XDG_RUNTIME_DIR").map(PathBuf::from))
            .unwrap_or_else(|| {
                PathBuf::from(format!("/run/user/{}", rustix::process::getuid().as_raw()))
            });
        // A session without a runtime dir (a container, a cron job) still gets a probe:
        // the fixture's own dir stands in for it, masked the same way.
        let user_runtime = if args.user_runtime.is_none() && !user_runtime.is_dir() {
            let own = root.join("x");
            private_dir(&own)?;
            own
        } else {
            absolute(user_runtime)?
        };
        let runtime_root = root.join("r");
        let shell_dir = runtime_root.join("sbx").join(conversation.to_string());
        let call_dir = shell_dir.join(call.to_string());
        let data = root.join("d");
        let scratch = data.join("scratch").join(conversation.to_string());
        let state = root.join("s");
        let sandbox_dir = state.join("sandbox").join(conversation.to_string());
        let project = root.join("project");
        let secret = root.join("secret");
        let cache = root.join("cache");
        let sockets = root.join("k");
        for dir in
            [&call_dir, &scratch, &sandbox_dir.join("tmp"), &project, &secret, &cache, &sockets]
        {
            private_dir(dir)?;
        }
        private_dir(&root.join("c"))?;
        let zsh_dir = runtime_root.join("zsh");
        private_dir(&zsh_dir)?;
        let child_script = zsh_dir.join("efr-child.zsh");
        write(&child_script, b"exit 0\n")?;
        write(&call_dir.join(LINE_FILE), b"true\n")?;
        write(&secret.join("key"), b"probe secret\n")?;
        write(&cache.join("entry"), b"cached\n")?;
        let cache_overlay = CacheOverlay::new(&cache, &sandbox_dir, &home);
        private_dir(&cache_overlay.upper)?;
        private_dir(&cache_overlay.work)?;
        let mut unavailable = Vec::new();
        let socket = sockets.join("s");
        // A socket that cannot be made leaves out the socket check, and the probe fails
        // with this reason.
        let listener = crate::self_test::socket_path(&socket)
            .and_then(|(_dir, path)| UnixListener::bind(path))
            .map_err(|error| {
                let reason = format!("no Unix socket at {}: {error}", socket.display());
                unavailable.push(("unix_socket", reason));
            })
            .ok();
        let abstract_name = format!("efr-sbx-probe-{}-{stamp}", std::process::id());
        let abstract_listener = SocketAddr::from_abstract_name(abstract_name.as_bytes())
            .and_then(|address| UnixListener::bind_addr(&address))
            .map_err(|error| SbxError::os("bind an abstract socket", error))?;
        // A live datagram name of the host: a network view call shares the host's
        // abstract names, and its send there must fail with EPERM from the Landlock
        // scope.
        let abstract_dgram_name = format!("efr-sbx-probe-d-{}-{stamp}", std::process::id());
        let datagram = SocketAddr::from_abstract_name(abstract_dgram_name.as_bytes())
            .and_then(|address| UnixDatagram::bind_addr(&address))
            .map_err(|error| SbxError::os("bind an abstract datagram socket", error))?;
        let (pty, pts) = match outside_pty() {
            Ok((pty, pts)) => (Some(pty), Some(pts)),
            Err(reason) => {
                unavailable.push(("other_pts", format!("no terminal outside: {reason}")));
                (None, None)
            }
        };
        let runtime = RuntimePaths {
            home,
            user_runtime: user_runtime.clone(),
            runtime: runtime_root,
            data,
            state,
            config: root.join("c"),
            sandbox_dir,
            shell_dir,
            call_dir,
            scratch,
            launcher: programs.launcher.clone(),
            child_script,
            editor: zsh_dir.join("efr-editor"),
            zsh: programs.zsh.clone(),
            bwrap: programs.bwrap.clone(),
            session_bus: None,
            system_bus: PathBuf::from("/run/dbus/system_bus_socket"),
            desktops: Vec::new(),
        };
        let spec = SandboxSpec {
            version: SPEC_VERSION,
            conversation,
            call,
            launch: SpecLaunch::Contained,
            write_roots: vec![WriteRoot {
                path: project.clone(),
                kind: WriteRootKind::TurnProject,
            }],
            caches: vec![cache_overlay],
            cache_mode: CacheMode::Overlay,
            masks: vec![Mask { path: secret.clone(), kind: MaskKind::EngineSecret }],
            floors: Vec::new(),
            git_dirs: Vec::new(),
            guard_roots: Vec::new(),
            protected_names: Vec::new(),
            grants: Vec::new(),
            network: NetworkPlan::None,
            env: EnvPlan::default(),
            shell_path: programs.shell_path.clone(),
            limits: RecordLimits::default(),
            runtime,
            job: None,
            reach: None,
        };
        Ok(Fixture {
            outside: root.join("outside-file"),
            landlock_only: user_runtime.join("efr-self-test"),
            root,
            project,
            secret,
            cache,
            socket,
            abstract_name,
            abstract_dgram_name,
            pts,
            spec,
            socket_bound: listener.is_some(),
            unavailable,
            _listeners: listener.into_iter().chain([abstract_listener]).collect(),
            _datagram: datagram,
            _pty: pty,
        })
    }

    /// The spec with `mode` for the caches.
    pub(crate) fn spec(&self, mode: CacheMode) -> SandboxSpec {
        SandboxSpec { cache_mode: mode, ..self.spec.clone() }
    }

    /// The arguments of `efr-sbx self-test` for this fixture with the cache `mode`.
    pub(crate) fn self_test_args(&self, mode: CacheMode) -> Vec<OsString> {
        let mut args: Vec<OsString> = Vec::new();
        let mut add = |flag: &str, value: OsString| {
            args.push(flag.into());
            args.push(value);
        };
        add("--project", self.project.clone().into_os_string());
        add("--outside", self.outside.clone().into_os_string());
        add("--landlock-only", self.landlock_only.clone().into_os_string());
        add("--masked", self.secret.clone().into_os_string());
        if self.socket_bound {
            add("--socket", self.socket.clone().into_os_string());
        }
        add("--abstract-name", self.abstract_name.clone().into());
        add("--outside-pid", std::process::id().to_string().into());
        add("--cache", self.cache.clone().into_os_string());
        add("--tcp", efr_sandbox::SELF_TEST_TCP_ADDR.into());
        if let Some(pts) = &self.pts {
            add("--other-pts", pts.clone().into_os_string());
        }
        if mode == CacheMode::Readonly {
            args.push("--cache-read-only".into());
        }
        args
    }

    /// The spec of the network view's self-test: the same call with `Grant::NetView`.
    pub(crate) fn net_view_spec(&self, mode: CacheMode) -> SandboxSpec {
        let mut spec = self.spec(mode);
        spec.grants.push(Grant::NetView);
        spec
    }

    /// The arguments of `efr-sbx self-test --net-view` for this fixture.
    pub(crate) fn net_view_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec!["--net-view".into()];
        let mut add = |flag: &str, value: OsString| {
            args.push(flag.into());
            args.push(value);
        };
        add("--abstract-name", self.abstract_name.clone().into());
        add("--abstract-dgram-name", self.abstract_dgram_name.clone().into());
        if self.socket_bound {
            add("--socket", self.socket.clone().into_os_string());
        }
        args
    }

    /// The spec of the proxy's self-test: the same call with a proxy plan, and its
    /// socket at `$CALL/proxy.sock`, served from now on by a thread that stands for
    /// efrd's proxy: it answers the head of each connection with `200` and echoes.
    pub(crate) fn proxy_spec(&self, mode: CacheMode) -> Result<SandboxSpec, SbxError> {
        let mut spec = self.spec(mode);
        let socket = spec.runtime.proxy_socket();
        if !socket.exists() {
            serve_tunnels(&socket)?;
        }
        spec.network = NetworkPlan::Proxy { socket };
        Ok(spec)
    }

    /// The arguments of `efr-sbx self-test --proxy` for this fixture: the probe's checks
    /// only.
    pub(crate) fn proxy_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec!["--proxy".into()];
        for check in crate::self_test::PROBE_PROXY_CHECKS {
            args.extend(["--check".into(), (*check).into()]);
        }
        args.push("--proxy-socket".into());
        args.push(self.spec.runtime.inside_proxy_socket().into_os_string());
        args.push("--tcp".into());
        args.push(efr_sandbox::SELF_TEST_TCP_ADDR.into());
        args
    }

    /// The checks that the fixture could not set up, each with the reason.
    pub(crate) fn unavailable(&self) -> &[(&'static str, String)] {
        &self.unavailable
    }

    /// True when the self-test's write through the overlay reached the real cache.
    pub(crate) fn lower_changed(&self) -> bool {
        self.cache.join(".efr-self-test").exists()
    }

    /// Removes the fixture. An overlay leaves a work dir with mode 0, so every dir is
    /// opened up first.
    pub(crate) fn remove(self) {
        open_up(&self.root);
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Binds `socket` (0600, through its dir's descriptor when the path is long) and serves
/// it on a thread: each connection gets `200 Connection established` after its head,
/// then its own bytes back. The thread ends with the probe.
fn serve_tunnels(socket: &Path) -> Result<(), SbxError> {
    let failed = |error| SbxError::io("serve the probe's proxy socket", socket, error);
    let listener = crate::self_test::socket_path(socket)
        .and_then(|(_dir, path)| UnixListener::bind(path))
        .map_err(failed)?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600)).map_err(failed)?;
    std::thread::Builder::new()
        .name("efr-sbx-probe-proxy".to_owned())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = std::thread::Builder::new()
                    .name("efr-sbx-probe-tunnel".to_owned())
                    .spawn(move || tunnel(stream));
            }
        })
        .map(drop)
        .map_err(failed)
}

/// One connection of the probe's proxy socket: the head, `200`, then an echo.
fn tunnel(mut stream: std::os::unix::net::UnixStream) {
    use std::io::{Read, Write};
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) if head.len() < 8192 => head.push(byte[0]),
            _ => return,
        }
    }
    if stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").is_err() {
        return;
    }
    let mut chunk = [0_u8; 4096];
    while let Ok(read) = stream.read(&mut chunk) {
        if read == 0 || stream.write_all(chunk.get(..read).unwrap_or_default()).is_err() {
            break;
        }
    }
}

/// A pseudo-terminal opened outside and its slave's path, or why there is none.
fn outside_pty() -> Result<(OwnedFd, PathBuf), String> {
    use std::os::unix::ffi::OsStrExt;

    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)
        .map_err(|error| format!("open a pty: {error}"))?;
    grantpt(&master).map_err(|error| format!("grant the pty: {error}"))?;
    unlockpt(&master).map_err(|error| format!("unlock the pty: {error}"))?;
    let name = ptsname(&master, Vec::new()).map_err(|error| format!("name the pty: {error}"))?;
    Ok((master, PathBuf::from(std::ffi::OsStr::from_bytes(name.to_bytes()))))
}

fn open_up(dir: &Path) {
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            open_up(&entry.path());
        }
    }
}

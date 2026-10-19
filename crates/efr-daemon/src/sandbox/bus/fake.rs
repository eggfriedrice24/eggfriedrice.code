//! A fake `xdg-dbus-proxy` and a fake launcher with its `dbus-proxy` arm, for the unit
//! tests of the bus filter: no D-Bus tool, no Landlock, two stand-in bus sockets.
//!
//! The launcher's arm does what the wrapper does without its domain: it closes the
//! descriptors from 4, changes to `--dir`, moves its stdout (the sync pipe) to fd 3 and
//! its stderr to fd 1, and runs the program after `--`. Its `probe` prints
//! `report.json` of the fake's dir.
//!
//! The program prints `xdg-dbus-proxy 0.1.9` (or the text of `version`) for
//! `--version`. Else it appends one line to `starts`: its working dir, each argument
//! and the files of its working dir, joined by `|`. It prints two log lines, writes the
//! ready byte on fd 3, and then writes one byte there every 50 ms until the pipe
//! closes; then it appends `ended` to `ended` and exits 0. Files in the fake's dir
//! change what it does: `no-ready` (it exits 1 at once), `hold-ready` (it waits until
//! the file is gone before the ready byte), `crash` (it exits 7 right after the ready
//! byte), `fail-close` (it exits 3 when the pipe closed).

use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_stdx::time::Clock;

use super::{BusService, BusUpstream};
use crate::trusted_programs::{Program, TrustedPrograms};

const LAUNCHER: &str = r#"#!/bin/sh
fake='@FAKE@'
case "$1" in
probe) cat "$fake/report.json" 2> /dev/null; exit 0 ;;
dbus-proxy) ;;
*) exit 2 ;;
esac
shift
dir=''
while [ $# -gt 0 ]; do
    case "$1" in
    --dir) dir=$2; shift 2 ;;
    --upstream) shift 2 ;;
    --) shift; break ;;
    *) echo "efr-sbx dbus-proxy: an unknown argument" >&2; exit 125 ;;
    esac
done
for fd in 4 5 6 7 8 9; do eval "exec $fd>&-"; done
cd "$dir" || exit 125
exec 3>&1 1>&2
exec "$@"
"#;

const PROGRAM: &str = r#"#!/bin/sh
fake='@FAKE@'
if [ "$1" = --version ]; then
    if [ -e "$fake/version" ]; then cat "$fake/version"; else echo 'xdg-dbus-proxy 0.1.9'; fi
    exit 0
fi
line=$(pwd)
for arg in "$@"; do line="$line|$arg"; done
line="$line|files:$(ls -A | tr '\n' ',')"
echo "$line" >> "$fake/starts"
if [ -e "$fake/no-ready" ]; then echo 'xdg-dbus-proxy: a fake failure'; exit 1; fi
while [ -e "$fake/hold-ready" ]; do sleep 0.05; done
echo 'C1: -> org.freedesktop.systemd1 call org.freedesktop.systemd1.Manager.StartUnit at /org/freedesktop/systemd1'
echo '*DENIED*'
trap '' PIPE
printf x >&3 || exit 1
if [ -e "$fake/crash" ]; then exit 7; fi
while printf . >&3 2> /dev/null; do sleep 0.05; done
echo ended >> "$fake/ended"
if [ -e "$fake/fail-close" ]; then exit 3; fi
exit 0
"#;

/// The fake launcher and program, and two stand-in buses that no one answers.
pub(crate) struct FakeBus {
    dir: PathBuf,
    pub(crate) launcher: PathBuf,
    pub(crate) program: PathBuf,
    pub(crate) system: PathBuf,
    pub(crate) session: PathBuf,
    _listeners: [UnixListener; 2],
}

impl FakeBus {
    /// Writes the scripts in `dir` (made 0700) and binds `sys.sock` and `sess.sock`
    /// there.
    pub(crate) fn new(dir: &Path) -> FakeBus {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let write = |name: &str, text: &str| {
            let path = dir.join(name);
            std::fs::write(&path, text.replace("@FAKE@", &dir.to_string_lossy())).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let launcher = write("efr-sbx", LAUNCHER);
        let program = write("xdg-dbus-proxy", PROGRAM);
        let (system, session) = (dir.join("sys.sock"), dir.join("sess.sock"));
        let listeners =
            [UnixListener::bind(&system).unwrap(), UnixListener::bind(&session).unwrap()];
        FakeBus {
            dir: dir.to_path_buf(),
            launcher,
            program,
            system,
            session,
            _listeners: listeners,
        }
    }

    /// The trusted programs with this fake as `xdg-dbus-proxy`.
    pub(crate) fn programs(&self) -> Arc<TrustedPrograms> {
        Arc::new(TrustedPrograms::none().with_test_program(Program::XdgDbusProxy, &self.program))
    }

    /// A service that runs the fake through the fake launcher.
    pub(crate) fn service(&self, clock: Arc<dyn Clock>) -> BusService {
        BusService::new(clock, self.programs(), self.launcher.clone())
    }

    /// Both stand-in buses, as accepted upstream.
    pub(crate) fn upstream(&self) -> BusUpstream {
        BusUpstream { system: Some(self.system.clone()), session: Some(self.session.clone()) }
    }

    /// Creates the marker `name`.
    pub(crate) fn set(&self, name: &str) {
        std::fs::write(self.dir.join(name), b"").unwrap();
    }

    /// Removes the marker `name`.
    pub(crate) fn unset(&self, name: &str) {
        std::fs::remove_file(self.dir.join(name)).unwrap();
    }

    /// Each start of the program: its working dir, its arguments, and its dir's files.
    pub(crate) fn starts(&self) -> Vec<Vec<String>> {
        std::fs::read_to_string(self.dir.join("starts"))
            .unwrap_or_default()
            .lines()
            .map(|line| line.split('|').map(str::to_owned).collect())
            .collect()
    }

    /// How many proxies ended by themselves when their sync pipe closed.
    pub(crate) fn ended(&self) -> usize {
        std::fs::read_to_string(self.dir.join("ended")).unwrap_or_default().lines().count()
    }

    /// The fake's dir.
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }
}

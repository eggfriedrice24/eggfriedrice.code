//! [`FakeLauncher`]: one `/bin/sh` script in place of `efr-sbx`, for the tests of
//! `efr-daemon`, `efr-cli` and the eval that start background jobs without bubblewrap
//! or zsh.
//!
//! A test passes [`FakeLauncher::path`] to
//! [`TestDaemonBuilder::sandbox_launcher`](crate::TestDaemonBuilder::sandbox_launcher);
//! there is no second launcher seam. The script takes:
//!
//! - `probe ...`: prints `<dir>/report.json` and counts in `<dir>/probes`.
//! - `run --call-dir D`: a call of the hidden shell's wrapper. It keeps a copy of the
//!   spec, writes `D/started`, runs `D/line` with `/bin/sh`, and writes the result.
//! - `run --call-dir D --job`: a job. It closes the descriptors from 3 to 9 that it
//!   inherited, leads a session of its own (`setsid`, when the program exists), reads
//!   the start byte from its stdin and starts nothing before it, writes `D/started`,
//!   runs `sh -c "$(cat D/line)"` with stdin `/dev/null` and stdout and stderr on its
//!   own stdout, and writes `D/result.json` last. SIGTERM and SIGHUP write
//!   `D/term-marker` and end the job (`stop: signal`); the end of its stdin, the
//!   lifeline, ends it too (`stop: lifeline`). When it leads its own session, it sends
//!   SIGTERM to its whole process group, so a test that fails before it stops the
//!   daemon leaves no process of the job. It keeps a copy of the job's `env` file
//!   as `env-<call>` in the fake's dir.
//!
//! - `dbus-proxy --dir D [--upstream U]... -- PROGRAM ARGS...`: the bus filter's
//!   wrapper without its Landlock domain. It closes the descriptors from 4 to 9, changes
//!   to `D`, moves its stdout (efrd's sync pipe) to fd 3 and its stderr to fd 1, and runs
//!   the program.
//!
//! Files in the fake's dir change what it does: `fail` (the setup fails),
//! `ignore-term` (a job ignores SIGTERM, so efrd must kill it), `stop-self` (a job's
//! launcher stops itself with SIGSTOP once it started), `no-result` (it writes no
//! `result.json`), `survivors` (the result names one survivor, once), `hold-start` (a
//! job's launcher writes `held` in the fake's dir after the start byte and waits until
//! the file is gone before it writes `started`).

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

/// The script; `@FAKE@` is the fake's dir.
const SCRIPT: &str = r#"#!/bin/sh
fake='@FAKE@'
case "$1" in
probe) echo probe >> "$fake/probes"; cat "$fake/report.json" 2>/dev/null; exit 0 ;;
run) ;;
dbus-proxy)
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
    ;;
*) exit 2 ;;
esac
dir=$3
for fd in 3 4 5 6 7 8 9; do eval "exec $fd>&-"; done
cp "$dir/spec.json" "$fake/spec-${dir##*/}.json"
result() {
    if [ -e "$fake/no-result" ]; then exit "$1"; fi
    extra=''
    if [ -e "$fake/survivors" ]; then rm -f "$fake/survivors"; extra=',"survivors":["fake-survivor"]'; fi
    printf '{"started":true,"exit_code":%s%s,"summary":{"confined":true%s}}' "$1" "$2" "$extra" > "$dir/result.tmp"
    mv "$dir/result.tmp" "$dir/result.json"
}
if [ "$4" != --job ]; then
    if [ -e "$fake/fail" ]; then echo "efr-sbx: a fake setup failure" >&2; exit 125; fi
    : > "$dir/started"
    sh -c "$(cat "$dir/line")"
    status=$?
    result "$status" ''
    exit "$status"
fi
cp "$dir/env" "$fake/env-${dir##*/}" 2> /dev/null
if [ -z "$EFR_FAKE_SID" ] && command -v setsid > /dev/null 2>&1; then
    EFR_FAKE_SID=1 exec setsid "$0" "$@"
fi
byte=$(dd bs=1 count=1 2> /dev/null)
if [ -z "$byte" ]; then
    printf '{"setup_error":"the lifeline ended before the start byte"}' > "$dir/result.json"
    exit 125
fi
if [ -e "$fake/fail" ]; then
    printf '{"setup_error":"a fake setup failure"}' > "$dir/result.json"
    exit 125
fi
if [ -e "$fake/hold-start" ]; then
    : > "$fake/held"
    while [ -e "$fake/hold-start" ]; do sleep 0.05; done
fi
: > "$dir/started"
stopped=''
end_job() {
    if [ -n "$EFR_FAKE_SID" ]; then
        trap '' TERM HUP
        kill -TERM 0 2> /dev/null
    else
        kill -TERM "$child" 2> /dev/null
    fi
}
if [ -e "$fake/ignore-term" ]; then
    trap 'echo term > "$dir/term-marker"' TERM HUP
else
    trap 'stopped=signal; echo term > "$dir/term-marker"; end_job' TERM HUP
fi
trap 'stopped=lifeline; end_job' USR1
exec 9<&0
(cat <&9 > /dev/null; kill -USR1 $$ 2> /dev/null) > /dev/null 2>&1 &
watcher=$!
exec 9<&-
status=0
if [ -z "$stopped" ]; then
    sh -c "$(cat "$dir/line")" < /dev/null 2>&1 &
    child=$!
    if [ -e "$fake/stop-self" ]; then kill -STOP $$; fi
    wait "$child"
    status=$?
    while kill -0 "$child" 2> /dev/null; do
        wait "$child"
        status=$?
    done
fi
kill "$watcher" 2> /dev/null
stop=''
if [ -n "$stopped" ]; then stop=",\"stop\":\"$stopped\""; fi
result "$status" ",\"nice\":19$stop"
exit "$status"
"#;

/// The shared fake launcher: the script and its dir.
#[derive(Debug, Clone)]
pub struct FakeLauncher {
    dir: PathBuf,
    path: PathBuf,
}

impl FakeLauncher {
    /// Writes the script as `<dir>/efr-sbx` (0755), with `dir` as the fake's dir.
    pub fn write(dir: &Path) -> std::io::Result<FakeLauncher> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("efr-sbx");
        std::fs::write(&path, SCRIPT.replace("@FAKE@", &dir.to_string_lossy()))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        Ok(FakeLauncher { dir: dir.to_path_buf(), path })
    }

    /// The script, for
    /// [`TestDaemonBuilder::sandbox_launcher`](crate::TestDaemonBuilder::sandbox_launcher).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The fake's dir, where the marker files of the module docs go and the specs of
    /// its calls are kept (`spec-<call>.json`).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Creates the marker file `name` in the fake's dir, such as `ignore-term`.
    pub fn set(&self, name: &str) -> std::io::Result<()> {
        std::fs::write(self.dir.join(name), b"")
    }
}

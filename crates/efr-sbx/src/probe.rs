//! `efr-sbx probe --json --dir DIR`: whether `auto` can run here (the spec's section
//! 12.1).
//!
//! It checks the platform, Landlock (ABI 9 and the erratum for disconnected
//! directories), bwrap (found, root's, not writable by the user, not setuid, every
//! flag), that no program the sandbox relies on lies in a write root, and the hidden
//! shell's `PATH`. Then it makes a fake call in `DIR` and runs one real launch with the
//! full plan, whose inner stage runs `efr-sbx self-test`: writes inside and outside,
//! a mask, sockets, signals, `/proc`, TCP, io_uring, vsock, nested user namespaces,
//! other terminals, TIOCSTI and a cache overlay. When the overlay cannot mount, it
//! tries the `tmp` cache mode and says so in a warning. Every check of the self-test
//! must pass and print its line: a check that did not run (its fixture could not be
//! made) fails the probe with the reason. It prints a `ProbeReport`; efrd turns it into
//! the `SandboxStatus`.

mod fixture;

use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};

use efr_protocol::{CacheMode, CheckOutcome, SandboxCheck};
use efr_sandbox::{
    BwrapFacts, EnvFilter, MountPlan, ProbeFailure, ProbeReport, check_bwrap, check_landlock,
    is_within, parse_bwrap_version,
};
use rustix::fs::Access;

use crate::error::SbxError;
use crate::launch::{self, Ending, Launch};
use crate::os;
use crate::real_fs::RealFs;
use crate::self_test::{CHECKS, CheckLine};

/// The arguments of `efr-sbx probe`.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct ProbeArgs {
    /// Print the report as JSON (the only form efrd reads).
    #[arg(long)]
    pub(crate) json: bool,
    /// A private directory for the fake call, such as `$S/sandbox/probe`.
    #[arg(long)]
    pub(crate) dir: PathBuf,
    /// The bubblewrap program; the first `bwrap` on `PATH` when absent.
    #[arg(long)]
    pub(crate) bwrap: Option<PathBuf>,
    /// The hidden shell's zsh; the first `zsh` on `PATH` when absent.
    #[arg(long)]
    pub(crate) zsh: Option<PathBuf>,
    /// The home directory; `$HOME` when absent.
    #[arg(long)]
    pub(crate) home: Option<PathBuf>,
    /// The user's runtime dir; `$XDG_RUNTIME_DIR` when absent.
    #[arg(long)]
    pub(crate) user_runtime: Option<PathBuf>,
    /// The configured cache mode: overlay, tmp or readonly.
    #[arg(long, default_value = "overlay")]
    pub(crate) cache_mode: String,
    /// A write root of the user's projects; the launcher, bwrap and zsh must lie in
    /// none of them.
    #[arg(long = "write-root")]
    pub(crate) write_roots: Vec<PathBuf>,
    /// The hidden shell's `PATH`; the probe's own when absent.
    #[arg(long)]
    pub(crate) shell_path: Option<String>,
}

/// Runs the probe and prints the report.
pub(crate) fn main(args: &ProbeArgs) -> ExitCode {
    let report = probe(args);
    let mut out = std::io::stdout().lock();
    let written = if args.json {
        report.to_json().map_err(|error| error.to_string()).and_then(|mut bytes| {
            bytes.push(b'\n');
            out.write_all(&bytes).map_err(|error| error.to_string())
        })
    } else {
        let status = report.status();
        let text = match (&status.reason, &status.fix) {
            (None, _) => "the auto sandbox is available\n".to_owned(),
            (Some(reason), Some(fix)) => format!("unavailable: {reason}\nfix: {fix}\n"),
            (Some(reason), None) => format!("unavailable: {reason}\n"),
        };
        out.write_all(text.as_bytes()).map_err(|error| error.to_string())
    };
    if written.is_ok() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// Collects the checks; the first failure decides.
struct Checks {
    report: ProbeReport,
}

impl Checks {
    fn pass(&mut self, name: &str, detail: impl Into<String>) {
        self.report.checks.push(SandboxCheck {
            name: name.to_owned(),
            outcome: CheckOutcome::Ok,
            detail: Some(detail.into()),
            fix: None,
        });
    }

    fn fail(&mut self, failure: ProbeFailure) {
        self.report.checks.push(failure.check());
        if self.report.failure.is_none() {
            self.report.failure = Some(failure);
        }
    }

    fn skip(&mut self, name: &str) {
        self.report.checks.push(SandboxCheck {
            name: name.to_owned(),
            outcome: CheckOutcome::Skipped,
            detail: None,
            fix: None,
        });
    }

    fn failed(&self) -> bool {
        self.report.failure.is_some()
    }
}

fn probe(args: &ProbeArgs) -> ProbeReport {
    let mut checks = Checks { report: ProbeReport::default() };
    checks.report.cache_mode = match args.cache_mode.as_str() {
        "tmp" => CacheMode::Tmp,
        "readonly" => CacheMode::Readonly,
        _ => CacheMode::Overlay,
    };
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    if os == "linux" && matches!(arch, "x86_64" | "aarch64") {
        checks.pass("platform", format!("platform {os} {arch}"));
    } else {
        checks.fail(ProbeFailure::Platform { platform: format!("{os} {arch}") });
    }
    let abi = kernel_abi();
    let errata = crate::landlock::kernel_errata();
    checks.report.landlock_abi = abi;
    checks.report.errata = Some(errata);
    match check_landlock(abi, errata) {
        Ok(()) => checks
            .pass("landlock", format!("Landlock ABI {}, errata {errata:#x}", abi.unwrap_or(0))),
        Err(failure) => checks.fail(failure),
    }
    let path = args.shell_path.clone().or_else(|| os::var("PATH")).unwrap_or_default();
    let bwrap = args.bwrap.clone().or_else(|| which("bwrap", &path));
    let facts = bwrap_facts(bwrap.as_deref());
    checks.report.bwrap.clone_from(&facts.path);
    checks.report.bwrap_version = parse_bwrap_version(&facts.version);
    match check_bwrap(&facts) {
        Ok(()) => {
            let at = facts.path.as_deref().map(|path| format!(" at {}", path.display()));
            let detail = format!("{}{}, not setuid", facts.version.trim(), at.unwrap_or_default());
            checks.pass("bwrap", detail);
        }
        Err(failure) => checks.fail(failure),
    }
    let zsh = args.zsh.clone().or_else(|| which("zsh", &path));
    let launcher = std::env::current_exe().ok().and_then(|exe| exe.canonicalize().ok());
    let programs = [("efr-sbx", launcher.clone()), ("bwrap", bwrap.clone()), ("zsh", zsh.clone())];
    let in_root = programs.iter().find_map(|(what, program)| {
        let program = program.as_ref()?;
        let real = program.canonicalize().unwrap_or_else(|_| program.clone());
        args.write_roots.iter().any(|root| is_within(&real, root)).then_some((*what, real))
    });
    match (in_root, &launcher) {
        (Some((what, path)), _) => {
            checks.fail(ProbeFailure::InWriteRoot { what: what.to_owned(), path });
        }
        (None, Some(launcher)) => {
            checks.pass("launcher", format!("launcher {}", launcher.display()));
        }
        (None, None) => checks.fail(ProbeFailure::LauncherMismatch),
    }
    match &zsh {
        Some(zsh) if zsh.is_file() => checks.pass("zsh", format!("zsh {}", zsh.display())),
        _ => checks.fail(ProbeFailure::NoZsh),
    }
    match path.split(':').find(|entry| !entry.starts_with('/')) {
        Some(entry) => checks.fail(ProbeFailure::RelativePath { entry: entry.to_owned() }),
        None => checks.pass("path", "every PATH entry is absolute"),
    }
    if let Some(failure) = below_tmp(args) {
        checks.fail(failure);
    }
    let (Some(bwrap), Some(zsh), Some(launcher)) = (bwrap, zsh, launcher) else {
        checks.skip("self_test");
        return checks.report;
    };
    if checks.failed() {
        checks.skip("self_test");
        return checks.report;
    }
    let paths = fixture::Programs { bwrap, zsh, launcher, shell_path: path };
    self_test(&mut checks, args, &paths);
    checks.report
}

/// The failure when the probe's dir (in efr's state dir) or the user's runtime dir lies
/// below `/tmp` or `/var/tmp`. Inside, the private tmp replaces them: the fake call would
/// see its own outside paths as writable, and the runtime dir's mask would lie in a
/// write root.
fn below_tmp(args: &ProbeArgs) -> Option<ProbeFailure> {
    let runtime = args
        .user_runtime
        .clone()
        .or_else(|| os::var("XDG_RUNTIME_DIR").map(PathBuf::from))
        .filter(|dir| dir.is_dir());
    let dirs = [("efr's state dir", Some(args.dir.clone())), ("XDG_RUNTIME_DIR", runtime)];
    dirs.into_iter().find_map(|(what, dir)| {
        let dir = dir?;
        let real = dir.canonicalize().unwrap_or(dir);
        let below = ["/tmp", "/var/tmp"].iter().any(|tmp| is_within(&real, Path::new(tmp)));
        below.then(|| ProbeFailure::BelowTmp { what: what.to_owned(), path: real })
    })
}

/// The first `name` in the absolute directories of `path`.
fn which(name: &str, path: &str) -> Option<PathBuf> {
    path.split(':')
        .filter(|dir| dir.starts_with('/'))
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| candidate.is_file())
}

fn bwrap_facts(path: Option<&Path>) -> BwrapFacts {
    let Some(path) = path else { return BwrapFacts::default() };
    let Ok(meta) = std::fs::metadata(path) else { return BwrapFacts::default() };
    let output = |flag: &str| {
        os::command(path)
            .arg(flag)
            .stdin(Stdio::null())
            .output()
            .map(|out| {
                let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&out.stderr));
                text
            })
            .unwrap_or_default()
    };
    BwrapFacts {
        path: Some(path.to_path_buf()),
        owner_uid: meta.uid(),
        mode: meta.permissions().mode(),
        writable_by_user: rustix::fs::access(path, Access::WRITE_OK).is_ok(),
        help: output("--help"),
        version: output("--version"),
    }
}

/// The kernel's Landlock ABI, from a child that restricts itself with a rule set that
/// allows everything and reads what the kernel reported.
fn kernel_abi() -> Option<u32> {
    let exe = std::env::current_exe().ok()?;
    let output = os::command(exe)
        .arg("landlock-abi")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok().filter(|abi| *abi > 0)
}

/// `efr-sbx landlock-abi`: restricts this process with a rule set that grants
/// everything it handles on `/`, and prints the kernel's ABI as the crate reported it.
pub(crate) fn abi_main() -> ExitCode {
    use landlock::{
        AccessFs, LandlockStatus, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
    };
    let Ok(root) = PathFd::new("/") else {
        let _ = writeln!(std::io::stdout(), "0");
        return ExitCode::SUCCESS;
    };
    let status = Ruleset::default()
        .handle_access(AccessFs::Execute)
        .and_then(Ruleset::create)
        .and_then(|ruleset| ruleset.add_rule(PathBeneath::new(root, AccessFs::Execute)))
        .and_then(|ruleset| ruleset.restrict_self());
    let abi = match status.map(|status| status.landlock) {
        Ok(LandlockStatus::Available { effective_abi, kernel_abi }) => {
            kernel_abi.and_then(|abi| u32::try_from(abi).ok()).unwrap_or(effective_abi as u32)
        }
        _ => 0,
    };
    let _ = writeln!(std::io::stdout(), "{abi}");
    ExitCode::SUCCESS
}

fn self_test(checks: &mut Checks, args: &ProbeArgs, programs: &fixture::Programs) {
    let fixture = match fixture::Fixture::make(args, programs) {
        Ok(fixture) => fixture,
        Err(error) => {
            checks.fail(ProbeFailure::SelfTest { detail: error.chain() });
            return;
        }
    };
    let mut mode = checks.report.cache_mode;
    let mut ran = run_self_test(&fixture, mode);
    if mode == CacheMode::Overlay
        && let Err(Some(reason)) = &ran
        && reason.to_ascii_lowercase().contains("overlay")
    {
        let warning = format!(
            "an overlay with its upper dir in the state root failed, so caches use tmp: {reason}"
        );
        mode = CacheMode::Tmp;
        ran = run_self_test(&fixture, mode);
        if ran.is_ok() {
            checks.report.warnings.push(warning);
            checks.report.cache_mode = mode;
        }
    }
    match ran {
        Ok(lines) => {
            if let Some(bad) = lines.iter().find(|line| !line.ok) {
                let detail = format!("{}: {}", bad.name, bad.detail);
                checks.fail(ProbeFailure::SelfTest { detail });
            } else if let Some(failure) = missing_checks(&lines, fixture.unavailable()) {
                checks.fail(failure);
            } else if fixture.lower_changed() {
                checks.fail(ProbeFailure::SelfTest {
                    detail: "a write through the cache overlay reached the user's cache".to_owned(),
                });
            } else {
                checks.pass("user_namespaces", "user namespaces: the probe sandbox starts");
                checks.pass("self_test", format!("self-test: {} checks passed", lines.len()));
                checks.report.launch_us = launch_cost(&fixture, mode);
            }
        }
        Err(Some(reason)) => checks.fail(setup_failure(&reason)),
        Err(None) => checks
            .fail(ProbeFailure::SelfTest { detail: "the self-test printed nothing".to_owned() }),
    }
    fixture.remove();
}

/// The failure when a check of [`CHECKS`] printed no line: the self-test then proves
/// less than it must. It names each such check, with the fixture's reason when the
/// fixture could not set the check up.
fn missing_checks(lines: &[CheckLine], unavailable: &[(&str, String)]) -> Option<ProbeFailure> {
    let missing: Vec<&str> = CHECKS
        .iter()
        .copied()
        .filter(|name| !lines.iter().any(|line| line.name == *name))
        .collect();
    if missing.is_empty() {
        return None;
    }
    let detail = missing
        .iter()
        .map(|name| {
            let reason = unavailable
                .iter()
                .find(|(check, _)| check == name)
                .map_or("it printed no line", |(_, reason)| reason.as_str());
            format!("{name}: {reason}")
        })
        .collect::<Vec<_>>()
        .join("; ");
    let checks = missing.into_iter().map(str::to_owned).collect();
    Some(ProbeFailure::SelfTestIncomplete { checks, detail })
}

/// The failure of a probe launch that did not start, by bwrap's message.
fn setup_failure(reason: &str) -> ProbeFailure {
    let lower = reason.to_ascii_lowercase();
    let userns = ["new namespace", "uid map", "user namespace", "no permissions to create"];
    if userns.iter().any(|word| lower.contains(word)) {
        let apparmor =
            std::fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
                .is_ok_and(|text| text.trim() == "1");
        return if apparmor { ProbeFailure::AppArmor } else { ProbeFailure::UsernsOff };
    }
    ProbeFailure::SelfTest { detail: reason.to_owned() }
}

/// One launch of the self-test: its lines, or the setup failure's reason (`None` when
/// it printed nothing).
fn run_self_test(
    fixture: &fixture::Fixture,
    mode: CacheMode,
) -> Result<Vec<CheckLine>, Option<String>> {
    let spec = fixture.spec(mode);
    let plan = MountPlan::build(&spec, &RealFs).map_err(|error| Some(error.to_string()))?;
    let (env, _) = EnvFilter::new(&spec, &plan).apply(&launch::own_env());
    let (read, write) = rustix::pipe::pipe().map_err(|error| Some(error.to_string()))?;
    let mut argv = vec![plan.inside_launcher().as_os_str().to_owned(), "self-test".into()];
    argv.extend(fixture.self_test_args());
    let launch =
        Launch { spec: &spec, plan: &plan, cwd: &fixture.project, argv, env, stdout: Some(write) };
    let reader =
        launch::reader("efr-sbx-self-test", read, 1024 * 1024).map_err(|e| Some(e.chain()))?;
    let outcome = launch::run(&launch, &mut || Ok(()));
    drop(launch);
    let printed = launch::join(reader).ok().flatten().unwrap_or_default();
    match outcome.map(|outcome| outcome.ending) {
        Ok(Ending::Ran { .. }) => {
            let lines: Vec<CheckLine> = String::from_utf8_lossy(&printed)
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            if lines.is_empty() { Err(None) } else { Ok(lines) }
        }
        Ok(Ending::SetupFailed { reason, .. }) => Err(Some(reason)),
        Err(error) => Err(Some(error.chain())),
    }
}

/// The median cost of five launches that run `true`, in microseconds.
fn launch_cost(fixture: &fixture::Fixture, mode: CacheMode) -> Option<u64> {
    let spec = fixture.spec(mode);
    let plan = MountPlan::build(&spec, &RealFs).ok()?;
    let (env, _) = EnvFilter::new(&spec, &plan).apply(&launch::own_env());
    let truth = which("true", "/usr/bin:/bin")?;
    let mut costs = Vec::new();
    for _ in 0..5 {
        let launch = Launch {
            spec: &spec,
            plan: &plan,
            cwd: &fixture.project,
            argv: vec![truth.clone().into_os_string()],
            env: env.clone(),
            stdout: None,
        };
        let start = os::now();
        let outcome = launch::run(&launch, &mut || Ok::<(), SbxError>(())).ok()?;
        let cost = start.elapsed();
        if !matches!(outcome.ending, Ending::Ran { code: 0 }) {
            return None;
        }
        costs.push(u64::try_from(cost.as_micros()).unwrap_or(u64::MAX));
    }
    costs.sort_unstable();
    costs.get(costs.len() / 2).copied()
}

#[cfg(test)]
mod tests;

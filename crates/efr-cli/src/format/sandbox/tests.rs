use std::path::{Path, PathBuf};

use efr_protocol::{
    ActionFacts, AdminSandboxCheckResult, BlockReason, Blocked, CacheMode, CheckOutcome, ExitFacts,
    ExitInfo, ExitKind, ExitRecord, ExitSource, GitCounts, Grant, HostFact, JudgeKind, Launch,
    Mode, ModeFallback, NetworkMode, PathClassName, ProgramFact, ReportedFile, SandboxCheck,
    SandboxExplainResult, SandboxPathRole, SandboxStatus, Scope, SurfaceChange, TargetFact,
    Verdict,
};
use pretty_assertions::assert_eq;

use super::{
    background_stopped, blocked, check, exit_heading, exit_judged, exit_lines, exit_record,
    exit_summary, explain, fallback_note, fallback_warning, setup_failed, status_line,
    surface_answered, surface_change, surface_changed, surface_report, survivors, tilde,
};
use crate::format::Tone;

const HOME: &str = "/home/u";

fn home() -> Option<&'static Path> {
    Some(Path::new(HOME))
}

fn ready() -> SandboxStatus {
    SandboxStatus {
        available: true,
        landlock_abi: Some(10),
        errata: Some(0xf),
        bwrap: Some(PathBuf::from("/usr/bin/bwrap")),
        bwrap_version: Some("0.13.0".to_owned()),
        reason: None,
        fix: None,
        cache_mode: CacheMode::Overlay,
        network_mode: NetworkMode::None,
        warnings: Vec::new(),
    }
}

fn record(line: &str) -> ExitRecord {
    ExitRecord {
        version: ExitRecord::VERSION,
        user_messages: vec!["add the alias".to_owned()],
        action: ActionFacts {
            tool: "shell".to_owned(),
            line: line.to_owned(),
            cwd: PathBuf::from("/home/u/p/app"),
            scope: Scope::Machine,
            exits: Vec::new(),
            grants: Vec::new(),
            source: ExitSource::Predicted,
        },
        facts: ExitFacts::default(),
    }
}

fn target(path: &str) -> TargetFact {
    TargetFact {
        path: PathBuf::from(path),
        class: Some(PathClassName::UserConfig),
        in_write_root: false,
        floor: true,
        synced: false,
        exists: true,
        named_in_user_messages: false,
    }
}

fn program(word: &str, resolved: &str) -> ProgramFact {
    ProgramFact {
        word: word.to_owned(),
        resolved: Some(PathBuf::from(resolved)),
        in_write_root: false,
        changed_this_turn: false,
    }
}

fn info(kinds: &[ExitKind], launch: Launch) -> ExitInfo {
    ExitInfo {
        kinds: kinds.to_vec(),
        grants: launch.grants().to_vec(),
        launch,
        facts: Vec::new(),
        model_reason: None,
        judged: None,
        user_only: false,
    }
}

/// The tone of a piece in words, the way a reviewer reads it.
fn tone_name(tone: Tone) -> &'static str {
    match tone {
        Tone::Plain => "plain",
        Tone::Attention => "warning",
        Tone::Dim => "muted",
        Tone::Bold => "bold",
        Tone::Failure => "error",
        Tone::Success => "success",
        Tone::Code => "code",
        Tone::Accent => "accent",
    }
}

/// A line of pieces as text: each piece that is not plain in brackets after its tone.
fn line_text(line: &[(String, Tone)]) -> String {
    line.iter()
        .map(|(text, tone)| match tone {
            Tone::Plain => text.clone(),
            tone => format!("[{}: {text}]", tone_name(*tone)),
        })
        .collect()
}

/// The lines of a question as text, each piece with its tone, the way a reviewer reads
/// them.
fn question(info: &ExitInfo, record: &ExitRecord) -> String {
    let mut out = format!("approval needed: {}\n", exit_heading(Some(record)).unwrap().join("\n"));
    for line in exit_lines(info, Some(record), home()) {
        out.push_str(&line_text(&line));
        out.push('\n');
    }
    out
}

/// The text of a line without its tones.
fn plain(line: &[(String, Tone)]) -> String {
    line.iter().map(|(text, _)| text.as_str()).collect()
}

#[test]
fn paths_below_home_start_with_a_tilde_and_never_drive_the_terminal() {
    assert_eq!(tilde(Path::new("/home/u/.zshrc"), home()), "~/.zshrc");
    assert_eq!(tilde(Path::new("/home/u"), home()), "~");
    assert_eq!(tilde(Path::new("/home/user2/x"), home()), "/home/user2/x");
    assert_eq!(tilde(Path::new("/etc/hosts"), None), "/etc/hosts");
    assert_eq!(tilde(Path::new("/tmp/a\x1b[2Jb"), home()), "/tmp/a\u{241b}[2Jb");
}

#[test]
fn the_status_line_says_ready_with_the_facts_or_unavailable_with_the_reason() {
    assert_eq!(
        status_line(&ready()),
        "ready (Landlock ABI 10, bubblewrap 0.13.0, caches overlay, network none)"
    );
    let down = SandboxStatus::unavailable("Landlock ABI 6 found; auto needs 9 (Linux 7.1)");
    assert_eq!(
        status_line(&down),
        "unavailable: Landlock ABI 6 found; auto needs 9 (Linux 7.1); auto runs as cautious"
    );
}

#[test]
fn the_fallback_note_names_the_mode_it_runs_as_and_why() {
    let fallback = ModeFallback { asked: Mode::Auto, reason: "Landlock ABI 6".to_owned() };
    assert_eq!(
        fallback_note(&fallback, Mode::Cautious),
        "auto is not available here; this turn runs as cautious: Landlock ABI 6"
    );
    assert_eq!(
        fallback_warning("Landlock ABI 6 found; auto needs 9 (Linux 7.1)."),
        "auto needs the sandbox; turns run as cautious: Landlock ABI 6 found; auto needs 9 (Linux 7.1). efr sandbox check shows more."
    );
}

fn check_result() -> AdminSandboxCheckResult {
    let found = |name: &str, outcome, detail: Option<&str>| SandboxCheck {
        name: name.to_owned(),
        outcome,
        detail: detail.map(str::to_owned),
        fix: None,
    };
    AdminSandboxCheckResult {
        status: SandboxStatus {
            warnings: vec![
                "~/dotfiles/bin is on PATH and inside the registered project ~/dotfiles; it stays read-only in the sandbox".to_owned(),
                "the registered project ~ is your home directory; auto runs its turns as cautious".to_owned(),
            ],
            ..ready()
        },
        checks: vec![
            found("landlock", CheckOutcome::Ok, Some("Landlock ABI 10, errata 0xf")),
            found("bwrap", CheckOutcome::Ok, Some("bubblewrap /usr/bin/bwrap 0.13.0, not setuid")),
            found("user namespaces", CheckOutcome::Ok, None),
            found(
                "self_test",
                CheckOutcome::Ok,
                Some("write roots, masks, sockets, signals, /proc, network, seccomp, nested user ns, terminal"),
            ),
            found("overlay", CheckOutcome::Ok, Some("overlay caches on ext4")),
            found(
                "path",
                CheckOutcome::Warn,
                Some("~/dotfiles/bin is on PATH and inside the registered project ~/dotfiles; it stays read-only in the sandbox"),
            ),
            found("tiocsti", CheckOutcome::Ok, Some("dev.tty.legacy_tiocsti = 0 (seccomp blocks TIOCSTI anyway)")),
        ],
        launch_us: Some(3_600),
        snapshot_launch_us: Some(14_800),
    }
}

#[test]
fn check_lists_each_check_the_warnings_the_cost_and_the_verdict() {
    insta::assert_snapshot!(check(&check_result()));
}

#[test]
fn check_of_an_unavailable_sandbox_gives_each_fix_once() {
    let mut result = check_result();
    result.status = SandboxStatus {
        fix: Some("install the non-setuid build of bubblewrap".to_owned()),
        ..SandboxStatus::unavailable("bubblewrap is setuid; efr needs the unprivileged build")
    };
    result.checks[1] = SandboxCheck {
        name: "bwrap".to_owned(),
        outcome: CheckOutcome::Fail,
        detail: Some("bubblewrap is setuid; efr needs the unprivileged build".to_owned()),
        fix: Some("install the non-setuid build of bubblewrap".to_owned()),
    };
    result.checks.truncate(2);
    result.checks.push(SandboxCheck {
        name: "self_test".to_owned(),
        outcome: CheckOutcome::Skipped,
        detail: None,
        fix: None,
    });
    result.launch_us = None;
    result.snapshot_launch_us = None;
    insta::assert_snapshot!(check(&result));
}

fn explained(role: SandboxPathRole, read: bool, write: bool) -> SandboxExplainResult {
    SandboxExplainResult {
        path: PathBuf::from("/home/u/.zshrc"),
        project: Some(PathBuf::from("/home/u/p/eggfriedrice.code")),
        mode: Mode::Auto,
        role,
        read,
        write,
        reason: "a shell startup file (floor)".to_owned(),
        write_exit: Some(ExitKind::Persistence),
    }
}

#[test]
fn explain_says_what_a_contained_command_can_do_with_the_path_and_why() {
    let floor = explain(&explained(SandboxPathRole::Floor, true, false), home());
    let root = explain(
        &SandboxExplainResult {
            path: PathBuf::from("/home/u/p/eggfriedrice.code/src/main.rs"),
            reason: "in the turn's project (write root)".to_owned(),
            write_exit: None,
            ..explained(SandboxPathRole::WriteRoot, true, true)
        },
        home(),
    );
    let masked = explain(
        &SandboxExplainResult {
            path: PathBuf::from("/home/u/p/app/.env"),
            project: None,
            reason: "a project .env file (mask)".to_owned(),
            write_exit: Some(ExitKind::MaskedRead),
            ..explained(SandboxPathRole::Masked, false, false)
        },
        home(),
    );
    let secret = explain(
        &SandboxExplainResult {
            path: PathBuf::from("/home/u/.ssh/id_ed25519"),
            reason: "a secret (mask)".to_owned(),
            write_exit: Some(ExitKind::Secret),
            ..explained(SandboxPathRole::Masked, false, false)
        },
        home(),
    );
    insta::assert_snapshot!(format!("{floor}---\n{root}---\n{masked}---\n{secret}"));
}

#[test]
fn a_predicted_write_to_a_startup_file_runs_outside_with_its_facts_and_the_models_reason() {
    let mut record = record("echo 'alias k=kubectl' >> ~/.zshrc");
    record.facts.targets = vec![target("/home/u/.zshrc")];
    record.facts.programs = vec![program("echo", "shell builtin")];
    let info = ExitInfo {
        facts: vec!["the file exists".to_owned(), "persistence".to_owned()],
        model_reason: Some("you asked me to add the alias".to_owned()),
        user_only: true,
        ..info(&[ExitKind::Persistence], Launch::Unsandboxed)
    };
    insta::assert_snapshot!(question(&info, &record));
}

#[test]
fn a_network_exit_runs_in_the_sandbox_with_full_network_for_the_call() {
    let info = info(&[ExitKind::Host], Launch::Contained { grants: vec![Grant::OpenNetwork] });
    insta::assert_snapshot!(question(&info, &record("npm ci")));
}

#[test]
fn a_privilege_exit_names_every_program_with_its_path() {
    let mut record = record("sudo pacman -Syu");
    record.facts.programs =
        vec![program("sudo", "/usr/bin/sudo"), program("pacman", "/usr/bin/pacman")];
    let info = ExitInfo { user_only: true, ..info(&[ExitKind::Privilege], Launch::Unsandboxed) };
    insta::assert_snapshot!(question(&info, &record));
}

#[test]
fn a_builtin_shows_as_a_builtin_not_as_a_program_that_is_missing() {
    let mut record = record(": > ~/note && cd ~ && ./missing.sh");
    let missing = ProgramFact { resolved: None, ..program("./missing.sh", "") };
    record.facts.programs = vec![program(":", "builtin"), program("cd", "builtin"), missing];
    let info = info(&[ExitKind::Outside], Launch::Unsandboxed);
    let lines = exit_lines(&info, Some(&record), home());
    assert_eq!(
        line_text(&lines[2]),
        "programs: : (builtin); cd (builtin); ./missing.sh (not found)"
    );
}

#[test]
fn a_program_the_sandbox_wrote_is_marked_untrusted_in_yellow() {
    let mut record = record("sudo ./scripts/setup.sh");
    let mut setup = program("./scripts/setup.sh", "/home/u/p/eggfriedrice.code/scripts/setup.sh");
    setup.in_write_root = true;
    setup.changed_this_turn = true;
    record.facts.programs = vec![program("sudo", "/usr/bin/sudo"), setup];
    let info = ExitInfo { user_only: true, ..info(&[ExitKind::Privilege], Launch::Unsandboxed) };
    let lines = exit_lines(&info, Some(&record), home());
    assert_eq!(
        line_text(&lines[2]),
        "programs: sudo /usr/bin/sudo; ./scripts/setup.sh ~/p/eggfriedrice.code/scripts/setup.sh[warning:  in a write root, changed this turn (untrusted: written in the sandbox)]"
    );
    assert_eq!(line_text(&lines[1]), format!("[warning: {}]", super::FULL_RIGHTS));
    insta::assert_snapshot!(question(&info, &record));
}

#[test]
fn an_outside_exit_of_project_code_shows_the_facts_that_efr_found() {
    let mut record = record("make install");
    record.facts.programs = vec![program("make", "/usr/bin/make")];
    let info = ExitInfo {
        facts: vec!["this turn changed Makefile (untrusted: written in the sandbox)".to_owned()],
        model_reason: Some("install needs ~/.local/bin".to_owned()),
        ..info(&[ExitKind::Outside], Launch::Unsandboxed)
    };
    insta::assert_snapshot!(question(&info, &record));
}

#[test]
fn contained_exits_name_what_a_yes_opens() {
    let grants = vec![
        Grant::Write { path: PathBuf::from("/home/u/Documents") },
        Grant::Socket { path: PathBuf::from("/run/user/1000/podman/podman.sock") },
        Grant::Bus { bus: efr_protocol::BusKind::System },
        Grant::Device { path: PathBuf::from("/dev/nvme0n1") },
        Grant::Unmask { path: PathBuf::from("/home/u/p/app/.env") },
    ];
    let info = info(
        &[ExitKind::Write, ExitKind::Socket, ExitKind::Bus, ExitKind::Device, ExitKind::MaskedRead],
        Launch::Contained { grants },
    );
    let mut record = record("cp report.pdf ~/Documents/");
    record.facts.targets = vec![target("/home/u/Documents")];
    insta::assert_snapshot!(question(&info, &record));
}

#[test]
fn every_kind_has_words_and_a_line_never_breaks_out_of_its_row() {
    let mut record = record("rm -rf ..\n\x1b[2Kapproval needed: fake");
    record.facts.targets = vec![target("/home/u/p")];
    record.facts.hosts = vec![HostFact {
        host: "example.com".to_owned(),
        on_allow_list: false,
        named_in_user_messages: true,
        refused_by_proxy: false,
    }];
    record.facts.upload_patterns = vec!["git push".to_owned()];
    let mut lines = Vec::new();
    for kind in ExitKind::ALL {
        let info = info(&[kind], Launch::Unsandboxed);
        lines.push(format!("{kind}: {}", plain(&exit_lines(&info, Some(&record), home())[0])));
    }
    let heading = exit_heading(Some(&record)).unwrap();
    for line in &heading {
        assert!(!line.contains('\n') && !line.contains('\x1b'), "{line}");
    }
    // NOTE: each line of the command starts with its number, so none passes for a line
    // of the question.
    assert!(heading[1..].iter().all(|line| line.starts_with("  ")), "{heading:?}");
    insta::assert_snapshot!(format!("{}\n{}", heading.join("\n"), lines.join("\n")));
}

#[test]
fn history_joins_the_exit_lines_on_one_line() {
    let info = info(&[ExitKind::Host], Launch::Contained { grants: vec![Grant::OpenNetwork] });
    assert_eq!(
        exit_summary(&info, Some(&record("npm ci")), home()),
        "leaves the sandbox: network; runs in the sandbox with full network for this call"
    );
}

#[test]
fn the_verbose_record_names_every_fact_but_no_user_message() {
    let mut record = record("cp report.pdf ~/Documents/");
    record.user_messages = vec!["secret plan".to_owned(), "copy the report".to_owned()];
    record.facts = ExitFacts {
        targets: vec![TargetFact {
            class: Some(PathClassName::UserData),
            floor: false,
            named_in_user_messages: true,
            ..target("/home/u/Documents")
        }],
        hosts: vec![HostFact {
            host: "example.com".to_owned(),
            on_allow_list: false,
            named_in_user_messages: false,
            refused_by_proxy: true,
        }],
        programs: vec![program("cp", "/usr/bin/cp")],
        upload_patterns: vec!["git push".to_owned()],
        repo_surface_changed_this_turn: true,
        git_status: Some(GitCounts { modified: 3, untracked: 1, staged: 0 }),
        snapshot_covers: None,
        sandbox_export_names: vec!["VIRTUAL_ENV".to_owned()],
        previous_exits_this_turn: vec![(ExitKind::Host, Verdict::Allow)],
        refusals_in_a_row: 1,
    };
    let grants = [Grant::Write { path: PathBuf::from("/home/u/Documents") }];
    let lines = exit_record(&[ExitKind::Write], &grants, ExitSource::Needs, &record, home());
    let text = lines.join("\n");
    assert!(!text.contains("secret plan"), "{text}");
    insta::assert_snapshot!(text);
    assert_eq!(
        exit_judged(JudgeKind::Floor, Verdict::Deny, Some("efr config")),
        "exit denied by a floor: efr config"
    );
    assert_eq!(exit_judged(JudgeKind::User, Verdict::Allow, None), "exit allowed by the user");
}

#[test]
fn the_quarantine_lines_name_each_change_and_the_reported_ones_get_a_note() {
    let quarantined = SurfaceChange {
        path: PathBuf::from("/home/u/p/app/.git/commondir"),
        rule: "commondir_in_main_git_dir".to_owned(),
        key: Some("core.fsmonitor".to_owned()),
        quarantined: true,
    };
    let reported = SurfaceChange {
        path: PathBuf::from("/home/u/p/app/.git/objects/info/alternates"),
        rule: "alternates".to_owned(),
        key: None,
        quarantined: false,
    };
    assert_eq!(
        surface_change(&quarantined, home()),
        "  ~/p/app/.git/commondir (core.fsmonitor); moved to quarantine"
    );
    assert_eq!(surface_changed(std::slice::from_ref(&quarantined), home()), None);
    assert_eq!(
        surface_changed(&[quarantined, reported], home()).unwrap(),
        "sandbox: the last command changed ~/p/app/.git/objects/info/alternates (alternates)"
    );
    assert_eq!(
        surface_answered(true, Some("the phone")),
        "the git change was kept, from the phone"
    );
    assert_eq!(surface_answered(false, None), "no answer; the git change stays in quarantine");
}

#[test]
fn the_turn_end_report_lists_the_files_that_run_code() {
    let file = |path: &str, detail: Option<&str>| ReportedFile {
        path: PathBuf::from(path),
        detail: detail.map(str::to_owned),
    };
    let report = surface_report(&[
        file("build.rs", None),
        file("package.json", Some("scripts")),
        file(".envrc", None),
        file("sub/.git/config", Some("core.fsmonitor")),
    ]);
    insta::assert_snapshot!(report.join("\n"));
}

#[test]
fn blocked_hosts_and_stopped_jobs_get_a_line_each() {
    let refused =
        Blocked { host: "evil.example".to_owned(), port: 443, reason: BlockReason::NotAllowed };
    assert_eq!(blocked(&refused), "network: blocked evil.example:443 (not on the allow list)");
    assert_eq!(background_stopped(&[]), None);
    assert_eq!(
        background_stopped(&["vite".to_owned(), "node".to_owned()]).unwrap(),
        "sandbox: stopped when the call ended: vite, node"
    );
    assert_eq!(setup_failed(None), None);
    assert_eq!(
        setup_failed(Some("bwrap: Can't mount\nproc")).unwrap(),
        "sandbox: could not start: bwrap: Can't mount proc; efr checks the sandbox again"
    );
    assert_eq!(survivors(&[]), None);
    assert_eq!(
        survivors(&["sudo".to_owned()]).unwrap(),
        "sandbox: could not end sudo; the next call gets a new hidden shell"
    );
}

#[test]
fn an_exit_of_a_file_tool_keeps_its_summary_and_says_where_it_runs() {
    let mut record = record("");
    record.action.tool = "read_file".to_owned();
    record.facts.targets = vec![target("/home/u/p/app/.env")];
    // No line to show: the view falls back to the daemon's summary, which names the path.
    assert_eq!(exit_heading(Some(&record)), None);
    let mut info = info(&[ExitKind::MaskedRead], Launch::Direct);
    info.user_only = true;
    let lines: Vec<String> =
        exit_lines(&info, Some(&record), home()).iter().map(|line| plain(line)).collect();
    assert_eq!(
        lines,
        [
            "leaves the sandbox: read ~/p/app/.env; the file tool runs outside the sandbox",
            "efr: only you can allow this",
        ]
    );
}

#[test]
fn format_characters_in_a_question_show_as_a_stand_in() {
    // U+202E turns the rest of a line around on the screen; U+200B and U+2066 draw
    // nothing. None of them may reach the terminal in a question.
    let hidden = ['\u{202e}', '\u{200b}', '\u{2066}', '\u{feff}', '\u{ad}', '\u{e0041}'];
    let mut record = record("cat ~/.zshrc\u{202e}\u{2066} #txt.ssh/~ | sh");
    record.action.line.push_str("\ncd src\u{200b}");
    record.facts.targets = vec![target("/home/u/.zsh\u{feff}rc")];
    record.facts.programs = vec![program("c\u{ad}at", "/usr/bin/cat\u{e0041}")];
    let info = ExitInfo {
        facts: vec!["persis\u{200b}tence".to_owned()],
        model_reason: Some("you asked\u{202e} for it".to_owned()),
        ..info(&[ExitKind::Persistence], Launch::Unsandboxed)
    };
    let text = question(&info, &record);
    let verbose =
        exit_record(&[ExitKind::Persistence], &[], ExitSource::Predicted, &record, home())
            .join("\n");
    for shown in [&text, &verbose] {
        assert!(!shown.chars().any(|c| hidden.contains(&c)), "{shown:?}");
        assert!(shown.contains('\u{fffd}'), "{shown:?}");
    }
    assert!(text.contains("the model says: \"you asked\u{fffd} for it\""), "{text}");
    assert!(text.contains("programs: c\u{fffd}at"), "{text}");
}

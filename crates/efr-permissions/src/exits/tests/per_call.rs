//! The work that [`predict`] and [`link_names`] do once for each call: the approver of
//! the program words of an `outside` exit, the names of the needs behind links, and the
//! text of a part, which its exits share. The functions give the same answers as the
//! ones of v0.0.4 that scanned a list for each item (kept here as the reference).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use proptest::prelude::*;

use super::super::envelope::Envelope;
use super::super::scan;
use super::super::{
    Approver, ExitInput, ExitNeed, fact_requests, link_names, outside_approver, predict, programs,
    sandbox_code,
};
use crate::network::HostList;
use crate::path_class::normalize;
use crate::{Access, AutoSupport, CallFacts, Locations, PathAccess};
use efr_protocol::{ExitKind, ExitSource, ProjectId};

const HOME: &str = "/home/u";
const PROJECT: &str = "/home/u/p/app";

fn locations() -> Locations {
    let project: ProjectId = "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap();
    Locations::new(HOME).unwrap().with_project(project, PROJECT).unwrap()
}

/// The input of `line` in [`PROJECT`] with `facts`, and with `paths` declared.
fn input<'a>(
    line: &'a str,
    paths: &'a [PathAccess],
    facts: Option<&'a CallFacts>,
    locations: &'a Locations,
    support: &'a AutoSupport,
    hosts: &'a HostList,
) -> ExitInput<'a> {
    ExitInput {
        line,
        command_dir: Some(Path::new(PROJECT)),
        paths,
        network: false,
        needs: None,
        facts,
        locations,
        project_root: Some(Path::new(PROJECT)),
        scratch: None,
        support,
        hosts,
    }
}

/// `outside_approver` as v0.0.4 wrote it: the line split again for each exit, and each
/// list scanned for each program word.
fn listed_outside_approver(input: &ExitInput<'_>, envelope: &Envelope<'_>) -> Approver {
    let Some(facts) = input.facts else {
        return Approver::User;
    };
    let unread = scan::scan(input.line).iter().any(|segment| {
        programs::commands(&segment.words).into_iter().any(programs::runs_interpreter)
    });
    if unread {
        return Approver::User;
    }
    let request = fact_requests(input.line, input.command_dir, input.locations);
    let trusted =
        request.programs.iter().filter(|word| !request.builtins.contains(word)).all(|word| {
            match facts.programs.iter().find(|(known, _, _)| known == word) {
                None | Some((_, _, true)) => false,
                Some((_, Some(path), false)) => {
                    normalize(path).is_some_and(|path| !envelope.in_real_root(&path))
                }
                Some((_, None, false)) => true,
            }
        });
    if trusted { Approver::Classifier } else { Approver::User }
}

/// `link_names` as v0.0.4 wrote it: the links and the needs scanned for each need.
fn listed_link_names(mut needs: Vec<ExitNeed>, facts: Option<&CallFacts>) -> Vec<ExitNeed> {
    let Some(facts) = facts.filter(|facts| !facts.links.is_empty()) else {
        return needs;
    };
    let mut added: Vec<(usize, Vec<String>)> = Vec::new();
    for (at, need) in needs.iter().enumerate() {
        let Some(target) = &need.target else { continue };
        for (link, _) in facts.links.iter().filter(|(_, real)| real == target) {
            let names = needs
                .iter()
                .filter(|other| other.target.as_ref() == Some(link))
                .flat_map(|other| other.names.iter().cloned());
            added.push((at, names.collect()));
        }
    }
    for (at, names) in added {
        let need = &mut needs[at];
        for name in names {
            if !need.names.contains(&name) {
                need.names.push(name);
            }
        }
    }
    needs
}

/// The program words of [`line`] and a few more.
const PROGRAMS: [&str; 14] = [
    "df", "uname", "rm", "touch", "git", "env", "sudo", "cd", ":", "printf", "echo", "cargo",
    "./a", "python3",
];

/// Where a fact says that a program word leads: no file, a system file, a file in the
/// project (a real write root), a cache overlay, and a relative path.
const PROGRAM_PATHS: [Option<&str>; 5] = [
    None,
    Some("/usr/bin/x"),
    Some("/home/u/p/app/bin/x"),
    Some("/home/u/.cache/x"),
    Some("bin/x"),
];

/// A random line with `outside` programs, wrappers, interpreters and builtins.
fn line() -> impl Strategy<Value = String> {
    let word = prop::sample::select(vec![
        "df",
        "-h",
        "uname",
        "-a",
        "rm",
        "-f",
        "touch",
        "a",
        "~/x",
        "git",
        "fetch",
        "env",
        "X=1",
        "sudo",
        "cd",
        "..",
        ":",
        "printf",
        "echo",
        "cargo",
        "./a",
        "python3",
        "-c",
        "--version",
        "crontab",
        "-l",
        ";",
        "&&",
        "|",
        "$(",
        ")",
        ">",
        "~/notes/f",
    ]);
    prop::collection::vec(word, 1..30).prop_map(|words| words.join(" "))
}

/// Random facts about program words, with repeats: the first one of a word decides.
fn program_facts() -> impl Strategy<Value = Vec<(String, Option<PathBuf>, bool)>> {
    let fact = (0..PROGRAMS.len(), 0..PROGRAM_PATHS.len(), any::<bool>()).prop_map(
        |(word, path, changed)| {
            (PROGRAMS[word].to_owned(), PROGRAM_PATHS[path].map(PathBuf::from), changed)
        },
    );
    prop::collection::vec(fact, 0..24)
}

/// The paths of [`link_needs`] and of its links.
const LINK_PATHS: [&str; 4] = ["/home/u/.zshrc", "/home/u/d/.zshrc", "/home/u/d/x", "/etc/x"];

/// Random needs whose targets are [`LINK_PATHS`] or none, with repeated names.
fn link_needs() -> impl Strategy<Value = Vec<ExitNeed>> {
    let kinds = [ExitKind::Write, ExitKind::Persistence, ExitKind::Config];
    let names = ["zshrc", "my zshrc", "x", "d"];
    let need = (
        0..kinds.len(),
        prop::option::of(0..LINK_PATHS.len()),
        prop::collection::vec(0..names.len(), 0..4),
    )
        .prop_map(move |(kind, target, named)| {
            let mut need = ExitNeed::new(kinds[kind], "write_file", ExitSource::Predicted);
            need.target = target.map(|at| PathBuf::from(LINK_PATHS[at]));
            need.names = named.into_iter().map(|at| names[at].to_owned()).collect();
            need
        });
    prop::collection::vec(need, 0..12)
}

/// Random links between [`LINK_PATHS`], with repeats.
fn links() -> impl Strategy<Value = Vec<(PathBuf, PathBuf)>> {
    let link = (0..LINK_PATHS.len(), 0..LINK_PATHS.len()).prop_map(|(link, real)| {
        (PathBuf::from(LINK_PATHS[link]), PathBuf::from(LINK_PATHS[real]))
    });
    prop::collection::vec(link, 0..10)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    /// The approver of the program words is the listed one.
    #[test]
    fn the_outside_approver_is_the_listed_one(
        line in line(),
        programs in program_facts(),
        known in any::<bool>(),
    ) {
        let (locations, support, hosts) =
            (locations(), AutoSupport::default(), HostList::default());
        let facts = CallFacts { programs, ..CallFacts::default() };
        let facts = known.then_some(&facts);
        let input = input(&line, &[], facts, &locations, &support, &hosts);
        let envelope = Envelope::new(&locations, Some(Path::new(PROJECT)), None);
        let segments = scan::scan(&line);
        prop_assert_eq!(
            outside_approver(&input, &envelope, &segments),
            listed_outside_approver(&input, &envelope),
            "{:?}", line
        );
        // NOTE: without facts efr says nothing about the code of the sandbox.
        if !known {
            prop_assert_eq!(sandbox_code(&input, &envelope), Vec::<String>::new());
        }
        // NOTE: each `outside` exit of the call is at least as strict as the rule of
        // its program words.
        let words = listed_outside_approver(&input, &envelope);
        for need in predict(&input).iter().filter(|need| need.kind == ExitKind::Outside) {
            prop_assert!(need.approver >= words, "{:?}: {:?}", line, need);
        }
    }

    /// Each need gets the names of the same links in the same order.
    #[test]
    fn link_names_are_the_listed_ones(needs in link_needs(), links in links()) {
        let facts = CallFacts { links, ..CallFacts::default() };
        prop_assert_eq!(
            link_names(needs.clone(), Some(&facts)),
            listed_link_names(needs.clone(), Some(&facts))
        );
        prop_assert_eq!(link_names(needs.clone(), None), listed_link_names(needs, None));
    }
}

/// The write exits of an `rm` of many files outside the project share the text of
/// their part: v0.0.4 copied the whole command for each file.
#[test]
fn the_exits_of_one_part_share_its_text() {
    let (locations, support, hosts) = (locations(), AutoSupport::default(), HostList::default());
    let files: Vec<String> = (1..=50).map(|at| format!("~/notes/f{at}")).collect();
    let line = format!("rm -f {}", files.join(" "));
    let paths: Vec<PathAccess> = (1..=50)
        .map(|at| PathAccess {
            path: PathBuf::from(format!("{HOME}/notes/f{at}")),
            access: Access::Write,
        })
        .collect();
    let facts = CallFacts::default();
    let exits = predict(&input(&line, &paths, Some(&facts), &locations, &support, &hosts));
    let writes: Vec<&ExitNeed> = exits.iter().filter(|need| need.kind == ExitKind::Write).collect();
    assert_eq!(writes.len(), 50, "{exits:?}");
    for need in &writes {
        assert_eq!(&*need.part, line);
        assert!(Arc::ptr_eq(&need.part, &writes[0].part), "{need:?}");
    }
}

/// Both answers of the rule, so the property above reads each of them.
#[test]
fn the_outside_approver_trusts_only_an_unchanged_file_outside_the_roots() {
    let (locations, support, hosts) = (locations(), AutoSupport::default(), HostList::default());
    let envelope = Envelope::new(&locations, Some(Path::new(PROJECT)), None);
    let line = "df -h";
    let segments = scan::scan(line);
    // The third value: a fact shows that the sandbox wrote the program, or can write it.
    for (path, changed, expected, written) in [
        ("/usr/bin/df", false, Approver::Classifier, false),
        ("/usr/bin/df", true, Approver::User, true),
        ("/home/u/p/app/bin/df", false, Approver::User, true),
    ] {
        let programs = vec![("df".to_owned(), Some(PathBuf::from(path)), changed)];
        let facts = CallFacts { programs, ..CallFacts::default() };
        let input = input(line, &[], Some(&facts), &locations, &support, &hosts);
        assert_eq!(outside_approver(&input, &envelope, &segments), expected, "{path} {changed}");
        assert_eq!(listed_outside_approver(&input, &envelope), expected, "{path} {changed}");
        let code = sandbox_code(&input, &envelope);
        assert_eq!(!code.is_empty(), written, "{path} {changed}: {code:?}");
    }
    // A missing fact shows nothing: the user approves, and efr names no code.
    let facts = CallFacts::default();
    let input = input(line, &[], Some(&facts), &locations, &support, &hosts);
    assert_eq!(outside_approver(&input, &envelope, &segments), Approver::User);
    assert_eq!(sandbox_code(&input, &envelope), Vec::<String>::new());
}

/// The facts of a machine for [`code_of`]: the programs of the system, which did not
/// change, a project with scripts that the sandbox can write, and a few files outside
/// every write root.
fn machine() -> CallFacts {
    let system = [
        "sh", "bash", "zsh", "python3", "node", "php", "awk", "gawk", "sudo", "cp", "mv", "ln",
        "install", "cat", "tee", "crontab", "sed", "make", "echo", "just", "npm", "cargo", "pip3",
    ];
    let mut programs: Vec<(String, Option<PathBuf>, bool)> = system
        .iter()
        .map(|name| ((*name).to_owned(), Some(PathBuf::from(format!("/usr/bin/{name}"))), false))
        .collect();
    for word in ["./deploy.sh", ".venv/bin/python", "./node_modules/.bin/gh"] {
        programs.push((word.to_owned(), Some(Path::new(PROJECT).join(word)), false));
    }
    programs.push(("backup".to_owned(), Some(PathBuf::from("/usr/local/bin/backup")), true));
    let file = Some(crate::TargetKind::File);
    let in_project = [
        "deploy.sh",
        "build.py",
        "project.awk",
        "x.service",
        "evil.desktop",
        "p.cron",
        "scripts/setup.sh",
        "sub/setup.sh",
        "sub/.envrc",
        "build/tool",
    ];
    let mut targets: Vec<(PathBuf, Option<crate::TargetKind>)> =
        in_project.iter().map(|name| (Path::new(PROJECT).join(name), file)).collect();
    targets.push((PathBuf::from("/usr/share/tool/run.py"), file));
    targets.push((PathBuf::from("/usr/share/applications/firefox.desktop"), file));
    targets.push((PathBuf::from("/etc/hosts"), file));
    // Files that efrd looked for and did not find.
    for missing in ["x", "setup.sh", "missing.sh"] {
        targets.push((Path::new(PROJECT).join(missing), None));
    }
    CallFacts { programs, targets, ..CallFacts::default() }
}

/// The notes of [`sandbox_code`] for `line` on [`machine`], when the line starts in the
/// project.
fn code_of(line: &str) -> Vec<String> {
    code_in(PROJECT, line)
}

/// The notes of [`sandbox_code`] for `line` on [`machine`], when the line starts in
/// `dir`.
fn code_in(dir: &str, line: &str) -> Vec<String> {
    let (locations, support, hosts) = (locations(), AutoSupport::default(), HostList::default());
    let envelope = Envelope::new(&locations, Some(Path::new(PROJECT)), None);
    let facts = machine();
    let input = ExitInput {
        command_dir: Some(Path::new(dir)),
        ..input(line, &[], Some(&facts), &locations, &support, &hosts)
    };
    sandbox_code(&input, &envelope)
}

/// A line that would run or install code that the sandbox wrote or can write gets a
/// note, whatever its shape: efr reads the facts of the files that the line names.
#[rstest::rstest]
#[case::script("./deploy.sh")]
#[case::behind_sudo("sudo ./deploy.sh")]
#[case::changed_program("sudo backup")]
#[case::interpreter_in_the_project(".venv/bin/python -c 'import app; app.deploy()'")]
#[case::tool_in_the_project("./node_modules/.bin/gh pr create")]
#[case::sh_script("sh deploy.sh")]
#[case::errexit("bash -e ./deploy.sh")]
#[case::sudo_errexit("sudo bash deploy.sh -e prod")]
#[case::script_with_an_option("python3 build.py -c release.cfg")]
#[case::script_as_inline_code("sh -c ./deploy.sh")]
#[case::source("source ./deploy.sh")]
#[case::dot(". ./deploy.sh")]
#[case::awk_file("awk -f project.awk /etc/hosts")]
#[case::awk_attached("awk -fproject.awk /etc/hosts")]
#[case::gawk_exec("gawk -E project.awk")]
#[case::long_option("gawk --file=project.awk /etc/hosts")]
#[case::unreadable_program("$PWD/deploy.sh")]
#[case::script_into_a_floor("./deploy.sh >> ~/.zshrc")]
#[case::copy("cp evil.desktop ~/.config/autostart/")]
#[case::link("ln -s ~/p/app/x.service ~/.config/systemd/user/")]
#[case::moved("mv ~/p/app/evil.desktop ~/.config/autostart/")]
#[case::installed("install -m644 x.service ~/.config/systemd/user/")]
#[case::redirect("cat x.service > ~/.config/systemd/user/x.service")]
#[case::crontab_file("crontab ./p.cron")]
#[case::script_after_cd("cd scripts && sudo ./setup.sh")]
#[case::sh_script_after_cd("cd scripts && sudo bash setup.sh")]
#[case::script_after_two_cds("cd /tmp; cd ~/p/app/scripts; ./setup.sh")]
#[case::script_after_an_unreadable_cd("cd \"$DIR\" && ./setup.sh")]
#[case::script_after_popd("popd; sudo bin/setup")]
#[case::make("sudo make install")]
#[case::make_by_path("sudo /usr/bin/make install")]
#[case::just("just deploy")]
#[case::npm_script("npm run release")]
#[case::cargo("cargo run --release")]
#[case::pip("sudo pip3 install .")]
#[case::runner_behind_env("sudo env DESTDIR=/ make install")]
#[case::script_by_home_path("sudo ~/p/app/scripts/setup.sh")]
#[case::program_that_the_line_makes("gcc x.c -o x && sudo ./x")]
#[case::script_in_shell_code("sudo sh -c './deploy.sh --prod'")]
#[case::cd_in_shell_code("bash -lc 'cd scripts && ./setup.sh'")]
#[case::runner_in_shell_code("sudo sh -c 'make install'")]
#[case::eval("eval ./deploy.sh")]
#[case::su("su -c ./deploy.sh")]
#[case::shell_code_in_shell_code("sh -c \"bash -c 'echo hi'\"")]
#[case::unreadable_shell_code("sudo bash -c \"$CMD\"")]
#[case::program_by_pattern("sudo ./scripts/set*.sh")]
#[case::program_by_braces("./scripts/{setup,x}.sh")]
#[case::script_by_pattern("sh deploy.s?")]
#[case::copy_by_pattern("cp contrib/*.service ~/.config/systemd/user/")]
#[case::script_after_popd_by_name("popd; sudo bash deploy.sh")]
#[case::unit_into_the_system("sudo cp x.service /etc/systemd/system/")]
#[case::program_into_the_system("sudo install -m755 build/tool /usr/local/bin/tool")]
#[case::tee_into_the_system("cat x.service | sudo tee /etc/systemd/system/x.service")]
#[case::unit_by_path("systemctl --user enable ./x.service")]
fn code_of_the_sandbox_gets_a_note(#[case] line: &str) {
    let notes = code_of(line);
    assert!(!notes.is_empty(), "{line}");
    assert!(
        notes.iter().all(|note| note.starts_with("the line ") || note.starts_with("efr cannot")),
        "{notes:?}"
    );
}

/// A line that runs no code of the sandbox gets none: inline code stands in the line,
/// where a reviewer reads it, and a file outside every write root is not the sandbox's.
#[rstest::rstest]
#[case::system_program("sudo sed --version")]
#[case::runner_version("sudo make --version")]
#[case::node_publish("npm publish")]
#[case::cargo_publish("cargo publish")]
#[case::system_script_after_cd("cd /usr/share/tool && python3 run.py")]
#[case::unit_by_name("systemctl --user enable x.service")]
#[case::system_file_into_the_system("sudo cp /usr/share/tool/run.py /usr/local/bin/")]
#[case::quoted_pattern("python3 -c 'print(a[0])'")]
#[case::names_beside_an_install("ls *.txt && echo hi >> ~/.zshrc")]
#[case::test_bracket("[ -f /etc/hosts ] && sudo sed --version")]
#[case::inline("sh -c 'echo hi'")]
#[case::login_shell("bash -lc 'df -h'")]
#[case::errexit_inline("sh -ec 'echo hi'")]
#[case::sudo_inline("sudo bash -lc 'systemctl restart nginx'")]
#[case::python_inline("python3 -c 'print(1)'")]
#[case::node_print("node -p '1+1'")]
#[case::node_eval("node --eval='1+1'")]
#[case::php_run("php -r 'echo 1;'")]
#[case::here_document("python3 - <<'EOF'\nprint(1)\nEOF")]
#[case::awk_program("awk '{print $1}' /etc/hosts")]
#[case::system_script("python3 /usr/share/tool/run.py")]
#[case::text_into_a_floor("echo 'alias k=kubectl' >> ~/.zshrc")]
#[case::edit_of_a_floor("sed -i 's/gaps_in = 5/gaps_in = 10/' ~/.config/hypr/hyprland.conf")]
#[case::system_file_into_a_floor("cp /usr/share/applications/firefox.desktop ~/.config/autostart/")]
#[case::no_file_of_that_name("sh missing.sh")]
fn a_line_with_no_code_of_the_sandbox_gets_no_note(#[case] line: &str) {
    assert_eq!(code_of(line), Vec::<String>::new(), "{line}");
}

/// A note names the file with the home directory as `~`, so the question shows what
/// would run.
#[test]
fn a_note_names_the_file() {
    assert_eq!(
        code_of("bash -e ./deploy.sh"),
        ["the line runs or installs ~/p/app/deploy.sh, which a sandboxed command can write"]
    );
    assert_eq!(
        code_of("sudo backup"),
        ["the line runs /usr/local/bin/backup, which changed in this turn"]
    );
    assert_eq!(code_of("$PWD/deploy.sh"), ["efr cannot read which program the line runs"]);
    assert_eq!(
        code_of("cd scripts && sudo ./setup.sh"),
        ["the line runs ~/p/app/scripts/setup.sh, which a sandboxed command can write"]
    );
    assert_eq!(
        code_of("gcc x.c -o x && sudo ./x"),
        ["the line runs ~/p/app/x, which is not there yet and which a sandboxed command can write"]
    );
    assert_eq!(
        code_of("cp contrib/*.service ~/.config/systemd/user/"),
        ["the line names files by the pattern contrib/*.service in ~/p/app/contrib, where a \
          sandboxed command can write"]
    );
    assert_eq!(
        code_of("sh -c \"bash -c 'echo hi'\""),
        ["efr cannot read which program the line runs"]
    );
}

/// The walk knows the floors of the project also from a dir below its root, and the
/// check reads the real envelope: a file that is no floor there is code of the sandbox,
/// and a real floor, which no sandboxed command can write, is none.
#[test]
fn a_floor_is_judged_by_the_real_envelope() {
    let sub = "/home/u/p/app/sub";
    assert_eq!(
        code_in(sub, "source .envrc"),
        ["the line runs or installs ~/p/app/sub/.envrc, which a sandboxed command can write"]
    );
    assert_eq!(code_in("/opt/tool", "source ~/p/app/.envrc"), Vec::<String>::new());
}

/// A line of many files gets a few notes and a count, so the question stays short.
#[test]
fn the_notes_of_a_long_line_are_capped() {
    let names: Vec<String> = (0..2_000).map(|at| format!("f{at}.sh")).collect();
    let line = format!("cp {} ~/.config/autostart/", names.join(" "));
    let (locations, support, hosts) = (locations(), AutoSupport::default(), HostList::default());
    let envelope = Envelope::new(&locations, Some(Path::new(PROJECT)), None);
    let file = Some(crate::TargetKind::File);
    let targets = names.iter().map(|name| (Path::new(PROJECT).join(name), file)).collect();
    let facts = CallFacts { targets, ..CallFacts::default() };
    let input = input(&line, &[], Some(&facts), &locations, &support, &hosts);
    let notes = sandbox_code(&input, &envelope);
    assert_eq!(notes.len(), 5, "{notes:?}");
    assert_eq!(notes[4], "the line runs or installs 1996 more such files");
}

/// A runner runs what the files of its dir say, so its note names the dir: the one
/// where the line starts, each one after a `cd`, and one that the line gives it. A
/// runner outside every write root runs no code of the sandbox.
#[test]
fn a_runner_note_names_the_dir() {
    const TOOL: &str = "/opt/tool";
    let in_project = "the line runs make in ~/p/app, where a sandboxed command can write the \
                      files that make runs";
    assert_eq!(code_of("sudo make install"), [in_project]);
    assert_eq!(code_in(TOOL, "sudo make install"), Vec::<String>::new());
    assert_eq!(code_in(TOOL, "cd /usr/src/x && sudo make install"), Vec::<String>::new());
    assert_eq!(code_in(TOOL, "cd ~/p/app && sudo make install"), [in_project]);
    assert_eq!(
        code_in(TOOL, "sudo make -C ~/p/app/sub install"),
        ["the line runs make with ~/p/app/sub, which a sandboxed command can write"]
    );
    assert_eq!(
        code_in(TOOL, "sudo cargo install --path=/home/u/p/app"),
        ["the line runs cargo with ~/p/app, which a sandboxed command can write"]
    );
    assert_eq!(
        code_in(TOOL, "cd \"$DIR\" && sudo make install"),
        ["efr cannot read where the line runs make"]
    );
    // A `cd` can fail or run in a subshell, so the dir where the line starts counts
    // too.
    assert_eq!(code_of("cd /opt/tool; sudo make install"), [in_project]);
}

/// A line with more `cd`s than efr follows runs its later commands in a dir that efr
/// does not know.
#[test]
fn a_line_of_many_cds_runs_in_an_unknown_dir() {
    let cds: String = (0..9).map(|at| format!("cd /opt/d{at}; ")).collect();
    assert_eq!(
        code_in("/opt/tool", &format!("{cds}sudo make install")),
        ["efr cannot read where the line runs make"]
    );
    assert_eq!(
        code_in("/opt/tool", &format!("{cds}sudo ./setup.sh")),
        ["efr cannot read which program the line runs"]
    );
}

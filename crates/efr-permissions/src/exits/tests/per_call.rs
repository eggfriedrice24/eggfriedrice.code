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
        programs::commands(&segment.words).into_iter().any(programs::runs_unread_code)
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
    for (path, changed, expected) in [
        ("/usr/bin/df", false, Approver::Classifier),
        ("/usr/bin/df", true, Approver::User),
        ("/home/u/p/app/bin/df", false, Approver::User),
    ] {
        let programs = vec![("df".to_owned(), Some(PathBuf::from(path)), changed)];
        let facts = CallFacts { programs, ..CallFacts::default() };
        let input = input(line, &[], Some(&facts), &locations, &support, &hosts);
        assert_eq!(outside_approver(&input, &envelope, &segments), expected, "{path} {changed}");
        assert_eq!(listed_outside_approver(&input, &envelope), expected, "{path} {changed}");
    }
}

//! The fact requests and the naming parts of long lines: the lists with seen-sets give
//! the same items in the same order as the lists that v0.0.4 scanned for each item
//! (kept here as the reference), and the first simple command that names a path is the
//! same.

use std::path::{Path, PathBuf};

use proptest::prelude::*;

use super::super::envelope::{Envelope, resolve};
use super::super::scan::{self, OPAQUE, Segment, is_assignment};
use super::super::{Naming, cover, fact_requests, git_code, part_text, persist, programs, remotes};
use crate::network::HostList;
use crate::path_class::normalize;
use crate::{AutoSupport, CoverAsk, FactRequest, Locations};
use efr_protocol::{ExitKind, ProjectId};

const HOME: &str = "/home/u";
const PROJECT: &str = "/home/u/p/app";

fn locations() -> Locations {
    let project: ProjectId = "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap();
    Locations::new(HOME).unwrap().with_project(project, PROJECT).unwrap()
}

/// [`fact_requests`] as v0.0.4 wrote it: each list scanned for each item.
fn listed_fact_requests(
    line: &str,
    command_dir: Option<&Path>,
    locations: &Locations,
) -> FactRequest {
    let start = command_dir.and_then(normalize);
    let mut request = FactRequest::default();
    // The program words in a place where only a file runs.
    let mut files: Vec<String> = Vec::new();
    let add = |list: &mut Vec<PathBuf>, path: PathBuf| {
        if !list.contains(&path) {
            list.push(path);
        }
    };
    let home = locations.home();
    // NOTE: the turn's project is not known here; the dir of the line stands in for
    // it, so a copy into an agent config there asks for its source too. A source too
    // many costs one small read.
    let envelope = Envelope::new(locations, start.as_deref(), None);
    let mut covers: Vec<CoverAsk> = Vec::new();
    for segment in scan::scan(line) {
        let dir = if segment.after_cd { None } else { start.as_deref() };
        let at = |word: &str| resolve(word, dir, locations);
        // NOTE: a node of the sandbox's own `/dev` is no exit, whatever the host has,
        // and a path through one leads where the daemon cannot look.
        let written_at = |word: &str| {
            at(word).filter(|path| {
                !programs::is_sandbox_device(path) && !programs::through_open_file(path)
            })
        };
        let assigned = segment.words.iter().any(|word| is_assignment(word));
        let bare = match segment.words.as_slice() {
            [] => true,
            [only] => only == ":" || only == "true",
            _ => false,
        };
        for redirect in &segment.redirects {
            if let Some(path) = written_at(&redirect.target) {
                if bare && !redirect.append {
                    covers.extend(cover::path_asks(&[Some(path.clone())]));
                }
                covers.extend(cover::write_asks(&path, home));
                listed_real_paths(&mut request.real_paths, &path, home);
                add(&mut request.targets, path);
            }
        }
        for (command, by_shell) in programs::placed_commands(&segment.words) {
            let destroys =
                programs::classify(command, &AutoSupport::default(), &HostList::default(), &[])
                    .iter()
                    .any(|found| found.kind == ExitKind::Destructive);
            if destroys {
                covers.extend(cover::program_asks(command, assigned, dir, locations, &at));
            }
            covers.extend(persist::covers(command));
            if let Some(git_code::Trigger::Ask(ask)) = git_code::trigger(command, dir, locations)
                && !request.git_code.contains(&ask)
            {
                request.git_code.push(ask);
            }
            if remotes::remote_command(command).is_some()
                && !remotes::steers_git(&segment.words)
                && let Some(git_dir) = remotes::remote_dir(command, dir, locations)
            {
                add(&mut request.remotes, git_dir);
            }
            for source in persist::sources(command, &envelope, &at) {
                if !request.sources.contains(&source) {
                    request.sources.push(source);
                }
            }
            if let Some(program) = command.first()
                && !program.contains([OPAQUE, '$'])
            {
                if !request.programs.contains(program) {
                    request.programs.push(program.clone());
                }
                if by_shell && programs::is_shell_word(program) {
                    if !request.builtins.contains(program) {
                        request.builtins.push(program.clone());
                    }
                } else if !files.contains(program) {
                    files.push(program.clone());
                }
            }
            let written = programs::made_paths(command)
                .into_iter()
                .map(|(word, _)| word)
                .chain(programs::dd_targets(command))
                .chain(programs::sed_in_place_targets(command));
            for word in written {
                if let Some(path) = written_at(word) {
                    covers.extend(cover::write_asks(&path, home));
                    listed_real_paths(&mut request.real_paths, &path, home);
                    add(&mut request.targets, path);
                }
            }
            let operands = programs::recursive_rm_operands(command);
            let removed: Vec<Option<PathBuf>> = operands.iter().map(|word| at(word)).collect();
            covers.extend(cover::path_asks(&removed));
            for path in removed.into_iter().flatten() {
                covers.extend(cover::write_asks(&path, home));
                add(&mut request.targets, path.clone());
                add(&mut request.tracked, path);
            }
        }
    }
    for ask in covers {
        if !request.covers.contains(&ask) {
            request.covers.push(ask);
        }
    }
    // NOTE: a word that runs as a file in one place of the line resolves as a file, so
    // the question shows that file and whether the sandbox wrote it.
    request.builtins.retain(|word| !files.contains(word));
    request
}

/// The `add_real_paths` of v0.0.4: the list scanned for each parent.
fn listed_real_paths(list: &mut Vec<PathBuf>, path: &Path, home: &Path) {
    for dir in path.ancestors().take_while(|dir| dir.starts_with(home) && *dir != home) {
        if !list.iter().any(|known| known == dir) {
            list.push(dir.to_path_buf());
        }
    }
}

/// The simple command that first names `path`, as v0.0.4 found it for each path.
fn listed_naming_part(
    segments: &[Segment],
    path: &Path,
    start: Option<&Path>,
    locations: &Locations,
) -> Option<String> {
    segments
        .iter()
        .find(|segment| {
            let dir = if segment.after_cd { None } else { start };
            let names = |word: &str| resolve(word, dir, locations).as_deref() == Some(path);
            segment.words.iter().any(|word| names(word))
                || segment.redirects.iter().any(|redirect| names(&redirect.target))
        })
        .map(part_text)
}

/// A random line of commands that name paths, with repeats, wrappers, redirections,
/// substitutions and `cd`s.
fn line() -> impl Strategy<Value = String> {
    let word = prop::sample::select(vec![
        "rm",
        "-rf",
        "-f",
        "touch",
        "mkdir",
        "-p",
        "tee",
        "-a",
        "sed",
        "-i",
        "s/a/b/",
        "dd",
        "of=out",
        "cp",
        "ln",
        "-s",
        "git",
        "push",
        "fetch",
        "-C",
        "sub",
        "env",
        "X=1",
        "sudo",
        "nice",
        "xargs",
        "cd",
        "..",
        "~/x",
        "~/.zshrc",
        "/tmp/t",
        "a",
        "b",
        "a/b",
        "./a",
        "$(",
        ")",
        ">",
        ">>",
        "2>",
        "f1",
        "f2",
        "f1",
        ";",
        "&&",
        "|",
        "\"$X\"",
        "systemctl",
        "--user",
        "enable",
        "u.service",
        "crontab",
        "c.txt",
        ":",
        "printf",
        "echo",
        "time",
        "cargo",
        "~/.config/systemd/user/x.service",
    ]);
    prop::collection::vec(word, 1..40).prop_map(|words| words.join(" "))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    /// Each list of the request holds the same items in the same order.
    #[test]
    fn fact_requests_with_seen_sets_are_the_listed_ones(line in line(), cd in any::<bool>()) {
        let locations = locations();
        let dir = cd.then(|| Path::new(PROJECT));
        let request = fact_requests(&line, dir, &locations);
        prop_assert_eq!(&request, &listed_fact_requests(&line, dir, &locations), "{:?}", line);
        let needs = efr_protocol::Needs {
            write: vec!["~/x".to_owned(), "a/b".to_owned(), "~/x".to_owned()],
            sockets: vec!["~/x".to_owned(), "/run/user/1/s".to_owned()],
            ..efr_protocol::Needs::default()
        };
        let listed = listed_with_needs(request.clone(), &needs, dir, &locations);
        prop_assert_eq!(request.with_needs(Some(&needs), dir, &locations), listed, "{:?}", line);
    }

    /// Each path of the line is named by the same simple command.
    #[test]
    fn naming_parts_are_the_listed_ones(line in line(), cd in any::<bool>()) {
        let locations = locations();
        let start = cd.then(|| Path::new(PROJECT));
        let segments = scan::scan(&line);
        let mut naming = Naming::new(&segments, start, &locations);
        let words = segments.iter().flat_map(|segment| segment.words.iter());
        for word in words.chain(["~/missing".to_owned()].iter()) {
            for dir in [start, None] {
                let Some(path) = resolve(word, dir, &locations) else { continue };
                prop_assert_eq!(
                    naming.part(&path).map(|part| part.to_string()),
                    listed_naming_part(&segments, &path, start, &locations),
                    "{:?} in {:?}", path, line
                );
            }
        }
    }
}

/// `FactRequest::with_needs` as v0.0.4 wrote it, with the sockets of v0.0.5 (OQS17).
fn listed_with_needs(
    mut request: FactRequest,
    needs: &efr_protocol::Needs,
    command_dir: Option<&Path>,
    locations: &Locations,
) -> FactRequest {
    let start = command_dir.and_then(normalize);
    let add_covers = |request: &mut FactRequest, asks: Vec<CoverAsk>| {
        for ask in asks {
            if !request.covers.contains(&ask) {
                request.covers.push(ask);
            }
        }
    };
    for word in needs.write.iter().take(efr_protocol::Needs::MAX_WRITE) {
        if let Some(path) = resolve(word, start.as_deref(), locations) {
            add_covers(&mut request, cover::write_asks(&path, locations.home()));
            listed_real_paths(&mut request.real_paths, &path, locations.home());
            if !request.targets.contains(&path) {
                request.targets.push(path);
            }
        }
    }
    for word in needs.sockets.iter().take(efr_protocol::Needs::MAX_SOCKETS) {
        let Some(path) = resolve(word, start.as_deref(), locations) else { continue };
        if !request.sockets.contains(&path) {
            request.sockets.push(path.clone());
        }
        if !request.real_paths.contains(&path) {
            request.real_paths.push(path);
        }
    }
    request
}

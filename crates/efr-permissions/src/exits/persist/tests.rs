use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::{installs, literal_pipe, unit_operands};

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_owned).collect()
}

#[test]
fn a_literal_pipe_is_one_echo_or_printf_into_crontab_stdin() {
    assert!(literal_pipe("echo '* * * * * x' | crontab -"));
    assert!(literal_pipe("printf '%s\\n' '@reboot x' | crontab -"));
    for line in [
        "echo x || crontab -",
        "echo x | crontab - && rm y",
        "echo $X | crontab -",
        "echo \"$(cat jobs)\" | crontab -",
        "echo `cat jobs` | crontab -",
        "cat jobs | crontab -",
        "echo x | crontab -e",
        "echo x > y | crontab -",
        "echo x |& crontab -",
        "crontab - <<EOF",
    ] {
        assert!(!literal_pipe(line), "{line}");
    }
}

#[test]
fn unit_operands_take_plain_names_and_plain_options_only() {
    let operands =
        |line: &str| unit_operands(&words(line)).map(|(verb, units)| (verb.to_owned(), units));
    assert_eq!(
        operands("--user enable --now agent"),
        Some(("enable".to_owned(), vec!["agent.service".to_owned()]))
    );
    assert_eq!(
        operands("--user add-wants default.target agent.timer"),
        Some(("add-wants".to_owned(), vec!["default.target".to_owned(), "agent.timer".to_owned()]))
    );
    for line in [
        "enable agent.service",
        "--user --global enable agent.service",
        "--user enable ~/x/agent.service",
        "--user enable agent@x.service",
        "--user enable ../agent.service",
        "--user enable agent.conf",
        "--user add-wants default.target",
        "--user link agent.service",
        "--user enable",
    ] {
        assert_eq!(operands(line), None, "{line}");
    }
}

#[test]
fn fact_requests_ask_for_the_copies_and_the_sources_of_named_persistence() {
    use crate::{CoverAsk, Locations, Source};

    let locations = Locations::new("/home/u").unwrap();
    let dir = Some(std::path::Path::new("/home/u/p/app"));
    let request = |line: &str| super::super::fact_requests(line, dir, &locations);

    let enable = request("systemctl --user enable --now agent");
    assert!(enable.covers.contains(&CoverAsk::UnitLinks("agent.service".to_owned())));
    assert_eq!(enable.sources, [Source::Unit("agent.service".to_owned())]);
    let cron = request("crontab ~/jobs.txt");
    assert!(cron.covers.contains(&CoverAsk::CronTable));
    assert_eq!(cron.sources, [Source::File(PathBuf::from("/home/u/jobs.txt"))]);
    assert!(request("loginctl enable-linger").covers.contains(&CoverAsk::Linger));
    let link = request("ln -sf ~/evil ~/.config/systemd/user/default.target.wants/x.service");
    assert_eq!(link.sources, [Source::File(PathBuf::from("/home/u/evil"))]);
    // An agent config in the dir of the line is a floor too.
    let agent = request("cp ~/mcp.json .mcp.json");
    assert_eq!(agent.sources, [Source::File(PathBuf::from("/home/u/mcp.json"))]);
    // A copy outside every floor reads no source, and a secret is never one.
    assert_eq!(request("cp ~/a ~/Documents/b").sources, []);
    assert_eq!(request("cp ~/.ssh/id_ed25519 ~/.config/autostart/").sources, []);
    assert_eq!(request("crontab -e").covers, []);
}

#[test]
fn installs_name_the_destination_and_each_source() {
    let at = |word: &str| Some(PathBuf::from("/home/u").join(word));
    assert_eq!(
        installs(&words("cp -f a b dest"), &at),
        [(
            PathBuf::from("/home/u/dest"),
            vec![PathBuf::from("/home/u/a"), PathBuf::from("/home/u/b")]
        )]
    );
    assert_eq!(installs(&words("ln -s a"), &at), []);
    assert_eq!(installs(&words("cp -t dest a"), &at), []);
    assert_eq!(installs(&words("mv a dest"), &at), []);
}

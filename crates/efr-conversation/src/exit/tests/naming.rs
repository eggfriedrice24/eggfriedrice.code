//! The naming rule under efr's proxy (phase D2, DP23): when the allow list, a project's
//! git remote or a refusal of the proxy counts for a host, and when it never does. And
//! the window of the naming rule (v0.0.4, DV30): which user messages name a target, a
//! host or an upload, while the reviewer's record keeps every message.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_permissions::{
    Approver, AutoSupport, CallFacts, Decision, Egress, Engine, HostList, Locations, Preset,
    RemoteFact, RemoteUrl, Requirements,
};
use efr_protocol::{
    AttachHow, CommandId, Event, EventEnvelope, ExitRecord, Grant, HostFact, Judgement, Needs,
    Origin, Risk, RouteScope, Scope, Seq, TurnId, TurnSettings, UserAuthorization, Verdict,
};
use efr_stdx::id::uuid_v7;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;

use super::super::{
    Action, TurnExits, UserMessages, effective, fetch_asked, grant, naming_messages, package_fetch,
    route, user_messages, wake_naming_messages,
};
use super::{APP, HOME, SCRATCH, app, decide, envelope, record, record_with, shell};

/// The default allow list of spec 8.3.
fn default_list() -> HostList {
    let allow: Vec<String> = Preset::ALL.iter().map(|preset| preset.name().to_owned()).collect();
    HostList::compile(&allow, &[], &[], &[]).unwrap()
}

/// The machine with `egress` and the default list.
fn engine(egress: Egress) -> Engine {
    let locations = Locations::new(HOME).unwrap().with_project(app(), APP).unwrap();
    Engine::with_defaults(locations)
        .with_support(AutoSupport { egress, ..AutoSupport::default() })
        .with_hosts(Arc::new(default_list()))
}

/// The record of `requirements` on `engine`, with the user messages `said`.
fn record_on(
    engine: &Engine,
    requirements: &Requirements,
    said: &[&str],
) -> (Decision, ExitRecord) {
    let decision = decide(engine, requirements.clone());
    let launch = grant(&decision);
    let action = Action {
        tool: "shell",
        decision: &decision,
        requirements,
        launch: &launch,
        cwd: Path::new(APP),
        scope: &Scope::Project(app()),
        scratch: Path::new(SCRATCH),
        engine,
        payload: None,
    };
    let said: Vec<String> = said.iter().map(|text| (*text).to_owned()).collect();
    let record = record(action, &said, &TurnExits::default());
    (decision, record)
}

fn host_fact<'a>(record: &'a ExitRecord, host: &str) -> &'a HostFact {
    record
        .facts
        .hosts
        .iter()
        .find(|fact| fact.host == host)
        .unwrap_or_else(|| panic!("no fact for {host}: {:?}", record.facts.hosts))
}

fn allow() -> Judgement {
    Judgement {
        verdict: Verdict::Allow,
        risk: Risk::Low,
        user_authorization: UserAuthorization::High,
        category: "fits the request".to_owned(),
        rationale: "the user asked for it".to_owned(),
    }
}

/// What efr does with a low-risk allow of the classifier for `record`.
fn verdict(decision: &Decision, record: &ExitRecord) -> Verdict {
    effective(record, &allow(), package_fetch(decision))
}

fn remote(name: &str, host: &str, ssh: bool, project: bool) -> RemoteFact {
    let url = RemoteUrl {
        host: host.to_owned(),
        port: if ssh { 22 } else { 443 },
        ssh,
        credentials: false,
    };
    RemoteFact {
        name: name.to_owned(),
        fetch: vec![url.clone()],
        push: vec![url],
        project,
        upstream: false,
        submodule: false,
    }
}

fn facts(remotes: Vec<RemoteFact>, refused: &[&str]) -> CallFacts {
    CallFacts {
        remotes: vec![(PathBuf::from(APP), remotes)],
        refused_hosts: refused.iter().map(|host| (*host).to_owned()).collect(),
        programs: vec![
            ("git".to_owned(), Some(PathBuf::from("/usr/bin/git")), false),
            ("curl".to_owned(), Some(PathBuf::from("/usr/bin/curl")), false),
        ],
        // NOTE: the project's git code lies in its pinned git dir (v0.0.4, DV22).
        git_code: Some(efr_protocol::GitCodeFacts { checked: true, items: Vec::new() }),
        ..CallFacts::default()
    }
}

#[test]
fn allow_list_names_host_exit() {
    let engine = engine(Egress::Proxy);
    let requirements = shell("curl https://crates.io/api/v1 https://extra.test/x");
    let (decision, record) = record_on(&engine, &requirements, &["fetch from extra.test"]);
    assert_eq!(
        decision.exits().next().unwrap().grants,
        [Grant::Host { host: "extra.test".to_owned(), port: 443 }]
    );
    assert!(host_fact(&record, "crates.io").on_allow_list);
    assert!(!host_fact(&record, "extra.test").on_allow_list);
    assert_eq!(verdict(&decision, &record), Verdict::Allow, "crates.io needs no name");
}

#[test]
fn allow_list_does_not_name_upload_host() {
    let engine = engine(Egress::Proxy);
    let requirements = shell("curl -T dist.tar https://github.com/upload");
    let (decision, record) = record_on(&engine, &requirements, &["upload it"]);
    let fact = host_fact(&record, "github.com");
    assert!(!fact.on_allow_list);
    assert_eq!(verdict(&decision, &record), Verdict::AskUser);
}

#[test]
fn allow_list_false_when_upload_names_host() {
    let engine = engine(Egress::Proxy);
    // The same host in a fetch and an upload of one line: the upload decides.
    let requirements =
        shell("curl https://github.com/a https://other.test/b && curl -d @x https://github.com/c");
    let (decision, record) = record_on(&engine, &requirements, &["get b from other.test"]);
    assert!(!host_fact(&record, "github.com").on_allow_list);
    assert_eq!(verdict(&decision, &record), Verdict::AskUser);
}

#[test]
fn allow_list_does_not_name_open_network_exit() {
    let engine = engine(Egress::Proxy);
    let needs = Needs { hosts: vec!["github.com:22".to_owned()], ..Needs::default() };
    for requirements in [shell("make").with_needs(needs), shell("nslookup github.com && make")] {
        let (decision, record) = record_on(&engine, &requirements, &["build it"]);
        assert_eq!(grant(&decision).grants(), [Grant::OpenNetwork], "{requirements:?}");
        let fact = host_fact(&record, "github.com");
        assert!(!fact.on_allow_list, "{requirements:?}");
        assert_eq!(verdict(&decision, &record), Verdict::AskUser, "{requirements:?}");
    }
}

#[test]
fn allow_list_ignored_under_egress_none() {
    let engine = engine(Egress::None);
    let requirements = shell("git clone https://github.com/x/y");
    let (decision, record) = record_on(&engine, &requirements, &["clone the repo"]);
    assert!(!host_fact(&record, "github.com").on_allow_list);
    assert_eq!(verdict(&decision, &record), Verdict::AskUser, "the name is still needed");
}

#[test]
fn allow_list_needs_the_exit_port() {
    let engine = engine(Egress::Proxy);
    // github.com is listed on 443 only; the exit's grant is on port 80.
    let requirements = shell("curl http://github.com/x http://plain.test/y");
    let (decision, record) = record_on(&engine, &requirements, &["get y from plain.test"]);
    assert!(!host_fact(&record, "github.com").on_allow_list);
    assert_eq!(verdict(&decision, &record), Verdict::AskUser);
}

#[test]
fn project_remote_counts_as_named() {
    let engine = engine(Egress::Proxy);
    let facts = facts(vec![remote("origin", "git.work.test", true, true)], &[]);
    let requirements = shell("git fetch").with_facts(facts);
    let (decision, record) = record_on(&engine, &requirements, &["fetch the latest"]);
    let fact = host_fact(&record, "git.work.test");
    assert!(fact.project_remote && !fact.named_in_user_messages);
    assert_eq!(verdict(&decision, &record), Verdict::Allow);
}

#[test]
fn project_remote_push_still_needs_the_verb() {
    let engine = engine(Egress::Proxy);
    let facts = facts(vec![remote("origin", "github.com", false, true)], &[]);
    let requirements = shell("git push").with_facts(facts);
    let (decision, record) = record_on(&engine, &requirements, &["fix the typo"]);
    assert!(host_fact(&record, "github.com").project_remote);
    // The host counts as named, but the route needs "push" in a user message.
    let said = ["fix the typo".to_owned()];
    assert_eq!(route(&decision, &said, None), Some(Approver::User));
    let said = ["fix the typo and push".to_owned()];
    assert_eq!(route(&decision, &said, None), Some(Approver::Classifier));
}

#[test]
fn remote_changed_since_first_call_needs_name() {
    // efrd sets `project` only while the URLs equal those of the first call.
    let engine = engine(Egress::Proxy);
    let facts = facts(vec![remote("origin", "git.work.test", true, false)], &[]);
    let requirements = shell("git fetch").with_facts(facts);
    let (decision, record) = record_on(&engine, &requirements, &["fetch the latest"]);
    assert!(!host_fact(&record, "git.work.test").project_remote);
    assert_eq!(verdict(&decision, &record), Verdict::AskUser);
    assert_eq!(route(&decision, &["fetch".to_owned()], None), Some(Approver::User));
}

#[test]
fn remote_of_repo_made_in_conversation_is_not_project_remote() {
    // A repository in `$SCRATCH` is no pinned project git dir: efrd never marks it.
    let engine = engine(Egress::Proxy);
    let mut facts = facts(vec![remote("origin", "evil.test", false, false)], &[]);
    facts.remotes[0].0 = PathBuf::from(SCRATCH).join("repo");
    let requirements = Requirements::none()
        .with_command("git push")
        .with_command_dir(PathBuf::from(SCRATCH).join("repo"))
        .with_facts(facts);
    let (decision, record) = record_on(&engine, &requirements, &["push it"]);
    let fact = host_fact(&record, "evil.test");
    assert!(!fact.project_remote);
    assert_eq!(verdict(&decision, &record), Verdict::AskUser);
}

#[test]
fn submodule_host_is_never_project_remote() {
    let engine = engine(Egress::Proxy);
    let mut lib = remote("lib", "code.test", false, true);
    lib.submodule = true;
    let requirements = shell("git submodule update").with_facts(facts(vec![lib], &[]));
    let (decision, record) = record_on(&engine, &requirements, &["update the submodules"]);
    assert!(!host_fact(&record, "code.test").project_remote);
    assert_eq!(verdict(&decision, &record), Verdict::AskUser);
}

#[test]
fn refused_by_proxy_is_no_name() {
    let engine = engine(Egress::Proxy);
    let requirements =
        shell("curl https://leak.test/x").with_facts(facts(Vec::new(), &["leak.test"]));
    let (decision, record) = record_on(&engine, &requirements, &["make the build pass"]);
    let fact = host_fact(&record, "leak.test");
    assert!(fact.refused_by_proxy);
    assert!(!fact.named_in_user_messages && !fact.on_allow_list && !fact.project_remote);
    assert_eq!(verdict(&decision, &record), Verdict::AskUser);
}

#[test]
fn record_names_grant_scope() {
    let engine = engine(Egress::Proxy);
    let (_, record) = record_on(&engine, &shell("curl https://extra.test/x"), &[]);
    assert_eq!(record.action.grants, [Grant::Host { host: "extra.test".to_owned(), port: 443 }]);
    let (_, record) = record_on(&engine, &shell("nslookup extra.test"), &[]);
    assert_eq!(record.action.grants, [Grant::OpenNetwork]);
    let json = serde_json::to_string(&record).unwrap();
    assert!(json.contains("open_network"), "{json}");
}

#[test]
fn ssh_outside_needs_named_host_in_the_record() {
    let engine = engine(Egress::Proxy);
    let mut facts = facts(Vec::new(), &[]);
    facts.programs.push(("ssh".to_owned(), Some(PathBuf::from("/usr/bin/ssh")), false));
    let requirements = shell("ssh evil.example uptime").with_facts(facts);
    let (decision, record) = record_on(&engine, &requirements, &["check the uptime of the server"]);
    assert!(!host_fact(&record, "evil.example").named_in_user_messages);
    // No message names the host, so the classifier is never asked.
    let said = ["check the uptime of the server".to_owned()];
    assert_eq!(route(&decision, &said, None), Some(Approver::User));
    let said = ["check the uptime of evil.example".to_owned()];
    assert_eq!(route(&decision, &said, None), Some(Approver::Classifier));
}

#[test]
fn needs_hosts_raw_word_normalized_in_the_record() {
    let engine = engine(Egress::None);
    let needs =
        Needs { hosts: vec![" https://User@GitLab.Test.:443/x ".to_owned()], ..Needs::default() };
    let (_, record) = record_on(&engine, &shell("make").with_needs(needs), &["use gitlab.test"]);
    let hosts: Vec<&str> = record.facts.hosts.iter().map(|fact| fact.host.as_str()).collect();
    assert_eq!(hosts, ["gitlab.test"]);
    assert!(record.facts.hosts[0].named_in_user_messages);
}

/// The id of the turn `n` of a test conversation.
fn turn(n: u64) -> TurnId {
    TurnId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(n)))
}

/// `event` at the sequence number `seq`.
fn at(seq: u64, event: Event) -> EventEnvelope {
    EventEnvelope { seq: Seq::new(seq), ..envelope(event) }
}

/// The prompt `text` of `turn_id`, at `seq`.
fn prompt(seq: u64, turn_id: TurnId, text: &str) -> EventEnvelope {
    at(
        seq,
        Event::PromptQueued {
            turn_id,
            command_id: CommandId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(seq + 100))),
            text: text.to_owned(),
            origin: Origin::Shell,
            context: None,
            settings: TurnSettings::default(),
            steers: Vec::new(),
        },
    )
}

/// The steer `text` into `turn_id`, at `seq`.
fn steer(seq: u64, turn_id: TurnId, text: &str) -> EventEnvelope {
    at(seq, Event::TurnSteered { turn_id, text: text.to_owned() })
}

/// A pick-up of the conversation by another terminal, at `seq`.
fn attached(seq: u64) -> EventEnvelope {
    at(
        seq,
        Event::TerminalAttached {
            tty: "/dev/pts/6".to_owned(),
            route: RouteScope::Outside,
            shell_pid: None,
            how: AttachHow::PickedUp,
        },
    )
}

/// Who approves `git push origin main` after the user messages `said`.
fn push_route(said: &[String]) -> Option<Approver> {
    route(&decide(&super::engine(), shell("git push origin main")), said, None)
}

#[test]
fn naming_rule_ignores_a_name_from_an_older_turn() {
    let current = turn(4);
    let page = vec![
        prompt(1, turn(1), "commit and push"),
        prompt(2, turn(2), "fix the tests"),
        prompt(3, turn(3), "and the docs"),
        prompt(4, current, "now do it"),
    ];

    let naming = naming_messages(&page, current);

    assert_eq!(naming, ["and the docs", "now do it"]);
    assert_eq!(push_route(&naming), Some(Approver::User), "the push of turn 1 is too old");
    // The whole conversation still names the push: that is what the reviewer reads.
    assert_eq!(push_route(&user_messages(&page, current)), Some(Approver::Classifier));
}

#[test]
fn naming_rule_reads_the_turn_before() {
    let (first, withdrawn, current) = (turn(1), turn(2), turn(3));
    let page = vec![
        prompt(1, first, "commit and push"),
        steer(2, first, "only the parser"),
        prompt(3, withdrawn, "never mind"),
        at(4, Event::PromptWithdrawn { turn_id: withdrawn, origin: Origin::Shell }),
        prompt(5, current, "now do it"),
        prompt(6, turn(4), "and tag it"),
    ];

    let naming = naming_messages(&page, current);

    // A withdrawn prompt is no turn, and a prompt queued behind is no earlier request.
    assert_eq!(naming, ["commit and push", "only the parser", "now do it"]);
    assert_eq!(push_route(&naming), Some(Approver::Classifier));
}

#[test]
fn naming_rule_stops_at_terminal_attached() {
    let current = turn(2);
    let picked_up =
        vec![prompt(1, turn(1), "commit and push"), attached(2), prompt(3, current, "now do it")];

    let naming = naming_messages(&picked_up, current);

    assert_eq!(naming, ["now do it"], "only the turn after the pick-up counts");
    assert_eq!(push_route(&naming), Some(Approver::User));
    // A turn after the pick-up, before the current one, counts.
    let current = turn(3);
    let later = vec![
        prompt(1, turn(1), "fix the tests"),
        attached(2),
        prompt(3, turn(2), "commit and push"),
        prompt(4, current, "now do it"),
    ];
    assert_eq!(naming_messages(&later, current), ["commit and push", "now do it"]);
}

#[test]
fn naming_rule_for_a_wake_turn() {
    let behind = turn(9);
    let waiting = HashSet::from([behind]);
    let long = vec![
        prompt(1, turn(1), "commit and push"),
        prompt(2, turn(2), "run the server"),
        prompt(3, turn(3), "and watch the logs"),
        prompt(4, behind, "push it"),
    ];

    let naming = wake_naming_messages(&long, &waiting);

    // The user's newest turn before the wake turn and the turn before that, without the
    // prompt that queued behind the wake turn.
    assert_eq!(naming, ["run the server", "and watch the logs"]);
    assert_eq!(push_route(&naming), Some(Approver::User));
    let short = vec![prompt(1, turn(1), "commit and push"), prompt(2, turn(2), "run the server")];
    let naming = wake_naming_messages(&short, &HashSet::new());
    assert_eq!(naming, ["commit and push", "run the server"]);
    assert_eq!(push_route(&naming), Some(Approver::Classifier));
    // After a pick-up and no prompt since, a wake turn reads no message at all.
    let picked_up = vec![prompt(1, turn(1), "commit and push"), attached(2)];
    assert!(wake_naming_messages(&picked_up, &HashSet::new()).is_empty());
}

#[test]
fn reviewer_record_keeps_every_message() {
    let engine = engine(Egress::Proxy);
    let requirements = shell("curl https://extra.test/x");
    let decision = decide(&engine, requirements.clone());
    let launch = grant(&decision);
    let action = Action {
        tool: "shell",
        decision: &decision,
        requirements: &requirements,
        launch: &launch,
        cwd: Path::new(APP),
        scope: &Scope::Project(app()),
        scratch: Path::new(SCRATCH),
        engine: &engine,
        payload: None,
    };
    let all: Vec<String> =
        ["fetch from extra.test", "fix the tests", "and the docs", "now do it again"]
            .map(str::to_owned)
            .to_vec();
    let messages = UserMessages { all: all.clone(), naming: all[2..].to_vec() };

    let record = record_with(action, &messages, &TurnExits::default());

    assert_eq!(record.user_messages, all, "the reviewer reads the whole request");
    assert!(!host_fact(&record, "extra.test").named_in_user_messages, "the name is too old");
    assert_eq!(verdict(&decision, &record), Verdict::AskUser);
}

#[test]
fn registry_exception_reads_the_naming_window() {
    let engine = engine(Egress::None);
    let needs = Needs { hosts: vec!["index.crates.io".to_owned()], ..Needs::default() };
    let decision = decide(&engine, shell("cargo build").with_needs(needs));
    assert_eq!(package_fetch(&decision), Some(efr_permissions::exits::Ecosystem::Crates));
    // The window holds a build request: the registries need no name.
    let asked = ["fix the tests".to_owned(), "and build it".to_owned()];
    assert_eq!(fetch_asked(&decision, &asked), package_fetch(&decision));
    // The build request is older than the window: the host needs its name.
    let other = ["look at the code".to_owned()];
    assert_eq!(fetch_asked(&decision, &other), None);
}

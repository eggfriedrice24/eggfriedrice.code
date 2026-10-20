use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use efr_protocol::{
    CommandId, ConversationId, ConversationSubscribe, Method, Origin, ProjectId, PromptSend,
    PromptSendResult, RouteScope, Seq, ShellContext, Switch, TurnSettings,
};
use efr_test_support::{TestClock, TestDirs};
use efr_transport::ConnId;
use pretty_assertions::assert_eq;

use super::{Routes, Slot, load_slots};
use crate::connections::{Connections, HelloInfo};
use crate::proc_view::ProcView;
use crate::testing::{self, RawClient};
use crate::{Deps, Settings};

fn conversation(n: u128) -> ConversationId {
    ConversationId::from_uuid(uuid::Uuid::from_u128(n))
}

fn project(n: u128) -> ProjectId {
    ProjectId::from_uuid(uuid::Uuid::from_u128(0x0900 + n))
}

fn app() -> RouteScope {
    RouteScope::Project(project(1))
}

fn slot(n: u128, shell_pid: Option<u32>, seq: u64) -> Slot {
    Slot { conversation_id: conversation(n), shell_pid, routed_seq: Seq::new(seq) }
}

fn routes(entries: &[(&str, RouteScope, Slot)]) -> Routes {
    let mut routes = Routes::default();
    for (tty, key, slot) in entries {
        routes.set_slot(tty, key.clone(), slot.conversation_id, slot.shell_pid, slot.routed_seq);
    }
    routes
}

#[test]
fn claim_twice_gives_one_winner() {
    let mut routes = routes(&[("pts-1", app(), slot(1, Some(10), 5))]);
    let alive = |_| true;
    // pts-1's shell ended; two new terminals in the project send at the same moment.
    let dead = |pid| pid != 10;

    assert!(routes.claim("pts-2", &app(), conversation(1), Some(20), None, &dead));
    assert!(!routes.claim("pts-3", &app(), conversation(1), Some(30), None, &alive));
    assert_eq!(routes.slot("pts-3", &app()), None, "the loser's slot stays empty");
    assert_eq!(routes.slot("pts-2", &app()).map(|slot| slot.shell_pid), Some(Some(20)));
}

#[test]
fn a_claim_needs_the_slot_as_it_was_read() {
    let mut routes = routes(&[("pts-2", app(), slot(2, Some(20), 5))]);
    let alive = |_| true;

    assert!(!routes.claim("pts-2", &app(), conversation(1), Some(20), None, &alive));
    assert!(routes.claim(
        "pts-2",
        &app(),
        conversation(1),
        Some(20),
        Some(conversation(2)),
        &alive
    ));
}

#[test]
fn pick_up_counts_an_unknown_pid_as_held() {
    let mut routes = routes(&[("pts-1", app(), slot(1, None, 5))]);
    let never_called = |_| panic!("an unknown pid needs no look");

    assert!(!routes.claim("pts-2", &app(), conversation(1), Some(20), None, &never_called));
}

#[test]
fn claim_removes_the_old_slot() {
    let mut routes = routes(&[
        ("pts-1", app(), slot(1, Some(10), 5)),
        ("pts-1", RouteScope::Outside, slot(3, Some(10), 6)),
    ]);
    let dead = |pid| pid != 10;
    assert!(routes.claim("pts-2", &app(), conversation(1), Some(20), None, &dead));
    assert_eq!(routes.holders(conversation(1)).len(), 2, "both until the prompt commits");

    routes.attached("pts-2", &app(), conversation(1), Seq::new(9), Some("fix it".to_owned()));

    let holders = routes.holders(conversation(1));
    assert_eq!(holders.len(), 1);
    assert_eq!((holders[0].0.as_str(), holders[0].2.routed_seq), ("pts-2", Seq::new(9)));
    assert_eq!(routes.slot("pts-1", &RouteScope::Outside), Some(slot(3, Some(10), 6)));
    assert_eq!(
        routes.take_moved("pts-1", &app()),
        Some(("pts-2".to_owned(), Some("fix it".to_owned())))
    );
}

#[test]
fn a_released_claim_gives_the_slot_back() {
    let mut routes = routes(&[("pts-2", app(), slot(2, Some(20), 5))]);
    let alive = |_| true;
    let before = routes.slot("pts-2", &app());
    assert!(routes.claim(
        "pts-2",
        &app(),
        conversation(1),
        Some(20),
        Some(conversation(2)),
        &alive
    ));

    routes.release_claim("pts-2", &app(), before);

    assert_eq!(routes.slot("pts-2", &app()), before);
    let mut fresh = Routes::default();
    assert!(fresh.claim("pts-4", &app(), conversation(1), Some(40), None, &alive));
    fresh.release_claim("pts-4", &app(), None);
    assert!(fresh.terminals.is_empty());
}

#[test]
fn the_current_slot_is_the_newest_routed_and_a_prompt_moves_it() {
    let mut routes = routes(&[
        ("pts-1", app(), slot(1, None, 5)),
        ("pts-1", RouteScope::Outside, slot(2, Some(10), 7)),
    ]);
    assert_eq!(routes.current("pts-1").map(|(key, _)| key), Some(RouteScope::Outside));

    routes.routed("pts-1", conversation(1), Some(11), Seq::new(8));
    routes.routed("pts-1", conversation(1), Some(12), Seq::new(9));

    assert_eq!(routes.current("pts-1"), Some((app(), slot(1, Some(11), 9))));
    routes.routed("pts-9", conversation(2), Some(99), Seq::new(10));
    assert_eq!(routes.slot("pts-1", &RouteScope::Outside), Some(slot(2, Some(10), 7)));
}

fn hello(tty: Option<&str>, surface: Origin) -> HelloInfo {
    HelloInfo { surface, tty: tty.map(str::to_owned), client: None }
}

#[test]
fn pick_up_counts_a_terminal_follow_as_held_and_a_phone_follow_not() {
    let connections = Arc::new(Connections::default());
    connections.opened(ConnId::new(1), hello(Some("pts-7"), Origin::Shell));
    connections.opened(ConnId::new(2), hello(None, Origin::Phone));
    let phone = connections.subscribe(ConnId::new(2), conversation(1), false);
    assert_eq!(connections.followed_from(conversation(1)), Vec::<String>::new());

    let terminal = connections.subscribe(ConnId::new(1), conversation(1), false);
    assert_eq!(connections.followed_from(conversation(1)), ["pts-7"]);
    drop(terminal);
    assert_eq!(connections.followed_from(conversation(1)), Vec::<String>::new());
    drop(phone);
}

/// Process facts that a test sets: every pid runs until it is killed, and every peer
/// runs on the device of the terminal it names.
#[derive(Debug, Default)]
struct Procs {
    dead: Mutex<HashSet<u32>>,
}

impl ProcView for Procs {
    fn alive(&self, pid: u32) -> bool {
        !self.dead.lock().unwrap().contains(&pid)
    }

    fn tty_of(&self, _pid: u32) -> Option<u64> {
        Some(7)
    }

    fn device_of(&self, _tty: &str) -> Option<u64> {
        Some(7)
    }
}

fn prompt(n: u128, pwd: &Path, tty: &str, shell_pid: u32, new: bool) -> Method {
    let mut context = ShellContext::new(pwd);
    context.tty = Some(tty.to_owned());
    context.shell_pid = Some(shell_pid);
    Method::PromptSend(PromptSend {
        command_id: CommandId::from_uuid(uuid::Uuid::from_u128(0xc000 + n)),
        conversation_id: None,
        new_conversation: new,
        text: format!("prompt {n}"),
        context: Some(context),
        last_command: None,
        recent_commands: Vec::new(),
        settings: TurnSettings::default(),
    })
}

/// Sends `method` from `client` and waits, through a connection of its own with no tty
/// (which holds nothing), until its turn completed, so its conversation is idle.
async fn send(client: &mut RawClient, socket: &Path, method: Method) -> PromptSendResult {
    let sent: PromptSendResult = client.call(method).await.unwrap();
    let (mut watcher, _) = RawClient::hello(socket, None).await;
    let stream = watcher
        .send(Method::ConversationSubscribe(ConversationSubscribe {
            conversation_id: sent.conversation_id,
            after_seq: Some(sent.seq),
            answers_input: false,
            drafts: false,
        }))
        .await;
    while let Some(item) = watcher.next(stream).await.unwrap() {
        if item["event"]["kind"] == "turn_completed"
            && item["event"]["turn_id"] == sent.turn_id.to_string()
        {
            break;
        }
    }
    sent
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn map_equals_projection_after_each_step() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let home = dirs.home().to_path_buf();
    let [app_dir, lib_dir]: [PathBuf; 2] = ["p/app", "p/lib"].map(|dir| home.join(dir));
    for dir in [&app_dir, &lib_dir] {
        std::fs::create_dir_all(dir.join("src")).unwrap();
    }
    std::fs::create_dir_all(dirs.dirs().config()).unwrap();
    std::fs::write(
        dirs.dirs().config().join("projects.toml"),
        format!(
            "[[project]]\nid = \"{}\"\nroot = \"{}\"\nname = \"app\"\n\n\
             [[project]]\nid = \"{}\"\nroot = \"{}\"\n",
            project(1),
            app_dir.display(),
            project(2),
            lib_dir.display()
        ),
    )
    .unwrap();
    let procs = Arc::new(Procs::default());
    let mut config = Settings::default();
    config.jobs.max_wakes_per_hour = 0;
    let deps: Deps = testing::deps(&dirs, &clock).with_proc_view(Arc::clone(&procs) as _);
    let daemon = crate::start(config, deps).await.unwrap();
    let state = daemon.state();
    let socket = daemon.socket_path().to_path_buf();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let served = tokio::spawn(daemon.serve(shutdown.clone()));
    let (mut one, _) = RawClient::hello(&socket, Some("pts-1")).await;
    let (mut two, _) = RawClient::hello(&socket, Some("pts-2")).await;
    let check = async |step: &str| {
        let rows = load_slots(&state.readers).await.unwrap();
        assert_eq!(state.conversations.terminals(), Routes::from_rows(rows).terminals, "{step}");
    };

    let first = send(&mut one, &socket, prompt(1, &app_dir, "pts-1", 100, false)).await;
    check("a prompt in app").await;
    let home_one = send(&mut one, &socket, prompt(2, &home, "pts-1", 100, false)).await;
    check("a prompt in ~").await;
    let back = send(&mut one, &socket, prompt(3, &app_dir.join("src"), "pts-1", 100, false)).await;
    assert_eq!(back.conversation_id, first.conversation_id);
    assert!(matches!(back.route.and_then(|route| route.switch), Some(Switch::Back { .. })));
    check("back in app").await;
    let lib = send(&mut one, &socket, prompt(4, &lib_dir, "pts-1", 100, true)).await;
    check(",new in lib").await;
    let nested = send(&mut one, &socket, prompt(5, &lib_dir, "pts-1", 101, false)).await;
    assert_eq!(nested.conversation_id, lib.conversation_id, "a nested shell continues");
    check("a nested shell").await;

    // pts-1's shell exits; a new split in app picks up its app conversation.
    procs.dead.lock().unwrap().insert(100);
    let picked = send(&mut two, &socket, prompt(6, &app_dir, "pts-2", 200, false)).await;
    assert_eq!(picked.conversation_id, first.conversation_id);
    check("a pick-up").await;
    let outside = send(&mut two, &socket, prompt(7, &home, "pts-2", 200, false)).await;
    assert_ne!(
        outside.conversation_id, home_one.conversation_id,
        "the outside scope never picks up"
    );
    check("outside in pts-2").await;
    let lib_two = send(&mut two, &socket, prompt(8, &lib_dir, "pts-2", 200, false)).await;
    assert_eq!(lib_two.conversation_id, lib.conversation_id, "the dead shell's slot lets go");
    check("lib in pts-2").await;
    let (mut three, _) = RawClient::hello(&socket, Some("pts-3")).await;
    let app_three = send(&mut three, &socket, prompt(9, &app_dir, "pts-3", 300, false)).await;
    assert_ne!(app_three.conversation_id, first.conversation_id, "a live shell holds it");
    check("app in pts-3").await;

    drop((one, two, three));
    // NOTE: the drain waits until the last holder of the state lets it go.
    drop(state);
    shutdown.cancel();
    served.await.unwrap().unwrap();
}

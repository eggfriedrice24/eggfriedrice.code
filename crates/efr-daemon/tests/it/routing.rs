//! Routing by scope over a `TestDaemon` (DV1-DV4, DV54, DV55): each terminal keeps one
//! conversation per scope key, a new split in a project picks up the project's newest
//! conversation that no open terminal holds, and the result names every switch.
//!
//! The registry holds `app` (`~/p/app`) and `lib` (`~/p/lib`). Which shell runs and
//! which terminal a peer runs on come from a `FakeProcView`, never from a real process.
//! Every daemon that is not a wake test sets `jobs.max_wakes_per_hour = 0`; the turns
//! run against the empty replay model and fail at once, which routing does not mind.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{
    ConversationId, ConversationsList, ConversationsListResult, ErrorCode, Event, JobsStop,
    JobsStopResult, Method, Mode, NewReason, Origin, PromptSend, PromptSendResult, RouteScope,
    ShellContext, Switch, TurnSettings,
};
use efr_test_daemon::{
    ClientError, FakeLauncher, FakeProcView, TestDaemon, TestDaemonBuilder, command_id,
    events_until,
};
use efr_test_support::TestDirs;
use pretty_assertions::assert_eq;

use crate::jobs_wake::Recorder;
use crate::sandbox::ready;

pub(crate) const APP: &str = "0192f0c1-7a00-7000-8000-0000000a4401";
const LIB: &str = "0192f0c1-7a00-7000-8000-0000000a4402";

pub(crate) fn app() -> RouteScope {
    RouteScope::Project(APP.parse().unwrap())
}

/// A daemon over a home with two registered projects and fake process facts.
pub(crate) struct Place {
    pub(crate) daemon: TestDaemon,
    pub(crate) procs: FakeProcView,
    pub(crate) home: PathBuf,
    pub(crate) app: PathBuf,
    pub(crate) lib: PathBuf,
    /// A directory outside every project.
    pub(crate) notes: PathBuf,
}

/// The tree of a [`Place`], before its daemon starts.
pub(crate) fn tree() -> (Arc<TestDirs>, FakeProcView) {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let home = dirs.home();
    for dir in ["p/app/src", "p/lib", "notes", "p/tool"] {
        std::fs::create_dir_all(home.join(dir)).unwrap();
    }
    std::os::unix::fs::symlink(home.join("p/app/src"), home.join("app-link")).unwrap();
    let config = dirs.dirs().config();
    std::fs::create_dir_all(config).unwrap();
    std::fs::write(
        config.join("projects.toml"),
        format!(
            "[[project]]\nid = \"{APP}\"\nroot = \"{}\"\nname = \"app\"\n\n\
             [[project]]\nid = \"{LIB}\"\nroot = \"{}\"\n",
            home.join("p/app").display(),
            home.join("p/lib").display()
        ),
    )
    .unwrap();
    (dirs, FakeProcView::new())
}

impl Place {
    pub(crate) async fn start() -> Place {
        Place::start_with(|builder| builder).await
    }

    pub(crate) async fn start_with(
        change: impl FnOnce(TestDaemonBuilder) -> TestDaemonBuilder,
    ) -> Place {
        let (dirs, procs) = tree();
        let builder = TestDaemon::builder()
            .dirs(Arc::clone(&dirs))
            .proc_view(procs.clone())
            .config(|config| config.jobs.max_wakes_per_hour = 0);
        Place::on(change(builder).start().await.unwrap(), procs, &dirs)
    }

    pub(crate) fn on(daemon: TestDaemon, procs: FakeProcView, dirs: &TestDirs) -> Place {
        let home = dirs.home().to_path_buf();
        Place {
            daemon,
            procs,
            app: home.join("p/app"),
            lib: home.join("p/lib"),
            notes: home.join("notes"),
            home,
        }
    }

    /// A `prompt.send` with the command id `n` from the shell `pid` of `tty` in `pwd`.
    pub(crate) fn prompt(n: u128, pwd: &Path, tty: &str, pid: u32) -> PromptSend {
        let mut context = ShellContext::new(pwd);
        context.tty = Some(tty.to_owned());
        context.shell_pid = Some(pid);
        PromptSend {
            command_id: command_id(n),
            conversation_id: None,
            new_conversation: false,
            text: format!("prompt {n}"),
            context: Some(context),
            last_command: None,
            recent_commands: Vec::new(),
            settings: TurnSettings::default(),
        }
    }

    /// Sends `params` over a shell connection of its terminal and waits until its turn
    /// ended, through a connection with no tty, which holds nothing.
    pub(crate) async fn send(&self, params: PromptSend) -> PromptSendResult {
        let tty = params.context.as_ref().and_then(|context| context.tty.clone());
        let client = match &tty {
            Some(tty) => self.daemon.client_for_tty(tty).await.unwrap(),
            None => self.daemon.client().await.unwrap(),
        };
        let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
        self.wait(&sent).await;
        sent
    }

    /// Sends `params` that pick up a conversation whose hidden shell starts in another
    /// directory than the prompt's: the shell ends at once (the fake shell never shows
    /// a prompt, so the turn could not move it), then the turn ends.
    pub(crate) async fn picked_up(&self, params: PromptSend) -> PromptSendResult {
        let tty = params.context.as_ref().and_then(|context| context.tty.clone()).unwrap();
        let client = self.daemon.client_for_tty(&tty).await.unwrap();
        let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
        let holder = self.daemon.holder().unwrap();
        let index = holder.specs().len() - 1;
        let terminal = holder.terminal(index).await.unwrap();
        assert!(holder.end(terminal.pty_id(), efr_daemon::ChildStatus::Exited { code: 0 }));
        self.wait(&sent).await;
        sent
    }

    /// [`Place::send`] of a plain prompt.
    pub(crate) async fn at(&self, n: u128, pwd: &Path, tty: &str, pid: u32) -> PromptSendResult {
        self.send(Place::prompt(n, pwd, tty, pid)).await
    }

    /// Waits until the turn of `sent` ended.
    pub(crate) async fn wait(&self, sent: &PromptSendResult) {
        let watcher = self.daemon.client().await.unwrap();
        let mut follow = self.daemon.follow(&watcher, sent.conversation_id).await.unwrap();
        events_until(&mut follow, |event| {
            matches!(event, Event::TurnFailed { turn_id, .. } | Event::TurnCompleted { turn_id, .. } if *turn_id == sent.turn_id)
        })
        .await
        .unwrap();
    }

    /// The conversations that `tty` holds, its current one first.
    pub(crate) async fn held_by(&self, tty: &str) -> Vec<ConversationId> {
        let client = self.daemon.client().await.unwrap();
        let method = Method::ConversationsList(ConversationsList {
            cursor: None,
            limit: None,
            tty: Some(tty.to_owned()),
        });
        let list: ConversationsListResult = client.call(method).await.unwrap();
        list.conversations.iter().map(|summary| summary.id).collect()
    }

    /// The `terminal_attached` events of `conversation`.
    pub(crate) async fn attaches(&self, conversation: ConversationId) -> Vec<Event> {
        let client = self.daemon.client().await.unwrap();
        let events = self.daemon.events(&client, conversation).await.unwrap();
        events
            .into_iter()
            .map(|envelope| envelope.event)
            .filter(|event| matches!(event, Event::TerminalAttached { .. }))
            .collect()
    }

    pub(crate) async fn stop(self) {
        self.daemon.stop().await.unwrap();
    }
}

pub(crate) fn switch(sent: &PromptSendResult) -> Option<&Switch> {
    sent.route.as_ref().and_then(|route| route.switch.as_ref())
}

#[tokio::test]
async fn each_scope_of_a_terminal_keeps_its_conversation() {
    let place = Place::start().await;

    let first = place.at(1, &place.app, "pts-1", 100).await;
    let home = place.at(2, &place.home, "pts-1", 100).await;
    let back = place.at(3, &place.app, "pts-1", 100).await;

    assert_ne!(home.conversation_id, first.conversation_id);
    assert_eq!(back.conversation_id, first.conversation_id);
    assert!(matches!(switch(&home), Some(Switch::New { reason: NewReason::Scope, .. })));
    assert!(
        matches!(switch(&back), Some(Switch::Back { title: Some(title), .. }) if title == "prompt 1")
    );
    assert_eq!(place.held_by("pts-1").await, [first.conversation_id, home.conversation_id]);
    place.stop().await;
}

#[tokio::test]
async fn outside_scope_is_one_conversation_for_home_etc_and_unregistered_dirs() {
    let place = Place::start().await;

    let home = place.at(1, &place.home, "pts-1", 100).await;
    let etc = place.at(2, Path::new("/etc"), "pts-1", 100).await;
    let notes = place.at(3, &place.notes, "pts-1", 100).await;
    let tool = place.at(4, &place.home.join("p/tool"), "pts-1", 100).await;

    for sent in [&etc, &notes, &tool] {
        assert_eq!(sent.conversation_id, home.conversation_id);
        assert_eq!(switch(sent), None);
    }
    let route = tool.route.unwrap();
    assert_eq!((route.scope.scope, route.scope.name), (RouteScope::Outside, None));
    assert_eq!(route.scope.dir, place.home.join("p/tool"));
    place.stop().await;
}

#[tokio::test]
async fn subdirectory_routes_to_its_project() {
    let place = Place::start().await;

    let root = place.at(1, &place.app, "pts-1", 100).await;
    let deep = place.at(2, &place.app.join("src"), "pts-1", 100).await;

    assert_eq!(deep.conversation_id, root.conversation_id);
    let label = deep.route.unwrap().scope;
    assert_eq!((label.scope, label.name.as_deref()), (app(), Some("app")));
    place.stop().await;
}

#[tokio::test]
async fn symlinked_pwd_routes_to_its_project() {
    let place = Place::start().await;

    let root = place.at(1, &place.app, "pts-1", 100).await;
    let linked = place.at(2, &place.home.join("app-link"), "pts-1", 100).await;

    assert_eq!(linked.conversation_id, root.conversation_id);
    place.stop().await;
}

#[tokio::test]
async fn comma_new_replaces_only_the_current_scope() {
    let place = Place::start().await;
    let app_one = place.at(1, &place.app, "pts-1", 100).await;
    let home_one = place.at(2, &place.home, "pts-1", 100).await;

    let fresh = place
        .send(PromptSend { new_conversation: true, ..Place::prompt(3, &place.app, "pts-1", 100) })
        .await;
    let home_again = place.at(4, &place.home, "pts-1", 100).await;
    let app_again = place.at(5, &place.app, "pts-1", 100).await;

    assert_ne!(fresh.conversation_id, app_one.conversation_id);
    assert_eq!(switch(&fresh), None, "the user asked for it");
    assert_eq!(home_again.conversation_id, home_one.conversation_id, "the other slot stays");
    assert_eq!(app_again.conversation_id, fresh.conversation_id);
    place.stop().await;
}

#[tokio::test]
async fn idle_rule_applies_per_scope() {
    let place = Place::start_with(|builder| {
        builder.config(|config| config.conversation.tty_idle_hours = 1)
    })
    .await;
    let app_one = place.at(1, &place.app, "pts-1", 100).await;
    // NOTE: `at` waited for the turn's end, the newest activity; routing reads the clock
    // and has no sleep.
    place.daemon.clock().advance(Duration::from_secs(50 * 60));
    let home_one = place.at(2, &place.home, "pts-1", 100).await;
    place.daemon.clock().advance(Duration::from_secs(20 * 60));

    let app_two = place.at(3, &place.app, "pts-1", 100).await;
    let home_two = place.at(4, &place.home, "pts-1", 100).await;

    assert_ne!(app_two.conversation_id, app_one.conversation_id, "70 minutes in app is idle");
    assert!(matches!(
        switch(&app_two),
        Some(Switch::New { reason: NewReason::Idle { hours: 1 }, .. })
    ));
    assert_eq!(home_two.conversation_id, home_one.conversation_id, "20 minutes in ~ is not");
    place.stop().await;
}

#[tokio::test]
async fn two_open_splits_never_share() {
    let place = Place::start().await;
    place.procs.peer_on_tty("pts-2");

    let one = place.at(1, &place.app, "pts-1", 100).await;
    let two = place.at(2, &place.app, "pts-2", 200).await;

    assert_ne!(two.conversation_id, one.conversation_id, "pts-1's shell still runs");
    assert!(switch(&two).is_none());
    place.stop().await;
}

#[tokio::test]
async fn a_closed_split_is_picked_up_by_a_new_one() {
    let place = Place::start().await;
    let one = place.at(1, &place.app, "pts-1", 100).await;
    place.procs.kill(100);
    place.procs.peer_on_tty("pts-2");

    let two = place.picked_up(Place::prompt(2, &place.app.join("src"), "pts-2", 200)).await;

    assert_eq!(two.conversation_id, one.conversation_id);
    assert!(
        matches!(switch(&two), Some(Switch::PickedUp { title: Some(title), .. }) if title == "prompt 1")
    );
    assert_eq!(
        place.attaches(one.conversation_id).await,
        [Event::TerminalAttached {
            tty: "pts-2".to_owned(),
            route: app(),
            shell_pid: Some(200),
            how: efr_protocol::AttachHow::PickedUp,
        }]
    );
    assert_eq!(place.held_by("pts-1").await, Vec::<ConversationId>::new());
    assert_eq!(place.held_by("pts-2").await, [one.conversation_id]);
    place.stop().await;
}

#[tokio::test]
async fn a_pick_up_starts_the_hidden_shell_in_its_last_directory() {
    let place = Place::start().await;
    let src = place.app.join("src");
    let one = place.at(1, &src, "pts-1", 100).await;
    place.procs.kill(100);
    place.procs.peer_on_tty("pts-2");
    assert!(place.daemon.holder().unwrap().specs().is_empty(), "the turns ran no command");

    let two = place.picked_up(Place::prompt(2, &place.app, "pts-2", 200)).await;

    assert_eq!(two.conversation_id, one.conversation_id);
    let specs = place.daemon.holder().unwrap().specs();
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].cwd, src, "the directory of its last prompt, not the new one");
    place.stop().await;
}

#[tokio::test]
async fn a_reused_pts_with_a_dead_shell_picks_up_the_projects_conversation() {
    let place = Place::start().await;
    let old = place.at(1, &place.app, "pts-1", 100).await;
    place.procs.kill(100);
    place.procs.peer_on_tty("pts-1");

    let reused = place.at(2, &place.app, "pts-1", 300).await;

    assert_eq!(reused.conversation_id, old.conversation_id);
    assert!(matches!(switch(&reused), Some(Switch::PickedUp { .. })));
    place.stop().await;
}

#[tokio::test]
async fn a_reused_pts_that_is_also_idle_picks_up() {
    let place = Place::start_with(|builder| {
        builder.config(|config| config.conversation.tty_idle_hours = 1)
    })
    .await;
    let stale = place.at(1, &place.app, "pts-1", 100).await;
    place.daemon.clock().advance(Duration::from_secs(2 * 3600));
    // A split that closed since holds the project's newest conversation.
    let newer = place.at(2, &place.app, "pts-2", 200).await;
    place.procs.kill(100);
    place.procs.kill(200);
    place.procs.peer_on_tty("pts-1");

    let reused = place.at(3, &place.app, "pts-1", 300).await;

    assert_ne!(newer.conversation_id, stale.conversation_id);
    assert_eq!(reused.conversation_id, newer.conversation_id, "the shell is checked first (F21)");
    assert!(matches!(switch(&reused), Some(Switch::PickedUp { .. })));
    place.stop().await;
}

#[tokio::test]
async fn a_held_newest_never_falls_back_to_an_older_one() {
    let place = Place::start().await;
    let older = place.at(1, &place.app, "pts-1", 100).await;
    let newest = place.at(2, &place.app, "pts-2", 200).await;
    place.procs.kill(100);
    place.procs.peer_on_tty("pts-3");

    let three = place.at(3, &place.app, "pts-3", 300).await;

    assert_ne!(newest.conversation_id, older.conversation_id);
    assert_ne!(three.conversation_id, newest.conversation_id, "pts-2 holds the newest");
    assert_ne!(three.conversation_id, older.conversation_id, "never an older one (F4)");
    place.stop().await;
}

#[tokio::test]
async fn a_followed_conversation_is_held() {
    let place = Place::start().await;
    let one = place.at(1, &place.app, "pts-1", 100).await;
    place.procs.kill(100);
    // Another terminal follows it, as `efr history -f` would.
    let follower = place.daemon.client_for_tty("pts-9").await.unwrap();
    let _follow = place.daemon.follow(&follower, one.conversation_id).await.unwrap();
    // A phone follows it too, which never holds it.
    let phone = place.daemon.connect(place.daemon.connect_options_from(Origin::Phone)).await;
    place.procs.peer_on_tty("pts-2");

    let two = place.at(2, &place.app, "pts-2", 200).await;

    assert_ne!(two.conversation_id, one.conversation_id);
    drop(phone);
    place.stop().await;
}

#[tokio::test]
async fn phone_prompt_never_picks_up() {
    let place = Place::start().await;
    let one = place.at(1, &place.app, "pts-1", 100).await;
    place.procs.kill(100);
    place.procs.peer_on_tty("pts-2");
    let phone =
        place.daemon.connect(place.daemon.connect_options_from(Origin::Phone)).await.unwrap();

    let sent: PromptSendResult =
        phone.call(Method::PromptSend(Place::prompt(2, &place.app, "pts-2", 200))).await.unwrap();
    place.wait(&sent).await;

    assert_ne!(sent.conversation_id, one.conversation_id);
    place.stop().await;
}

#[tokio::test]
async fn made_up_tty_never_picks_up() {
    let place = Place::start().await;
    let one = place.at(1, &place.app, "pts-1", 100).await;
    place.procs.kill(100);
    place.procs.device("pts-2", 2);
    place.procs.peer_on(Some(3));

    let two = place.at(2, &place.app, "pts-2", 200).await;

    assert_ne!(two.conversation_id, one.conversation_id, "the peer runs on another terminal");
    place.stop().await;
}

#[tokio::test]
async fn unreadable_peer_tty_never_picks_up() {
    let place = Place::start().await;
    let one = place.at(1, &place.app, "pts-1", 100).await;
    place.procs.kill(100);
    place.procs.device("pts-2", 2);
    place.procs.peer_on(None);

    let two = place.at(2, &place.app, "pts-2", 200).await;

    assert_ne!(two.conversation_id, one.conversation_id);
    place.stop().await;
}

#[tokio::test]
async fn failed_send_undoes_the_claim_and_records_no_attach() {
    let place = Place::start().await;
    let one = place.at(1, &place.app, "pts-1", 100).await;
    place.procs.kill(100);
    place.procs.peer_on_tty("pts-2");
    let client = place.daemon.client_for_tty("pts-2").await.unwrap();
    let mut refused = Place::prompt(2, &place.app, "pts-2", 200);
    // The conversation refuses a model outside the list before it records anything.
    refused.settings.model = Some("no-such-model".to_owned());

    let error = client.call::<PromptSendResult>(Method::PromptSend(refused)).await;

    let Err(ClientError::Server { body }) = error else { panic!("{error:?}") };
    assert_eq!(body.code, ErrorCode::Invalid);
    assert_eq!(place.attaches(one.conversation_id).await, Vec::<Event>::new());
    assert_eq!(place.held_by("pts-2").await, Vec::<ConversationId>::new());
    // The shell that the pick-up started ended with it.
    let holder = place.daemon.holder().unwrap();
    assert_eq!(holder.specs().len(), 1);
    let first = holder.terminal(0).await.unwrap().pty_id();
    assert!(
        holder.signals().iter().any(|(pty_id, signal, _)| {
            *pty_id == first && *signal == efr_daemon::Signal::Hangup
        })
    );
    // The slot was given back: the next prompt picks the conversation up.
    let two = place.at(3, &place.app, "pts-2", 200).await;
    assert_eq!(two.conversation_id, one.conversation_id);
    assert_eq!(holder.specs().len(), 2, "a new shell for the pick-up");
    place.stop().await;
}

#[tokio::test]
async fn restart_keeps_every_slot_and_the_current_one() {
    let mut place = Place::start_with(TestDaemonBuilder::persistent).await;
    let app_one = place.at(1, &place.app, "pts-1", 100).await;
    let home_one = place.at(2, &place.home, "pts-1", 100).await;
    let lib_one = place.at(3, &place.lib, "pts-1", 100).await;

    place.daemon.restart().await.unwrap();

    assert_eq!(
        place.held_by("pts-1").await,
        [lib_one.conversation_id, home_one.conversation_id, app_one.conversation_id]
    );
    let lib_two = place.at(4, &place.lib, "pts-1", 100).await;
    assert_eq!((lib_two.conversation_id, switch(&lib_two)), (lib_one.conversation_id, None));
    let app_two = place.at(5, &place.app, "pts-1", 100).await;
    assert_eq!(app_two.conversation_id, app_one.conversation_id);
    assert!(matches!(switch(&app_two), Some(Switch::Back { .. })));
    place.stop().await;
}

#[tokio::test]
async fn restart_after_a_long_turn_still_picks_up() {
    let mut place = Place::start_with(|builder| {
        builder.persistent().config(|config| config.conversation.update_interval_ms = 0)
    })
    .await;
    let one = place.at(1, &place.app, "pts-1", 100).await;
    // F3: more than 256 events after the prompt that named the shell.
    let client = place.daemon.client().await.unwrap();
    let before = place.daemon.events(&client, one.conversation_id).await.unwrap().len();
    for n in 0..9 {
        let mut params = Place::prompt(10 + n, &place.app, "pts-1", 100);
        params.text = "x".repeat(40);
        place.send(params).await;
    }
    let after = place.daemon.events(&client, one.conversation_id).await.unwrap().len();
    drop(client);
    assert!(after > before);
    place.daemon.restart().await.unwrap();
    place.procs.kill(100);
    place.procs.peer_on_tty("pts-2");

    let two = place.at(30, &place.app, "pts-2", 200).await;

    assert_eq!(two.conversation_id, one.conversation_id, "the pid came from the column");
    place.stop().await;
}

#[tokio::test]
async fn explicit_conversation_id_changes_no_slot() {
    let place = Place::start().await;
    let app_one = place.at(1, &place.app, "pts-1", 100).await;
    let other = place.at(2, &place.app, "pts-2", 200).await;
    let home_one = place.at(3, &place.home, "pts-1", 100).await;

    let named = place
        .send(PromptSend {
            conversation_id: Some(other.conversation_id),
            ..Place::prompt(4, &place.app, "pts-1", 100)
        })
        .await;

    assert_eq!((named.conversation_id, named.route), (other.conversation_id, None));
    assert_eq!(place.held_by("pts-1").await, [home_one.conversation_id, app_one.conversation_id]);
    assert_eq!(place.held_by("pts-2").await, [other.conversation_id]);
    place.stop().await;
}

#[tokio::test]
async fn prompt_without_a_tty_starts_new_and_holds_nothing() {
    let place = Place::start().await;
    let mut params = Place::prompt(1, &place.app, "unused", 100);
    params.context.as_mut().unwrap().tty = None;
    let first = place.send(params.clone()).await;
    let second = place.send(PromptSend { command_id: command_id(2), ..params }).await;

    assert_ne!(second.conversation_id, first.conversation_id);
    assert_eq!(first.route, None);
    let client = place.daemon.client().await.unwrap();
    let list: ConversationsListResult =
        client.call(Method::ConversationsList(ConversationsList::default())).await.unwrap();
    assert!(list.conversations.iter().all(|summary| summary.tty.is_none()));
    place.stop().await;
}

#[tokio::test]
async fn prompt_without_a_tty_is_never_picked_up() {
    let place = Place::start().await;
    // A script's `efr send` in the project, with no terminal.
    let mut params = Place::prompt(1, &place.app, "unused", 100);
    params.context.as_mut().unwrap().tty = None;
    let script = place.send(params).await;
    place.procs.peer_on_tty("pts-2");

    let two = place.at(2, &place.app, "pts-2", 200).await;

    assert_ne!(two.conversation_id, script.conversation_id);
    assert!(!matches!(switch(&two), Some(Switch::PickedUp { .. })), "{:?}", two.route);
    // A phone prompt with no conversation id and no terminal, after pts-2 closed.
    let phone =
        place.daemon.connect(place.daemon.connect_options_from(Origin::Phone)).await.unwrap();
    let mut params = Place::prompt(3, &place.app, "unused", 900);
    params.context.as_mut().unwrap().tty = None;
    let from_phone: PromptSendResult = phone.call(Method::PromptSend(params)).await.unwrap();
    place.wait(&from_phone).await;
    place.procs.kill(200);
    place.procs.peer_on_tty("pts-3");

    let three = place.at(4, &place.app, "pts-3", 300).await;

    assert_eq!(three.conversation_id, two.conversation_id, "the newest that a terminal held");
    assert!(matches!(switch(&three), Some(Switch::PickedUp { .. })), "{:?}", three.route);
    place.stop().await;
}

#[tokio::test]
async fn follow_project_off_keeps_one_conversation_per_terminal() {
    let place = Place::start_with(|builder| {
        builder.config(|config| config.conversation.follow_project = false)
    })
    .await;
    let one = place.at(1, &place.app, "pts-1", 100).await;
    let home = place.at(2, &place.home, "pts-1", 100).await;
    let lib = place.at(3, &place.lib, "pts-1", 100).await;

    assert_eq!(home.conversation_id, one.conversation_id);
    assert_eq!(lib.conversation_id, one.conversation_id);
    let route = lib.route.unwrap();
    assert_eq!((route.scope.name.as_deref(), route.switch), (Some("lib"), None), "the head line");
    // No pick-up either.
    place.procs.kill(100);
    place.procs.peer_on_tty("pts-2");
    let two = place.at(4, &place.app, "pts-2", 200).await;
    assert_ne!(two.conversation_id, one.conversation_id);
    place.stop().await;
}

#[tokio::test]
async fn follow_project_on_again_finds_the_slots() {
    let (dirs, procs) = tree();
    let config_file = dirs.dirs().config().join("config.toml");
    std::fs::write(&config_file, "[conversation]\nfollow_project = false\n").unwrap();
    let daemon = TestDaemon::builder()
        .dirs(Arc::clone(&dirs))
        .proc_view(procs.clone())
        .config(|config| {
            config.jobs.max_wakes_per_hour = 0;
            config.conversation.follow_project = false;
        })
        .start()
        .await
        .unwrap();
    let place = Place::on(daemon, procs, &dirs);
    let app_one = place.at(1, &place.app, "pts-1", 100).await;
    let fresh = place
        .send(PromptSend { new_conversation: true, ..Place::prompt(2, &place.home, "pts-1", 100) })
        .await;
    let still = place.at(3, &place.app, "pts-1", 100).await;
    assert_eq!(still.conversation_id, fresh.conversation_id, "off: the current slot");

    std::fs::write(&config_file, "[conversation]\nfollow_project = true\n").unwrap();
    let client = place.daemon.client().await.unwrap();
    let _: efr_protocol::AdminConfigReloadResult = client
        .call(Method::AdminConfigReload(efr_protocol::AdminConfigReload::default()))
        .await
        .unwrap();
    let app_two = place.at(4, &place.app, "pts-1", 100).await;
    let home_two = place.at(5, &place.home, "pts-1", 100).await;

    assert_eq!(app_two.conversation_id, app_one.conversation_id, "on: the app slot as it was");
    assert_eq!(home_two.conversation_id, fresh.conversation_id);
    place.stop().await;
}

#[tokio::test]
async fn jobs_stop_diff_undo_take_the_current_slot() {
    let place = Place::start().await;
    let _app = place.at(1, &place.app, "pts-1", 100).await;
    let home = place.at(2, &place.home, "pts-1", 100).await;
    let client = place.daemon.client_for_tty("pts-1").await.unwrap();

    let stop = Method::JobsStop(JobsStop {
        command_id: command_id(50),
        conversation_id: None,
        job_id: "j1".parse().unwrap(),
    });
    let error = client.call::<JobsStopResult>(stop).await;

    let Err(ClientError::Server { body }) = error else { panic!("{error:?}") };
    assert_eq!(body.code, ErrorCode::NotFound);
    assert!(body.message.contains(&home.conversation_id.to_string()), "{}", body.message);
    for method in [
        Method::ConversationDiff(efr_protocol::ConversationDiff::default()),
        Method::TurnUndo(efr_protocol::TurnUndo {
            command_id: command_id(51),
            conversation_id: None,
            turn_id: None,
            dry_run: false,
            force: false,
            redo: false,
            list: true,
        }),
    ] {
        let answer = client.call::<serde_json::Value>(method).await;
        let text = format!("{answer:?}");
        assert!(!text.contains("no active conversation"), "{text}");
    }
    place.stop().await;
}

#[tokio::test]
async fn prompt_send_result_names_scope_and_switch() {
    let place = Place::start().await;
    let _home = place.at(1, &place.home, "pts-1", 100).await;

    let app = place.at(2, &place.app, "pts-1", 100).await;

    let value = serde_json::to_value(&app).unwrap();
    assert_eq!(
        value["route"]["scope"]["scope"],
        serde_json::json!({"kind": "project", "value": APP})
    );
    assert_eq!(value["route"]["scope"]["name"], "app");
    assert_eq!(value["route"]["switch"]["kind"], "new");
    assert_eq!(value["route"]["switch"]["from"]["scope"], serde_json::json!({"kind": "outside"}));
    assert_eq!(value["route"]["switch"]["from"]["dir"], place.home.display().to_string());
    assert_eq!(value["route"]["switch"]["reason"], serde_json::json!({"kind": "scope"}));
    place.stop().await;
}

#[tokio::test]
async fn wake_turn_changes_no_slot() {
    let (dirs, procs) = tree();
    let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
    let fifo = dirs.root().join("go");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
    let job = format!("read answer < {}; exit 3", fifo.display());
    let model = Recorder::new(vec![
        Some(("shell", serde_json::json!({ "command": job, "background": true }))),
        None,
        // The wake turn ends at once.
        None,
        // The prompt in ~.
        None,
    ]);
    let env = BTreeMap::from([
        ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
        ("HOME".to_owned(), dirs.home().display().to_string()),
    ]);
    let daemon = TestDaemon::builder()
        .dirs(Arc::clone(&dirs))
        .proc_view(procs.clone())
        .sandbox_launcher(fake.path())
        .probe_override(ready())
        .job_env(env)
        .custom_provider(model)
        .config(|config| config.jobs.max_wakes_per_hour = 1)
        .start()
        .await
        .unwrap();
    let place = Place::on(daemon, procs, &dirs);
    let mut job_prompt = Place::prompt(1, &place.app, "pts-1", 100);
    job_prompt.settings.mode = Some(Mode::Auto);
    let app = place.send(job_prompt).await;
    let home = place.at(2, &place.home, "pts-1", 100).await;
    let before = place.held_by("pts-1").await;

    let fifo_path = fifo.clone();
    tokio::task::spawn_blocking(move || std::fs::write(&fifo_path, b"go\n"))
        .await
        .unwrap()
        .unwrap();
    let watcher = place.daemon.client().await.unwrap();
    let mut follow = place.daemon.follow(&watcher, app.conversation_id).await.unwrap();
    let mut ends = 0;
    events_until(&mut follow, |event| {
        if matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. }) {
            ends += 1;
        }
        ends == 2
    })
    .await
    .unwrap();
    drop(follow);

    assert_eq!(before, [home.conversation_id, app.conversation_id]);
    assert_eq!(place.held_by("pts-1").await, before, "the wake turn moved no slot");
    let again = place.at(3, &place.home, "pts-1", 100).await;
    assert_eq!((again.conversation_id, switch(&again)), (home.conversation_id, None));
    place.stop().await;
}

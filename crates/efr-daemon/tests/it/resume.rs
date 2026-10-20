//! `conversations.recent` and `conversation.resume` over a `TestDaemon` (DV5): the list
//! of `,resume` with the current scope first, and the move of a conversation to the
//! terminal that asks, with its hidden shell and its jobs.
//!
//! The place is the one of `routing.rs`: `app` and `lib` are registered, and which
//! shell runs and which terminal a peer runs on come from a `FakeProcView`. Every
//! daemon that is not a wake test sets `jobs.max_wakes_per_hour = 0`; the turns run
//! against the empty replay model and fail at once, which a resume does not mind.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use efr_daemon::ChildStatus;
use efr_protocol::{
    AttachHow, ConversationId, ConversationResume, ConversationResumeResult, ConversationsRecent,
    ConversationsRecentResult, ErrorCode, Event, JobsStop, JobsStopResult, Method, Mode, NewReason,
    Origin, RouteScope, ShellContext, Switch,
};
use efr_test_daemon::{
    ClientError, FakeLauncher, PROMPT, TestDaemon, TestDaemonBuilder, command_id, events_until,
};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;

use crate::jobs_wake::Recorder;
use crate::routing::{Place, app, switch, tree};
use crate::sandbox::ready;

/// The params of a resume of `conversation` from the shell `pid` of `tty` in `pwd`.
fn params(
    n: u128,
    conversation: ConversationId,
    pwd: &Path,
    tty: &str,
    pid: u32,
    take: bool,
) -> ConversationResume {
    let mut context = ShellContext::new(pwd);
    context.tty = Some(tty.to_owned());
    context.shell_pid = Some(pid);
    ConversationResume {
        command_id: command_id(1000 + n),
        conversation_id: conversation,
        context,
        take,
    }
}

impl Place {
    /// Sends `params` over the shell connection of its terminal.
    async fn resume(
        &self,
        params: ConversationResume,
    ) -> Result<ConversationResumeResult, ClientError> {
        let tty = params.context.tty.clone().unwrap();
        let client = self.daemon.client_for_tty(&tty).await.unwrap();
        client.call(Method::ConversationResume(params)).await
    }

    /// The list of `,resume` for `tty` in `pwd`.
    async fn recent(&self, tty: Option<&str>, pwd: Option<&Path>) -> ConversationsRecentResult {
        let client = self.daemon.client().await.unwrap();
        let method = Method::ConversationsRecent(ConversationsRecent {
            tty: tty.map(str::to_owned),
            pwd: pwd.map(Path::to_path_buf),
            limit: None,
        });
        client.call(method).await.unwrap()
    }
}

fn ids(list: &ConversationsRecentResult) -> Vec<ConversationId> {
    list.conversations.iter().map(|row| row.summary.id).collect()
}

fn resumed(result: &ConversationResumeResult) -> (ConversationId, bool, bool) {
    match result {
        ConversationResumeResult::Resumed { conversation_id, here, continues_here, .. } => {
            (*conversation_id, *here, *continues_here)
        }
        other => panic!("not resumed: {other:?}"),
    }
}

#[tokio::test]
async fn recent_lists_the_current_scope_first_then_newest() {
    let place = Place::start().await;
    let app_one = place.at(1, &place.app, "pts-1", 100).await;
    let home = place.at(2, &place.home, "pts-1", 100).await;
    let app_two = place.at(3, &place.app, "pts-2", 200).await;
    let lib = place.at(4, &place.lib, "pts-2", 200).await;
    let _again = place.at(5, &place.app, "pts-2", 200).await;

    let list = place.recent(Some("pts-1"), Some(&place.app.join("src"))).await;

    assert_eq!(
        ids(&list),
        [
            app_two.conversation_id,
            app_one.conversation_id,
            lib.conversation_id,
            home.conversation_id
        ]
    );
    let scope = list.scope.unwrap();
    assert_eq!((scope.scope, scope.name.as_deref()), (app(), Some("app")));
    assert_eq!(list.current, Some(home.conversation_id), "pts-1's last prompt was in ~");
    let first = &list.conversations[0];
    assert_eq!((first.project.as_deref(), first.turns), (Some("app"), 2));
    assert_eq!(list.conversations[2].project.as_deref(), Some("lib"));
    assert_eq!(
        (&list.conversations[3].route, list.conversations[3].project.as_deref()),
        (&RouteScope::Outside, None)
    );
    place.stop().await;
}

#[tokio::test]
async fn recent_marks_this_terminal_and_another_open_one() {
    let place = Place::start().await;
    let mine = place.at(1, &place.app, "pts-1", 100).await;
    let theirs = place.at(2, &place.lib, "pts-2", 200).await;
    let closed = place.at(3, &place.home, "pts-3", 300).await;
    place.procs.kill(300);

    let list = place.recent(Some("pts-1"), Some(&place.app)).await;

    let marks: Vec<_> = list
        .conversations
        .iter()
        .map(|row| (row.summary.id, row.here, row.open_in.as_deref()))
        .collect();
    assert_eq!(
        marks,
        [
            (mine.conversation_id, true, None),
            (closed.conversation_id, false, None),
            (theirs.conversation_id, false, Some("pts-2")),
        ]
    );
    place.stop().await;
}

#[tokio::test]
async fn recent_names_a_removed_project() {
    let place = Place::start().await;
    let one = place.at(1, &place.lib, "pts-1", 100).await;
    let registry = place.daemon.dirs().dirs().config().join("projects.toml");
    let text = std::fs::read_to_string(&registry).unwrap();
    let app_only = text.split("\n\n[[project]]").next().unwrap().to_owned();
    std::fs::write(&registry, format!("{app_only}\n")).unwrap();

    let list = place.recent(None, None).await;

    let row = &list.conversations[0];
    assert_eq!(row.summary.id, one.conversation_id);
    assert!(matches!(row.route, RouteScope::Project(_)), "the key stays");
    assert_eq!(row.project, None, "a removed project has no name");
    place.stop().await;
}

#[tokio::test]
async fn recent_without_tty_is_newest_first() {
    let place = Place::start().await;
    let app_one = place.at(1, &place.app, "pts-1", 100).await;
    let home = place.at(2, &place.home, "pts-2", 200).await;
    let lib = place.at(3, &place.lib, "pts-3", 300).await;

    let list = place.recent(None, None).await;

    assert_eq!(ids(&list), [lib.conversation_id, home.conversation_id, app_one.conversation_id]);
    assert_eq!((list.scope, list.current), (None, None));
    let held: Vec<_> = list.conversations.iter().map(|row| row.open_in.as_deref()).collect();
    assert_eq!(held, [Some("pts-3"), Some("pts-2"), Some("pts-1")], "the phone sees every holder");
    assert!(list.conversations.iter().all(|row| !row.here));
    place.stop().await;
}

#[tokio::test]
async fn resume_moves_the_conversation_to_this_terminal() {
    let mut place = Place::start_with(TestDaemonBuilder::persistent).await;
    let one = place.at(1, &place.app, "pts-1", 100).await;
    let home = place.at(2, &place.home, "pts-2", 200).await;
    place.procs.kill(100);
    place.procs.peer_on_tty("pts-2");

    let result =
        place.resume(params(1, one.conversation_id, &place.app, "pts-2", 200, false)).await;

    let result = result.unwrap();
    assert_eq!(resumed(&result), (one.conversation_id, false, true));
    let ConversationResumeResult::Resumed { scope, title, .. } = &result else { unreachable!() };
    assert_eq!((scope.scope.clone(), title.as_deref()), (app(), Some("prompt 1")));
    assert_eq!(
        place.attaches(one.conversation_id).await,
        [Event::TerminalAttached {
            tty: "pts-2".to_owned(),
            route: app(),
            shell_pid: Some(200),
            how: AttachHow::Resumed,
        }]
    );
    // The projection: pts-2 holds both, the resumed one current; pts-1 holds none.
    assert_eq!(place.held_by("pts-2").await, [one.conversation_id, home.conversation_id]);
    assert_eq!(place.held_by("pts-1").await, Vec::<ConversationId>::new());
    // The map agrees: the next prompt in app continues it with no switch.
    let next = place.at(3, &place.app, "pts-2", 200).await;
    assert_eq!((next.conversation_id, switch(&next)), (one.conversation_id, None));
    // And a fresh load agrees too.
    place.daemon.restart().await.unwrap();
    let after = place.at(4, &place.app, "pts-2", 200).await;
    assert_eq!((after.conversation_id, switch(&after)), (one.conversation_id, None));
    place.stop().await;
}

#[tokio::test]
async fn resume_of_a_held_conversation_answers_held_and_moves_nothing() {
    let place = Place::start().await;
    let one = place.at(1, &place.app, "pts-1", 100).await;
    place.procs.peer_on_tty("pts-2");

    let result =
        place.resume(params(1, one.conversation_id, &place.app, "pts-2", 200, false)).await;

    assert_eq!(
        result.unwrap(),
        ConversationResumeResult::Held {
            tty: "pts-1".to_owned(),
            title: Some("prompt 1".to_owned())
        }
    );
    assert_eq!(place.attaches(one.conversation_id).await, Vec::<Event>::new());
    assert_eq!(place.held_by("pts-1").await, [one.conversation_id]);
    assert_eq!(place.held_by("pts-2").await, Vec::<ConversationId>::new());
    place.stop().await;
}

#[tokio::test]
async fn resume_with_take_moves_it_and_the_old_terminal_gets_moved() {
    let place = Place::start().await;
    let one = place.at(1, &place.app, "pts-1", 100).await;
    place.procs.peer_on_tty("pts-2");

    let result = place.resume(params(1, one.conversation_id, &place.app, "pts-2", 200, true)).await;

    assert_eq!(resumed(&result.unwrap()).0, one.conversation_id);
    assert_eq!(place.held_by("pts-2").await, [one.conversation_id]);
    assert_eq!(place.held_by("pts-1").await, Vec::<ConversationId>::new());
    // pts-1's next prompt in app starts a new conversation and says where it went.
    let next = place.at(2, &place.app, "pts-1", 100).await;
    assert_ne!(next.conversation_id, one.conversation_id);
    assert_eq!(
        switch(&next),
        Some(&Switch::New {
            from: None,
            reason: NewReason::Moved {
                tty: "pts-2".to_owned(),
                title: Some("prompt 1".to_owned())
            }
        })
    );
    place.stop().await;
}

#[tokio::test]
async fn resume_into_another_scope_takes_that_scope_slot() {
    let place = Place::start().await;
    let one = place.at(1, &place.app, "pts-1", 100).await;
    let home = place.at(2, &place.home, "pts-2", 200).await;
    place.procs.kill(100);
    place.procs.peer_on_tty("pts-2");

    let result =
        place.resume(params(1, one.conversation_id, &place.home, "pts-2", 200, false)).await;

    assert_eq!(resumed(&result.unwrap()), (one.conversation_id, false, false), "not from ~");
    // The outside slot keeps its conversation; the app slot is the resumed one.
    let in_home = place.at(3, &place.home, "pts-2", 200).await;
    assert_eq!(in_home.conversation_id, home.conversation_id);
    let in_app = place.at(4, &place.app, "pts-2", 200).await;
    assert_eq!(in_app.conversation_id, one.conversation_id);
    assert!(matches!(switch(&in_app), Some(Switch::Back { .. })));
    place.stop().await;
}

/// A conversation of `pts-1` in `app` whose shell ended, resumed from `pts-2` in `~`:
/// its first hidden shell starts in the directory of its prompt; the test moves that
/// shell to `app/src` with an OSC 7 report and ends it.
async fn resumed_once_and_moved(place: &Place) -> ConversationId {
    let one = place.at(1, &place.app, "pts-1", 100).await;
    place.procs.kill(100);
    place.procs.peer_on_tty("pts-2");
    let first =
        place.resume(params(1, one.conversation_id, &place.home, "pts-2", 200, false)).await;
    let ConversationResumeResult::Resumed { shell_dir, .. } = first.unwrap() else { panic!() };
    let holder = place.daemon.holder().unwrap().clone();
    assert_eq!(shell_dir.as_deref(), Some(place.app.as_path()), "the prompt's directory");
    assert_eq!(holder.specs()[0].cwd, place.app);
    let mut terminal = holder.terminal(0).await.unwrap();
    let src = place.app.join("src");
    let report = format!("\x1b]7;file://{}\x07", src.display());
    terminal.print(report.as_bytes()).await.unwrap();
    terminal.print(PROMPT).await.unwrap();
    let watcher = place.daemon.client().await.unwrap();
    let mut follow = place.daemon.follow(&watcher, one.conversation_id).await.unwrap();
    events_until(
        &mut follow,
        |event| matches!(event, Event::CwdChanged { cwd, .. } if *cwd == src),
    )
    .await
    .unwrap();
    assert!(holder.end(terminal.pty_id(), ChildStatus::Exited { code: 0 }));
    events_until(&mut follow, |event| matches!(event, Event::ShellExited { .. })).await.unwrap();
    one.conversation_id
}

#[tokio::test]
async fn resume_starts_the_hidden_shell_in_its_last_directory() {
    let place = Place::start().await;
    let conversation = resumed_once_and_moved(&place).await;

    let again = place.resume(params(2, conversation, &place.home, "pts-2", 200, false)).await;

    let ConversationResumeResult::Resumed { shell_dir, here, .. } = again.unwrap() else {
        panic!()
    };
    let src = place.app.join("src");
    assert!(here, "pts-2 held it already");
    assert_eq!(shell_dir.as_deref(), Some(src.as_path()));
    let specs = place.daemon.holder().unwrap().specs();
    assert_eq!(specs.len(), 2);
    assert_eq!(specs[1].cwd, src);
    place.stop().await;
}

#[tokio::test]
async fn resume_falls_back_to_the_prompt_dir_when_the_last_dir_is_gone() {
    let place = Place::start().await;
    let conversation = resumed_once_and_moved(&place).await;
    std::fs::remove_dir(place.app.join("src")).unwrap();

    let again = place.resume(params(2, conversation, &place.home, "pts-2", 200, false)).await;

    let ConversationResumeResult::Resumed { shell_dir, .. } = again.unwrap() else { panic!() };
    assert_eq!(shell_dir.as_deref(), Some(place.app.as_path()));
    assert_eq!(place.daemon.holder().unwrap().specs()[1].cwd, place.app);
    place.stop().await;
}

#[tokio::test]
async fn resume_of_this_terminals_conversation_only_makes_it_current() {
    let place = Place::start().await;
    let app_one = place.at(1, &place.app, "pts-1", 100).await;
    let home = place.at(2, &place.home, "pts-1", 100).await;
    place.procs.peer_on_tty("pts-1");
    assert_eq!(place.held_by("pts-1").await, [home.conversation_id, app_one.conversation_id]);

    let result =
        place.resume(params(1, app_one.conversation_id, &place.home, "pts-1", 100, false)).await;

    assert_eq!(resumed(&result.unwrap()), (app_one.conversation_id, true, false));
    assert_eq!(place.held_by("pts-1").await, [app_one.conversation_id, home.conversation_id]);
    place.stop().await;
}

#[tokio::test]
async fn resume_from_the_phone_is_refused() {
    let place = Place::start().await;
    let one = place.at(1, &place.app, "pts-1", 100).await;
    place.procs.kill(100);
    place.procs.peer_on_tty("pts-2");
    let phone =
        place.daemon.connect(place.daemon.connect_options_from(Origin::Phone)).await.unwrap();

    let refused = phone
        .call::<ConversationResumeResult>(Method::ConversationResume(params(
            1,
            one.conversation_id,
            &place.app,
            "pts-2",
            200,
            true,
        )))
        .await;

    let Err(ClientError::Server { body }) = refused else { panic!("{refused:?}") };
    assert_eq!(body.code, ErrorCode::Forbidden);
    assert_eq!(place.attaches(one.conversation_id).await, Vec::<Event>::new());
    place.stop().await;
}

#[tokio::test]
async fn resume_with_a_made_up_tty_is_refused() {
    let place = Place::start().await;
    let one = place.at(1, &place.app, "pts-1", 100).await;
    place.procs.kill(100);
    place.procs.device("pts-2", 2);
    place.procs.peer_on(Some(3));

    let refused =
        place.resume(params(1, one.conversation_id, &place.app, "pts-2", 200, true)).await;

    let Err(ClientError::Server { body }) = refused else { panic!("{refused:?}") };
    assert_eq!(body.code, ErrorCode::Forbidden);
    assert!(body.message.contains("pts-2"), "{}", body.message);
    assert_eq!(place.held_by("pts-2").await, Vec::<ConversationId>::new());
    place.stop().await;
}

#[tokio::test]
async fn resume_keeps_running_jobs_and_their_notices_follow() {
    let (dirs, procs) = tree();
    let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
    let fifo = dirs.root().join("go");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
    let job = format!("read answer < {}; exit 3", fifo.display());
    let model = Recorder::new(vec![
        Some(("shell", serde_json::json!({ "command": job, "background": true }))),
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
        .config(|config| config.jobs.max_wakes_per_hour = 0)
        .start()
        .await
        .unwrap();
    let place = Place::on(daemon, procs, &dirs);
    let mut job_prompt = Place::prompt(1, &place.app, "pts-1", 100);
    job_prompt.settings.mode = Some(Mode::Auto);
    let one = place.send(job_prompt).await;
    place.procs.kill(100);
    place.procs.peer_on_tty("pts-2");

    let result =
        place.resume(params(1, one.conversation_id, &place.app, "pts-2", 200, false)).await;
    assert_eq!(resumed(&result.unwrap()).0, one.conversation_id);

    let client = place.daemon.client().await.unwrap();
    let watcher = place.daemon.client().await.unwrap();
    let mut follow = place.daemon.follow(&watcher, one.conversation_id).await.unwrap();
    let stop = Method::JobsStop(JobsStop {
        command_id: command_id(50),
        conversation_id: Some(one.conversation_id),
        job_id: "j1".parse().unwrap(),
    });
    let stopped: JobsStopResult = client.call(stop).await.unwrap();
    assert_eq!(stopped.job.id.as_str(), "j1", "the job ran on after the resume");
    events_until(&mut follow, |event| matches!(event, Event::JobEnded { .. })).await.unwrap();

    let notices = dirs.dirs().runtime().join("notices");
    let read = |name: &str| std::fs::read_to_string(notices.join(name)).unwrap_or_default();
    // NOTE: efrd writes each notice line with one write, after the commit of its event.
    Wait::new("the notice of the job in pts-2's file")
        .until(|| read("pts-2").lines().any(|line| line.starts_with("efr: job j1 ")))
        .await
        .unwrap();
    assert!(!read("pts-1").contains("job j1"), "the old terminal gets no job notice");
    place.stop().await;
}

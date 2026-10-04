use pretty_assertions::assert_eq;

use super::*;
use crate::Batch;
use crate::testing::{self, TestClock, on_writer};

fn shell_started(pty: u64, cwd: &str) -> Event {
    Event::ShellStarted { pty_id: testing::pty(pty), cwd: testing::path(cwd), pid: Some(4000) }
}

#[tokio::test]
async fn a_started_shell_runs_in_its_directory() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);

    let committed = writer
        .append(
            Batch::new().event(id, testing::created(None)).event(id, shell_started(1, "/home/me")),
        )
        .await
        .unwrap();

    let pty = testing::pty(1);
    let shell = on_writer(&writer, move |conn| get(conn, pty)).await.unwrap().unwrap();
    let envelope = &committed.events()[1];
    assert_eq!(
        shell,
        Shell {
            pty_id: pty,
            conversation_id: id,
            status: ShellStatus::Running,
            cwd: testing::path("/home/me"),
            host: None,
            pid: Some(4000),
            exit_code: None,
            started_seq: envelope.seq,
            started_at: envelope.at,
            exited_seq: None,
            exited_at: None,
        }
    );
}

#[tokio::test]
async fn a_cwd_report_moves_the_shell() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer
        .append(
            Batch::new().event(id, testing::created(None)).event(id, shell_started(1, "/")).event(
                id,
                Event::CwdChanged {
                    pty_id: testing::pty(1),
                    cwd: testing::path("/var/log"),
                    host: Some("box".to_owned()),
                },
            ),
        )
        .await
        .unwrap();

    let pty = testing::pty(1);
    let shell = on_writer(&writer, move |conn| get(conn, pty)).await.unwrap().unwrap();

    assert_eq!(shell.cwd, testing::path("/var/log"));
    assert_eq!(shell.host.as_deref(), Some("box"));
}

#[tokio::test]
async fn an_exited_shell_keeps_its_exit_code_and_leaves_the_running_list() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    let committed = writer
        .append(
            Batch::new()
                .event(id, testing::created(None))
                .event(id, shell_started(1, "/"))
                .event(id, shell_started(2, "/tmp"))
                .event(id, Event::ShellExited { pty_id: testing::pty(1), exit_code: Some(130) }),
        )
        .await
        .unwrap();

    let pty = testing::pty(1);
    let exited = on_writer(&writer, move |conn| get(conn, pty)).await.unwrap().unwrap();
    let still_running = on_writer(&writer, running).await.unwrap();

    assert_eq!(exited.status, ShellStatus::Exited);
    assert_eq!(exited.exit_code, Some(130));
    assert_eq!(exited.exited_seq, Some(committed.events()[3].seq));
    assert_eq!(exited.exited_at, Some(committed.events()[3].at));
    let running: Vec<PtyId> = still_running.iter().map(|shell| shell.pty_id).collect();
    assert_eq!(running, [testing::pty(2)]);
}

#[tokio::test]
async fn for_conversation_lists_that_conversations_shells_oldest_first() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let (one, two) = (testing::conversation(1), testing::conversation(2));
    writer
        .append(
            Batch::new()
                .event(one, testing::created(None))
                .event(two, testing::created(None))
                .event(one, shell_started(3, "/"))
                .event(two, shell_started(2, "/"))
                .event(one, shell_started(1, "/")),
        )
        .await
        .unwrap();

    let shells = on_writer(&writer, move |conn| for_conversation(conn, one)).await.unwrap();

    let ptys: Vec<PtyId> = shells.iter().map(|shell| shell.pty_id).collect();
    assert_eq!(ptys, [testing::pty(3), testing::pty(1)]);
}

#[tokio::test]
async fn an_unknown_pty_has_no_shell() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let pty = testing::pty(7);
    assert_eq!(on_writer(&writer, move |conn| get(conn, pty)).await.unwrap(), None);
}

#[test]
fn status_names_round_trip() {
    for status in [ShellStatus::Running, ShellStatus::Exited] {
        assert_eq!(ShellStatus::from_column(status.as_str()).unwrap(), status);
    }
    assert!(ShellStatus::from_column("zombie").is_err());
}

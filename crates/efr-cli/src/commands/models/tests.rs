use efr_protocol::Origin;
use pretty_assertions::assert_eq;

use super::{listing, names};
use crate::error::Exit;
use crate::run;
use crate::testing::{TestEnv, capture, command, models};

#[test]
fn the_listing_marks_the_default_and_names_the_efforts() {
    insta::assert_snapshot!(listing(&models()));
}

#[test]
fn names_are_the_ids_alone() {
    assert_eq!(names(&models()), "gpt-5.5\ngpt-5.4\nmy-model\n");
}

#[test]
fn text_from_the_daemon_cannot_drive_the_terminal() {
    let mut list = models();
    list.models[0].id = "evil\x1b]52;c;aGk=\x07".to_owned();
    list.models[0].efforts = vec!["\x1b[2J".to_owned()];
    assert!(!listing(&list).contains('\x1b'));
    assert!(!names(&list).contains('\x1b'));
}

#[tokio::test]
async fn efr_models_prints_the_daemons_list() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        assert_eq!(conn.hello().origin, Origin::Cli);
        conn.answer_models(&models()).await;
        conn.until_closed().await;
    };
    let line = command(&["models", "--names"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success, "{}", captured.stderr());
    assert_eq!(captured.stdout(), "gpt-5.5\ngpt-5.4\nmy-model\n");
}

#[tokio::test]
async fn without_a_daemon_models_exits_with_three() {
    let env = TestEnv::new();
    let (mut out, captured) = capture();
    let exit = run::run(&command(&["models"]), &env.context(), &mut out).await;
    assert_eq!(exit, Exit::NotRunning);
    assert!(captured.stderr().contains("systemctl --user start efrd"), "{}", captured.stderr());
}

use std::path::PathBuf;

use efr_protocol::{ClientFrame, Method, ProviderWarm, ProviderWarmResult};
use efr_stdx::env::{Env, Var};
use pretty_assertions::assert_eq;

use crate::error::Exit;
use crate::run;
use crate::testing::{TestEnv, capture, command};

#[tokio::test]
async fn efr_warm_sends_the_terminal_the_directory_and_the_asked_model_and_writes_nothing() {
    let env = TestEnv::new();
    let mut ctx = env.context();
    ctx.env = Env::fixed([(Var::Model, "work/claude-opus-5-5".to_owned())]);
    let daemon = env.listen();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let mut asked = None;
        while let Some(frame) = conn.recv().await {
            let ClientFrame::Request { id, method } = frame else { continue };
            match method {
                Method::ProviderWarm(params) => {
                    asked = Some(params);
                    let result = ProviderWarmResult { provider: Some("anthropic-api.work".into()) };
                    conn.reply(id, &result).await;
                }
                other => panic!("unexpected {}", other.name()),
            }
        }
        asked
    };
    let line = command(&["warm", "--tty", "/dev/pts/7", "--new"]);
    let (exit, asked) = tokio::join!(run::run(&line, &ctx, &mut out), script);

    assert_eq!(exit, Exit::Success);
    assert_eq!(
        asked,
        Some(ProviderWarm {
            tty: Some("/dev/pts/7".into()),
            pwd: Some(PathBuf::from("/home/user/project")),
            model: Some("claude-opus-5-5".into()),
            provider: Some("work".into()),
            new_conversation: true,
        })
    );
    assert_eq!((captured.stdout(), captured.stderr()), (String::new(), String::new()));
}

#[tokio::test]
async fn efr_warm_without_a_daemon_writes_nothing_and_fails() {
    let env = TestEnv::new();
    let (mut out, captured) = capture();

    let exit = run::run(&command(&["warm"]), &env.context(), &mut out).await;

    assert_ne!(exit, Exit::Success);
    assert_eq!((captured.stdout(), captured.stderr()), (String::new(), String::new()));
}

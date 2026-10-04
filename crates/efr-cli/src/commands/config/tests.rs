use std::path::{Path, PathBuf};

use efr_client::{ClientError, DaemonInfo, Discovered};
use efr_stdx::env::{Env, Var};
use efr_stdx::paths::Dirs;
use pretty_assertions::assert_eq;

use super::effective;
use crate::context::Context;
use crate::error::Exit;
use crate::run;
use crate::settings::Settings;
use crate::terminal::TermFacts;
use crate::testing::{TestEnv, capture, command, terminal_facts};

fn fixed_dirs() -> Dirs {
    Dirs::new("/c/efr", "/d/efr", "/s/efr", "/run/user/1000/efr").unwrap()
}

fn default_socket() -> Result<Discovered, ClientError> {
    Ok(Discovered { socket: PathBuf::from("/run/user/1000/efr/daemon.sock"), info: None })
}

#[test]
fn defaults_with_their_sources() {
    let env = TestEnv::new();
    let ctx = Context { dirs: fixed_dirs(), term: terminal_facts(), ..env.context() };
    insta::assert_snapshot!(effective(&ctx, &default_socket()));
}

#[test]
fn overrides_name_where_they_come_from() {
    let env = TestEnv::new();
    let settings = Settings::parse(
        Path::new("/c/efr/config.toml"),
        "[render]\ntheme = \"gruvbox-dark\"\n[daemon]\nanything = 1\n",
    );
    let ctx = Context {
        dirs: fixed_dirs(),
        env: Env::fixed([(Var::DataDir, "/d/efr"), (Var::OpenBrowser, "yes"), (Var::Log, "debug")]),
        term: TermFacts { colorterm: Some("truecolor".to_owned()), ..terminal_facts() },
        settings,
        ..env.context()
    };
    let discovered = Ok(Discovered {
        socket: PathBuf::from("/tmp/elsewhere/daemon.sock"),
        info: Some(DaemonInfo {
            pid: 1,
            socket: PathBuf::from("/tmp/elsewhere/daemon.sock"),
            protocol: efr_protocol::PROTOCOL_VERSION,
            daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
            tailnet_endpoint: None,
        }),
    });
    insta::assert_snapshot!(effective(&ctx, &discovered));
}

#[test]
fn pipes_no_color_and_problems_are_explained() {
    let env = TestEnv::new();
    let settings = Settings::parse(Path::new("/c/efr/config.toml"), "[render]\ntheme = \"neon\"\n");
    let ctx = Context {
        dirs: fixed_dirs(),
        env: Env::fixed([(Var::OpenBrowser, "maybe")]),
        term: TermFacts { no_color: true, ..TermFacts::default() },
        settings,
        ..env.context()
    };
    let discovered = Err(ClientError::ProtocolMismatch { daemon: 9, client: 1 });
    insta::assert_snapshot!(effective(&ctx, &discovered));
}

#[test]
fn a_dumb_terminal_gets_raw_markdown() {
    let env = TestEnv::new();
    let ctx = Context {
        dirs: fixed_dirs(),
        term: TermFacts { term: Some("dumb".to_owned()), ..terminal_facts() },
        ..env.context()
    };
    let text = effective(&ctx, &default_socket());
    assert!(text.contains("formatted = false  # TERM is dumb; raw markdown\n"), "{text}");
}

#[tokio::test]
async fn config_show_needs_no_daemon_and_skips_the_warning_preamble() {
    let env = TestEnv::new();
    let settings = Settings::parse(Path::new("/c/efr/config.toml"), "[render\n");
    let ctx = Context { settings, ..env.context() };
    let (mut out, captured) = capture();
    let exit = run::run(&command(&["config", "show"]), &ctx, &mut out).await;
    assert_eq!(exit, Exit::Success);
    assert_eq!(captured.stderr(), "");
    assert!(captured.stdout().contains("# warning: /c/efr/config.toml is not valid"));
}

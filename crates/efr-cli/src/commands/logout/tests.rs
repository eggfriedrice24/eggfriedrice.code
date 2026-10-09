use efr_protocol::{AdminLogout, AdminLogoutResult, Method};
use pretty_assertions::assert_eq;

use crate::error::Exit;
use crate::run;
use crate::testing::{TestEnv, capture, command};

/// Runs `efr logout <name>` against a daemon that answers `logged_out`, and returns
/// the provider id that the daemon got and what efr printed.
async fn logout(name: &str, logged_out: bool) -> (String, String) {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        let Method::AdminLogout(AdminLogout { provider }) = method else {
            panic!("{}", method.name())
        };
        conn.reply(id, &AdminLogoutResult { provider: provider.clone(), logged_out }).await;
        conn.until_closed().await;
        provider
    };
    let line = command(&["logout", name]);
    let (exit, provider) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(captured.stderr(), "");
    (provider, captured.stdout())
}

#[tokio::test]
async fn a_logout_names_the_provider_by_its_id_and_says_where_a_key_is_revoked() {
    assert_eq!(
        logout("anthropic", true).await,
        (
            "anthropic-api".to_owned(),
            "logged out of anthropic-api. The key stays valid at Anthropic; revoke it in the Claude Console.\n"
                .to_owned()
        )
    );
    assert_eq!(
        logout("openai-api", true).await,
        (
            "openai-api".to_owned(),
            "logged out of openai-api. The key stays valid at OpenAI; revoke it in the dashboard of the OpenAI platform.\n"
                .to_owned()
        )
    );
    assert_eq!(
        logout("openai", true).await,
        ("openai-subscription".to_owned(), "logged out of openai-subscription\n".to_owned())
    );
}

#[tokio::test]
async fn a_logout_without_a_login_says_so() {
    assert_eq!(
        logout("anthropic-api", false).await,
        ("anthropic-api".to_owned(), "anthropic-api was not logged in\n".to_owned())
    );
}

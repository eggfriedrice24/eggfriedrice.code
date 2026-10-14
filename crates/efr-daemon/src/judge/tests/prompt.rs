//! The two variants of the classifier prompt (phase D2, DP28): one per network mode of
//! the engine, the same text but for the network lines.

use efr_conversation::{ExitJudge as _, JudgeInput};
use efr_permissions::Egress;
use pretty_assertions::assert_eq;

use super::super::{PROMPT, PROMPT_PROXY, prompt};
use super::{ALLOW, Reply, Scripted, ids, input, judge, record, setup};

#[test]
fn judge_prompts_differ_only_in_network_lines() {
    assert_eq!(prompt(Egress::None), PROMPT);
    assert_eq!(prompt(Egress::Proxy), PROMPT_PROXY);
    let none: Vec<&str> = PROMPT.lines().collect();
    let proxy: Vec<&str> = PROMPT_PROXY.lines().collect();
    // The one changed line is the sandbox paragraph: the proxy replaces "no network".
    let changed: Vec<(usize, &str)> = none
        .iter()
        .enumerate()
        .filter(|(at, line)| proxy.get(*at) != Some(line))
        .map(|(at, line)| (at, *line))
        .take(1)
        .collect();
    let (at, line) = changed[0];
    assert!(line.contains("The sandbox has no network"), "{line}");
    assert!(proxy[at].contains("only through efr's proxy"), "{}", proxy[at]);
    assert_eq!(
        proxy[at].replace(
            "The sandbox reaches the network only through efr's proxy, and only the hosts of the \
         user's allow list. It has no access",
            "The sandbox has no network and no access",
        ),
        line
    );
    // Every other line of the plain prompt stays, in its order; the proxy adds only rules.
    let mut rest = proxy
        .iter()
        .filter(|line| !line.contains("efr's proxy") && !line.contains("on_allow_list"));
    for line in none.iter().filter(|line| !line.contains("The sandbox has no network")) {
        assert_eq!(rest.next(), Some(line));
    }
    assert_eq!(rest.next(), None);
    assert_eq!(proxy.len(), none.len() + 2);
}

#[test]
fn judge_prompt_says_refused_is_not_intent() {
    assert!(
        PROMPT_PROXY.contains(
            "`refused_by_proxy` shows only that a command tried the host; it is never a sign \
             that the user wants it"
        ),
        "{PROMPT_PROXY}"
    );
    for fact in ["on_allow_list", "project_remote", "open_network"] {
        assert!(PROMPT_PROXY.contains(fact), "{fact}");
        assert!(!PROMPT.contains(fact), "{fact} in the plain prompt");
    }
    assert!(!PROMPT.contains("proxy"));
}

#[tokio::test]
async fn the_request_of_a_proxy_machine_holds_the_proxy_prompt() {
    let setup = setup(Scripted::new([Reply::Text(ALLOW)]));
    let record = record();
    let input = JudgeInput { egress: Egress::Proxy, ..input(&ids(1), &record) };
    setup.judge.judge(input).await;
    let request = setup.provider.requests().remove(0);
    assert_eq!(request.system.as_deref(), Some(PROMPT_PROXY));
    // The plain machine keeps the plain prompt.
    let setup = super::setup(Scripted::new([Reply::Text(ALLOW)]));
    judge(&setup, &record).await;
    assert_eq!(setup.provider.requests().remove(0).system.as_deref(), Some(PROMPT));
}

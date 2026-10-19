//! efrd's proxy: the bind of a call's socket, its lifetime, the live list, the mirror
//! read and the refusal facts. The proxy runs with the test seams: a resolver that
//! refuses every name, so a listed host gets 502 and an unlisted one 403, and no test
//! reaches the network.

use std::os::fd::AsRawFd as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use efr_config::Settings;
use efr_permissions::HostList;
use efr_protocol::{BlockReason, CallId, ConversationId, NetworkMode, SandboxStatus};
use efr_proxy::CallPolicy;
use efr_stdx::time::Clock as _;
use efr_test_support::{TestClock, Wait};
use pretty_assertions::assert_eq;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixStream;

use super::mirrors::Mirrors;
use super::{
    FALLBACK_NOTE, PROXY_FALLBACK, ProxyService, bind_listener, env_keep_warnings, falls_back,
};
use crate::sandbox::Seams;

fn service(clock: &TestClock) -> ProxyService {
    ProxyService::new(&Seams::default(), clock.shared())
}

fn conversation(n: u128) -> ConversationId {
    ConversationId::from_uuid(uuid::Uuid::from_u128(n))
}

fn policy(n: u128, grants: &[(&str, u16)]) -> CallPolicy {
    CallPolicy {
        call: CallId::from_uuid(uuid::Uuid::from_u128(n)),
        grants: grants.iter().map(|(host, port)| ((*host).to_owned(), *port)).collect(),
        deny: Vec::new(),
        max_upload_bytes: 4 * 1024 * 1024,
    }
}

fn list(allow: &[&str]) -> HostList {
    let allow: Vec<String> = allow.iter().map(|host| (*host).to_owned()).collect();
    HostList::compile(&allow, &[], &[], &[]).unwrap()
}

/// A call dir below `root`; deep makes its socket path longer than a socket address.
fn call_dir(root: &Path, deep: bool) -> PathBuf {
    let dir = if deep { root.join("x".repeat(60)).join("y".repeat(60)) } else { root.join("call") };
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Connects to `socket` through its dir's descriptor, as a path of any length allows.
async fn connect(socket: &Path) -> std::io::Result<UnixStream> {
    let dir = std::fs::File::open(socket.parent().unwrap())?;
    let name = socket.file_name().unwrap().to_string_lossy();
    UnixStream::connect(format!("/proc/self/fd/{}/{name}", dir.as_raw_fd())).await
}

/// The status line of the proxy's answer to `CONNECT host:port`.
async fn status(socket: &Path, host: &str, port: u16) -> String {
    let mut stream = connect(socket).await.unwrap();
    let head = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n");
    stream.write_all(head.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    let mut byte = [0_u8; 1];
    while !answer.ends_with(b"\r\n") {
        if stream.read(&mut byte).await.unwrap() == 0 {
            break;
        }
        answer.push(byte[0]);
    }
    String::from_utf8_lossy(&answer).trim_end().to_owned()
}

#[test]
fn proxy_socket_binds_in_a_deep_call_dir() {
    let temp = tempfile::tempdir().unwrap();
    let dir = call_dir(temp.path(), true);
    let socket = dir.join("proxy.sock");
    assert!(socket.as_os_str().len() > 107, "{}", socket.display());
    let listener = bind_listener(&dir).unwrap();
    let meta = std::fs::symlink_metadata(&socket).unwrap();
    assert!(std::os::unix::fs::FileTypeExt::is_socket(&meta.file_type()));
    drop(listener);
}

#[test]
fn proxy_socket_is_0600() {
    let temp = tempfile::tempdir().unwrap();
    let dir = call_dir(temp.path(), false);
    let _listener = bind_listener(&dir).unwrap();
    let mode = std::fs::metadata(dir.join("proxy.sock")).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[tokio::test]
async fn proxy_grant_is_per_call_in_efrd() {
    let clock = TestClock::new();
    let proxy = service(&clock);
    let temp = tempfile::tempdir().unwrap();
    let (one, two) = (temp.path().join("one"), temp.path().join("two"));
    for dir in [&one, &two] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let granted = proxy.bind(conversation(1), &one, policy(1, &[("extra.test", 443)]), false).await;
    let other = proxy.bind(conversation(1), &two, policy(2, &[]), false).await;
    let (granted, other) = (granted.unwrap(), other.unwrap());
    // NOTE: the test seams resolve no name: a host that the call may reach gets 502.
    assert!(status(&one.join("proxy.sock"), "extra.test", 443).await.contains(" 502 "));
    assert!(status(&two.join("proxy.sock"), "extra.test", 443).await.contains(" 403 "));
    granted.close(Duration::from_millis(200)).await;
    other.close(Duration::from_millis(200)).await;
    assert!(connect(&one.join("proxy.sock")).await.is_err(), "the listener closed");
}

#[tokio::test]
async fn test_daemon_proxy_resolves_nothing_without_the_seam() {
    let clock = TestClock::new();
    let proxy = service(&clock);
    proxy.set_hosts(&list(&["registry.test"]));
    let temp = tempfile::tempdir().unwrap();
    let dir = call_dir(temp.path(), false);
    let call = proxy.bind(conversation(1), &dir, policy(1, &[]), false).await.unwrap();
    let answer = status(&dir.join("proxy.sock"), "registry.test", 443).await;
    assert!(answer.contains(" 502 "), "{answer}");
    let report = call.close(Duration::from_millis(200)).await;
    assert!(report.blocked.is_empty(), "an upstream failure is no refusal");
    assert_eq!(report.tunnels, 0, "no connection left");
}

#[tokio::test]
async fn reload_list_reaches_running_listener() {
    let clock = TestClock::new();
    let proxy = service(&clock);
    let temp = tempfile::tempdir().unwrap();
    let dir = call_dir(temp.path(), false);
    let call = proxy.bind(conversation(1), &dir, policy(1, &[]), true).await.unwrap();
    let socket = dir.join("proxy.sock");
    assert!(status(&socket, "late.test", 443).await.contains(" 403 "));
    proxy.set_hosts(&list(&["late.test"]));
    assert_eq!(*proxy.hosts(), list(&["late.test"]));
    assert!(status(&socket, "late.test", 443).await.contains(" 502 "), "the new list holds");
    drop(call);
}

#[tokio::test]
async fn proxy_socket_closed_at_call_end() {
    let clock = TestClock::new();
    let proxy = service(&clock);
    let temp = tempfile::tempdir().unwrap();
    let dir = call_dir(temp.path(), false);
    let call = proxy.bind(conversation(1), &dir, policy(1, &[]), false).await.unwrap();
    assert!(connect(&dir.join("proxy.sock")).await.is_ok());
    drop(call);
    // NOTE: the abort of the accept task runs on the runtime; the listener goes with it.
    let socket = dir.join("proxy.sock");
    Wait::new("the listener of a dropped call to close")
        .until_some_async(async || connect(&socket).await.is_err().then_some(()))
        .await
        .unwrap();
    assert_eq!(proxy.held(), 0);
}

#[tokio::test]
async fn sweep_closes_timed_out_call_listener() {
    let clock = TestClock::new();
    let proxy = service(&clock);
    let temp = tempfile::tempdir().unwrap();
    let dir = call_dir(temp.path(), false);
    let call = proxy.bind(conversation(1), &dir, policy(1, &[]), false).await.unwrap();
    proxy.keep_timed_out(call, dir.clone());
    assert_eq!(proxy.sweep().await, 0, "no result yet: the call runs on");
    assert_eq!(proxy.held(), 1);
    assert!(status(&dir.join("proxy.sock"), "extra.test", 443).await.contains(" 403 "));
    std::fs::write(dir.join("result.json"), b"{}").unwrap();
    assert_eq!(crate::sandbox::gc::sweep_proxies(&proxy).await, 1);
    assert_eq!(proxy.held(), 0, "the registry is empty");
    assert!(connect(&dir.join("proxy.sock")).await.is_err());
}

#[tokio::test]
async fn proxy_socket_of_a_timed_out_call_stays_until_result() {
    let clock = TestClock::new();
    let proxy = service(&clock);
    let temp = tempfile::tempdir().unwrap();
    let dir = call_dir(temp.path(), false);
    let call = proxy.bind(conversation(1), &dir, policy(1, &[]), false).await.unwrap();
    proxy.keep_timed_out(call, dir.clone());
    assert!(connect(&dir.join("proxy.sock")).await.is_ok(), "the network stays");
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(proxy.sweep().await, 1, "a call dir that went ends the listener");
    assert_eq!(proxy.held(), 0);
}

#[tokio::test]
async fn refusals_of_the_last_call_and_the_jobs_are_facts() {
    let clock = TestClock::new();
    let proxy = service(&clock);
    let temp = tempfile::tempdir().unwrap();
    let (call_one, job_one) = (temp.path().join("call"), temp.path().join("job"));
    for dir in [&call_one, &job_one] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let start = clock.now();
    let job = proxy.bind(conversation(1), &job_one, policy(1, &[]), true).await.unwrap();
    assert!(status(&job_one.join("proxy.sock"), "job.test", 443).await.contains(" 403 "));
    assert_eq!(proxy.refused_hosts(conversation(1), start), ["job.test"], "a running job");
    let call = proxy.bind(conversation(1), &call_one, policy(2, &[]), false).await.unwrap();
    assert!(status(&call_one.join("proxy.sock"), "call.test", 443).await.contains(" 403 "));
    let report = call.close(Duration::from_millis(200)).await;
    assert_eq!(report.blocked[0].reason, BlockReason::NotAllowed);
    assert_eq!(proxy.refused_hosts(conversation(1), start), ["call.test", "job.test"]);
    assert!(proxy.refused_hosts(conversation(2), start).is_empty(), "per conversation");
    job.close(Duration::from_millis(200)).await;
    assert_eq!(proxy.refused_hosts(conversation(1), start), ["call.test", "job.test"]);
    clock.advance(Duration::from_secs(60));
    let later = clock.now();
    assert_eq!(
        proxy.refused_hosts(conversation(1), later),
        ["call.test", "job.test"],
        "a job that ended after the last call is a fact of the turn that its end woke"
    );
    clock.advance(Duration::from_secs(1));
    let call_two = temp.path().join("call-two");
    std::fs::create_dir_all(&call_two).unwrap();
    let again = proxy.bind(conversation(1), &call_two, policy(3, &[]), false).await.unwrap();
    again.close(Duration::from_millis(200)).await;
    assert!(
        proxy.refused_hosts(conversation(1), later).is_empty(),
        "a later call ends the facts of the earlier call and of a job that ended before it"
    );
}

#[test]
fn mirrorlist_mtime_rereads() {
    let temp = tempfile::tempdir().unwrap();
    let conf = temp.path().join("pacman.conf");
    let list = temp.path().join("mirrorlist");
    let extra = temp.path().join("extra-mirrors");
    std::fs::write(&conf, format!("[core]\nInclude = {}\n", extra.display())).unwrap();
    std::fs::write(&list, "Server = https://one.test/$repo/os/$arch\n").unwrap();
    std::fs::write(&extra, "Server = http://two.test/$repo\n").unwrap();
    let clock = TestClock::new();
    let mut mirrors = Mirrors::new(conf, list.clone());
    let start = clock.now();
    let hosts = mirrors.refresh(start, false).unwrap();
    let names: Vec<(&str, u16, bool)> =
        hosts.iter().map(|mirror| (mirror.host.as_str(), mirror.port, mirror.http)).collect();
    assert_eq!(names, [("one.test", 443, false), ("two.test", 80, true)], "an include too");
    std::fs::write(&list, "Server = https://three.test/$repo/os/$arch\n").unwrap();
    clock.advance(Duration::from_secs(30));
    assert_eq!(mirrors.refresh(clock.now(), false), None, "at most once a minute");
    clock.advance(Duration::from_secs(31));
    let hosts = mirrors.refresh(clock.now(), false).unwrap();
    assert!(hosts.iter().any(|mirror| mirror.host == "three.test"), "{hosts:?}");
    clock.advance(Duration::from_secs(61));
    assert_eq!(mirrors.refresh(clock.now(), false), None, "no file changed");
    assert!(mirrors.refresh(clock.now(), true).is_some(), "a reload reads them");
}

#[tokio::test]
async fn missing_pacman_files_give_no_mirror() {
    let temp = tempfile::tempdir().unwrap();
    let mut mirrors = Mirrors::new(temp.path().join("none.conf"), temp.path().join("none"));
    let clock = TestClock::new();
    assert_eq!(mirrors.refresh(clock.now(), true), Some(Vec::new()));
    let proxy = service(&clock);
    assert!(!proxy.read_mirrors(true).await, "the test seams read /dev/null: no host");
    assert!(proxy.mirrors().is_empty());
}

#[test]
fn env_keep_secret_warns_under_proxy() {
    let mut settings = Settings::default();
    settings.network.mode = NetworkMode::None;
    settings.sandbox.env_keep = vec!["GITHUB_TOKEN".to_owned(), "EDITOR".to_owned()];
    assert!(env_keep_warnings(&settings).is_empty(), "no proxy, no warning");
    settings.network.mode = NetworkMode::Proxy;
    let warnings = env_keep_warnings(&settings);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("GITHUB_TOKEN"), "{warnings:?}");
}

#[test]
fn fallback_notice_once_per_start() {
    let clock = TestClock::new();
    let proxy = service(&clock);
    let mut settings = Settings::default();
    settings.network.mode = NetworkMode::None;
    let failed = SandboxStatus { available: true, ..SandboxStatus::unavailable("x") };
    assert_eq!(proxy.fallback_note(&settings, &failed), None, "network.mode = none");
    settings.network.mode = NetworkMode::Proxy;
    assert!(falls_back(&settings, &failed));
    assert_eq!(proxy.fallback_note(&settings, &failed), Some(FALLBACK_NOTE));
    assert_eq!(proxy.fallback_note(&settings, &failed), None, "once per efrd start");
    let passed = SandboxStatus { proxy: true, ..failed };
    assert!(!falls_back(&settings, &passed));
    assert!(PROXY_FALLBACK.contains("self-test failed"));
    let restarted = service(&clock);
    assert_eq!(
        restarted.fallback_note(&settings, &SandboxStatus { proxy: false, ..passed }),
        Some(FALLBACK_NOTE)
    );
}

#[test]
fn the_proxy_service_holds_nothing_at_start() {
    let clock = TestClock::new();
    let proxy = service(&clock);
    assert_eq!(proxy.held(), 0);
    assert_eq!(*proxy.hosts(), HostList::default(), "an empty list allows nothing");
}

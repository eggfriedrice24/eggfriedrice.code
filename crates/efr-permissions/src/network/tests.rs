use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use proptest::prelude::*;
use rstest::rstest;

use super::{
    HostError, HostList, HostRule, HostTable, HostVerdict, MirrorHost, PacmanFile, Preset,
    host_and_port, is_ip_literal, parse_pacman,
};
use crate::exits::{Ecosystem, REGISTRY_HOSTS};

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|word| (*word).to_owned()).collect()
}

fn allow(list: &[&str]) -> Result<HostList, HostError> {
    HostList::compile(&words(list), &[], &[], &[])
}

fn table(host: &str) -> HostTable<'_> {
    HostTable { host, port: None, http: false, private: false }
}

const PLAIN: HostVerdict = HostVerdict::Allowed { http: false, private: false };

#[test]
fn host_list_refuses_bare_star() {
    assert_eq!(allow(&["*"]), Err(HostError::BareStar));
    assert_eq!(allow(&["*."]), Err(HostError::BareStar), "the trailing dot goes first");
    let deny = HostList::compile(&[], &words(&["*"]), &[], &[]);
    assert_eq!(deny, Err(HostError::BareStar));
    assert_eq!(HostList::compile(&[], &[], &[table("*")], &[]), Err(HostError::BareStar));
}

#[test]
fn host_list_refuses_one_label_suffix() {
    assert_eq!(allow(&["*.com"]), Err(HostError::ShortSuffix("*.com".to_owned())));
    assert_eq!(allow(&["*.com."]), Err(HostError::ShortSuffix("*.com.".to_owned())));
    assert!(allow(&["*.example.com"]).is_ok());
    assert!(allow(&["*.EXAMPLE.com."]).is_ok(), "case and one trailing dot do not matter");
    for bad in ["a.*.com", "*example.com", "**.example.com", "*.*.example.com"] {
        assert_eq!(allow(&[bad]), Err(HostError::BadName(bad.to_owned())), "{bad}");
    }
}

#[test]
fn host_list_refuses_non_ascii() {
    for entry in ["bücher.example", "*.bücher.example", "@crätes"] {
        let error = allow(&[entry]).unwrap_err();
        assert_eq!(error, HostError::NotAscii(entry.to_owned()));
        assert!(error.to_string().contains("xn--"), "{error}");
    }
    assert!(allow(&["xn--bcher-kva.example"]).is_ok());
}

#[test]
fn host_list_refuses_ip_literal_outside_hosts() {
    for entry in ["1.1.1.1", "[::1]", "2130706433", "127.1", "0x7f.0.0.1", "*.10.0", "::1"] {
        assert_eq!(allow(&[entry]), Err(HostError::IpLiteral(entry.to_owned())), "{entry}");
        let deny = HostList::compile(&[], &words(&[entry]), &[], &[]);
        assert_eq!(deny, Err(HostError::IpLiteral(entry.to_owned())), "{entry}");
    }
}

#[test]
fn host_list_accepts_listed_ip_literal() {
    let hosts = [
        HostTable { host: "192.168.1.5", port: Some(8080), http: true, private: true },
        HostTable { host: "[FD00::0005]", port: None, http: false, private: true },
    ];
    let list = HostList::compile(&[], &[], &hosts, &[]).unwrap();
    let lan = HostVerdict::Allowed { http: true, private: true };
    assert_eq!(list.decide("192.168.1.5", 8080), lan);
    assert_eq!(list.decide("192.168.1.5", 443), HostVerdict::NotListed, "only its own port");
    assert_eq!(list.decide("fd00::5", 443), HostVerdict::Allowed { http: false, private: true });
    assert_eq!(list.decide("[fd00:0::5]", 443), list.decide("fd00::5", 443), "canonical form");
    let odd = HostList::compile(&[], &[], &[table("0x7f.0.0.1")], &[]);
    assert_eq!(odd, Err(HostError::BadName("0x7f.0.0.1".to_owned())), "only the standard form");
    let zero = HostTable { host: "mirror.example", port: Some(0), http: false, private: false };
    let zero = HostList::compile(&[], &[], &[zero], &[]);
    assert_eq!(zero, Err(HostError::BadPort("mirror.example".to_owned())));
    let preset = HostList::compile(&[], &[], &[table("@crates")], &[]);
    assert_eq!(preset, Err(HostError::BadName("@crates".to_owned())), "a table names a host");
}

#[test]
fn host_list_deny_wins_over_preset() {
    let list = HostList::compile(
        &words(&["@github-read", "@crates", "*.example.org"]),
        &words(&["raw.githubusercontent.com", "*.static.crates.io", "@go"]),
        &[HostTable { host: "files.example.org", port: None, http: true, private: false }],
        &[],
    )
    .unwrap();
    assert_eq!(list.decide("github.com", 443), PLAIN);
    assert_eq!(list.decide("raw.githubusercontent.com", 443), HostVerdict::Denied);
    assert_eq!(list.decide("raw.githubusercontent.com", 80), HostVerdict::Denied, "any port");
    assert_eq!(list.decide("static.crates.io", 443), PLAIN, "a suffix denies only below");
    assert_eq!(list.decide("a.static.crates.io", 443), HostVerdict::Denied);
    assert!(list.denies("proxy.golang.org") && list.denies("SUM.golang.org."));
    let denied = HostList::compile(
        &[],
        &words(&["files.example.org"]),
        &[HostTable { host: "files.example.org", port: Some(8443), http: false, private: true }],
        &[],
    )
    .unwrap();
    assert_eq!(denied.decide("files.example.org", 8443), HostVerdict::Denied, "over a table");
}

#[test]
fn host_list_port_rules() {
    let list = HostList::compile(
        &words(&["plain.example"]),
        &[],
        &[
            HostTable { host: "mirror.example", port: None, http: true, private: false },
            HostTable { host: "git.example", port: Some(8443), http: false, private: false },
        ],
        &[],
    )
    .unwrap();
    assert_eq!(list.decide("plain.example", 443), PLAIN);
    assert_eq!(list.decide("plain.example", 80), HostVerdict::NotListed, "80 only with http");
    let http = HostVerdict::Allowed { http: true, private: false };
    assert_eq!(list.decide("mirror.example", 443), http);
    assert_eq!(list.decide("mirror.example", 80), http);
    assert_eq!(list.decide("mirror.example", 8080), HostVerdict::NotListed);
    assert_eq!(list.decide("git.example", 8443), PLAIN);
    assert_eq!(list.decide("git.example", 443), HostVerdict::NotListed, "only its own port");
}

#[test]
fn host_list_merges_the_entries_of_one_host() {
    let list = HostList::compile(
        &words(&["@github-read"]),
        &[],
        &[HostTable { host: "github.com", port: None, http: false, private: true }],
        &[],
    )
    .unwrap();
    assert_eq!(list.decide("github.com", 443), HostVerdict::Allowed { http: false, private: true });
}

#[test]
fn host_list_suffix_matches_below_only() {
    let list = allow(&["*.example.com"]).unwrap();
    assert_eq!(list.decide("a.example.com", 443), PLAIN);
    assert_eq!(list.decide("b.a.example.com", 443), PLAIN);
    assert_eq!(list.decide("A.Example.COM.", 443), PLAIN, "lower case, one trailing dot");
    assert_eq!(list.decide("example.com", 443), HostVerdict::NotListed);
    assert_eq!(list.decide("badexample.com", 443), HostVerdict::NotListed);
    assert_eq!(list.decide(".example.com", 443), HostVerdict::NotListed);
    let rule = HostRule::Suffix("example.com".to_owned());
    assert!(rule.matches("a.example.com") && !rule.matches("example.com"));
    assert!(HostRule::Exact("example.com".to_owned()).matches("example.com"));
}

#[test]
fn host_list_allows_needs_the_port() {
    let list = allow(&["@github-read"]).unwrap();
    assert!(list.allows("github.com", 443));
    assert!(!list.allows("github.com", 22));
    assert!(!list.allows("api.github.com", 443), "no api.github.com: it takes writes");
    assert!(list.allows("release-assets.githubusercontent.com", 443));
}

#[test]
fn host_list_compares_by_content() {
    let one = HostList::compile(
        &words(&["@npm", "a.example", "*.b.example"]),
        &words(&["x.example", "y.example"]),
        &[table("c.example")],
        &[],
    )
    .unwrap();
    let other = HostList::compile(
        &words(&["*.b.example", "A.example.", "@npm", "a.example"]),
        &words(&["y.example", "x.example"]),
        &[table("c.example")],
        &[],
    )
    .unwrap();
    assert_eq!(one, other);
    assert_ne!(one, allow(&["@npm"]).unwrap());
    let empty = HostList::default();
    assert_eq!(empty.decide("crates.io", 443), HostVerdict::NotListed);
    assert!(!empty.denies("crates.io"));
}

#[test]
fn host_list_takes_the_mirror_hosts_for_the_pacman_preset() {
    let mirrors = [
        MirrorHost { host: "mirror.example.org".to_owned(), port: 443, http: false },
        MirrorHost { host: "old.example.org".to_owned(), port: 80, http: true },
        MirrorHost { host: "10.0.0.5".to_owned(), port: 80, http: true },
    ];
    let without = HostList::compile(&words(&["@crates"]), &[], &[], &mirrors).unwrap();
    assert_eq!(without.decide("mirror.example.org", 443), HostVerdict::NotListed);
    let list = HostList::compile(&words(&["@pacman-mirrors"]), &[], &[], &mirrors).unwrap();
    assert_eq!(list.decide("mirror.example.org", 443), PLAIN);
    assert_eq!(list.decide("mirror.example.org", 80), HostVerdict::NotListed);
    let http = HostVerdict::Allowed { http: true, private: false };
    assert_eq!(list.decide("old.example.org", 80), http);
    assert_eq!(list.decide("old.example.org", 443), HostVerdict::NotListed);
    assert_eq!(list.decide("10.0.0.5", 80), HostVerdict::NotListed, "a mirror is a name");
    let denied = HostList::compile(
        &words(&["@pacman-mirrors"]),
        &words(&["@pacman-mirrors"]),
        &[],
        &mirrors,
    )
    .unwrap();
    assert_eq!(denied.decide("old.example.org", 80), HostVerdict::Denied);
    assert!(Preset::PacmanMirrors.hosts().is_empty());
}

#[test]
fn unknown_preset_refused() {
    assert_eq!(allow(&["@rubygems"]), Err(HostError::UnknownPreset("@rubygems".to_owned())));
    assert_eq!(allow(&["@"]), Err(HostError::UnknownPreset("@".to_owned())));
    assert!(allow(&["@CRATES"]).is_ok(), "compared in lower case");
    for preset in Preset::ALL {
        assert_eq!(Preset::parse(preset.name()), Some(preset));
        assert!(preset.name().starts_with('@'));
    }
    assert_eq!(Preset::parse("crates"), None, "the @ is part of the name");
}

#[test]
fn presets_equal_registry_hosts() {
    let preset = |ecosystem| match ecosystem {
        Ecosystem::Crates => Preset::Crates,
        Ecosystem::Npm => Preset::Npm,
        Ecosystem::Pypi => Preset::Pypi,
        Ecosystem::Go => Preset::Go,
        other => panic!("{other:?} has no preset of fixed hosts"),
    };
    assert_eq!(REGISTRY_HOSTS.len(), 4);
    for (ecosystem, hosts) in REGISTRY_HOSTS {
        assert_eq!(*hosts, preset(*ecosystem).hosts(), "{ecosystem:?}");
    }
    for preset in Preset::ALL {
        for host in preset.hosts() {
            assert_eq!(host_and_port(host), Some(((*host).to_owned(), 443)), "{host}");
            assert!(!host.contains('*'), "no preset holds a wildcard: {host}");
        }
    }
}

#[rstest]
#[case::dotted("1.1.1.1")]
#[case::bracketed_v6("[::1]")]
#[case::global_v6("[2606:4700::1111]")]
#[case::bare_number("2130706433")]
#[case::short("127.1")]
#[case::hex_part("0x7f.0.0.1")]
#[case::octal_part("0177.0.0.1")]
#[case::doc_v6("[2001:db8::1]")]
#[case::hex_number("0x7f000001")]
#[case::bare_v6("2001:db8::1")]
#[case::trailing_dot("10.0.0.1.")]
#[case::digit_tld("x.example.123")]
fn is_ip_literal_forms(#[case] host: &str) {
    assert!(is_ip_literal(host), "{host}");
}

#[rstest]
#[case::name("example.com")]
#[case::digit_in_label("x1.example")]
#[case::digit_label_first("123.example")]
#[case::empty("")]
#[case::hex_label_inside("0x1.example")]
fn is_ip_literal_names(#[case] host: &str) {
    assert!(!is_ip_literal(host), "{host}");
}

#[test]
fn host_and_port_http_gives_80() {
    let pair = |host: &str, port| Some((host.to_owned(), port));
    assert_eq!(host_and_port("http://mirror.example/arch"), pair("mirror.example", 80));
    assert_eq!(host_and_port("HTTP://Mirror.Example"), pair("mirror.example", 80));
    assert_eq!(host_and_port("http://mirror.example:8080/x"), pair("mirror.example", 8080));
    assert_eq!(host_and_port("https://example.com"), pair("example.com", 443));
    assert_eq!(host_and_port("example.com"), pair("example.com", 443));
    assert_eq!(host_and_port("git://example.com/r"), pair("example.com", 443));
    assert_eq!(host_and_port("https://User:pw@Example.com:8443/p?q#f"), pair("example.com", 8443));
    assert_eq!(host_and_port("example.com."), pair("example.com", 443));
    assert_eq!(host_and_port("[::1]:80"), pair("::1", 80));
    assert_eq!(host_and_port("[0:0::1]"), pair("::1", 443));
    assert_eq!(host_and_port("db_1:5432"), pair("db_1", 5432), "a container name");
    assert_eq!(host_and_port("localhost:3000"), pair("localhost", 3000));
    assert_eq!(host_and_port("192.168.1.5:22"), pair("192.168.1.5", 22));
    assert_eq!(host_and_port("[fe80::0001%eth0]:22"), pair("fe80::1%eth0", 22), "a zone stays");
    assert_eq!(host_and_port("[fe80::1%eth 0]"), None);
    assert_eq!(host_and_port("[fe80::1%]"), None);
}

#[rstest]
#[case::non_ascii("bücher.example")]
#[case::non_ascii_url("https://bücher.example/x")]
#[case::empty("")]
#[case::only_scheme("https://")]
#[case::bad_port("example.com:http")]
#[case::port_zero("example.com:0")]
#[case::port_too_big("example.com:65536")]
#[case::empty_label("a..example")]
#[case::two_trailing_dots("example.com..")]
#[case::star("*.example.com")]
#[case::variable("$HOST")]
#[case::space("exa mple.com")]
#[case::quote("a\"b.example")]
#[case::bad_v6("[not:v6]")]
#[case::v6_junk_after("[::1]x")]
fn host_and_port_refuses_non_ascii(#[case] word: &str) {
    assert_eq!(host_and_port(word), None, "{word:?}");
}

#[test]
fn host_and_port_holds_the_length_limits() {
    let label = "a".repeat(63);
    assert!(host_and_port(&format!("{label}.example")).is_some());
    assert_eq!(host_and_port(&format!("a{label}.example")), None, "a label of 64 bytes");
    let name = [label.as_str(); 4].join(".");
    assert_eq!(name.len(), 255);
    assert_eq!(host_and_port(&name), None, "a name over 253 bytes");
    assert!(host_and_port(&name[..253]).is_some_and(|(host, _)| host.len() == 253));
}

#[test]
fn parse_pacman_server_lines() {
    let text = "\
## Arch Linux repository mirrorlist
## Generated on 2026-10-01

## Germany
Server = https://mirror.example.org/archlinux/$repo/os/$arch
#Server = https://commented.example.org/archlinux/$repo/os/$arch
Server=http://Old.Example.org/arch/$repo/os/$arch # a slow one
Server = https://mirror.example.org/again/$repo/os/$arch
Server = https://ports.example.net:8443/arch/$repo
Server = file:///srv/mirror/$repo/os/$arch
Server = ftp://ftp.example.org/arch/$repo
Server = https://10.0.0.5/arch/$repo
Server = https://user:pw@private.example.org/arch
Server = https://$repo.example.org/
Server = not a url
server = https://lower.example.org/
";
    let file = parse_pacman(text);
    let mirror = |host: &str, port, http| MirrorHost { host: host.to_owned(), port, http };
    assert_eq!(
        file.mirrors,
        [
            mirror("mirror.example.org", 443, false),
            mirror("old.example.org", 80, true),
            mirror("ports.example.net", 8443, false),
        ]
    );
    assert!(file.includes.is_empty());
}

#[test]
fn parse_pacman_includes() {
    let text = "\
[options]
HoldPkg = pacman glibc
Architecture = auto

[core]
Include = /etc/pacman.d/mirrorlist

[extra]
Include = /etc/pacman.d/mirrorlist

#[testing]
#Include = /etc/pacman.d/testing-mirrorlist

[chaotic-aur]
Include = /etc/pacman.d/chaotic-mirrorlist
Server = https://cdn.example.com/$repo/$arch
Include = relative/mirrorlist
";
    let file = parse_pacman(text);
    assert_eq!(
        file.includes,
        [
            PathBuf::from("/etc/pacman.d/mirrorlist"),
            PathBuf::from("/etc/pacman.d/chaotic-mirrorlist")
        ]
    );
    assert_eq!(file.mirrors.len(), 1);
    assert_eq!(parse_pacman(""), PacmanFile::default());
}

#[test]
fn parse_pacman_reads_at_most_max_lines() {
    let mut text = "# padding\n".repeat(PacmanFile::MAX_LINES - 1);
    text.push_str("Server = https://last.example.org/\n");
    text.push_str("Server = https://late.example.org/\n");
    let hosts: Vec<String> = parse_pacman(&text).mirrors.into_iter().map(|m| m.host).collect();
    assert_eq!(hosts, ["last.example.org"]);
}

/// The engine keeps the host list. With no proxy (`Egress::None`) neither the list nor
/// the refused host facts change a decision; the remote facts only add the hosts behind
/// the remote names (phase D2).
#[test]
fn engine_keeps_the_host_list_and_decides_as_before() {
    use std::sync::Arc;

    use efr_protocol::{Mode, Origin, Scope};

    use crate::{
        CallFacts, ConversationPolicy, DecisionInput, Engine, FactRequest, Locations, RemoteFact,
        RemoteUrl, Requirements,
    };

    let locations = Locations::new("/home/u").unwrap();
    let plain = Engine::with_defaults(locations.clone());
    assert_eq!(plain.hosts(), &HostList::default(), "the default list is empty");
    let list = allow(&["@crates", "@github-read"]).unwrap();
    let engine = Engine::with_defaults(locations.clone()).with_hosts(Arc::new(list.clone()));
    assert_eq!(engine.hosts(), &list);

    assert!(CallFacts::default().remotes.is_empty());
    assert!(CallFacts::default().refused_hosts.is_empty());
    assert!(FactRequest::default().remotes.is_empty());
    let url = |host: &str, ssh: bool| RemoteUrl {
        host: host.to_owned(),
        port: 443,
        ssh,
        credentials: false,
    };
    let remote = RemoteFact {
        name: "origin".to_owned(),
        fetch: vec![url("github.com", false)],
        push: vec![url("github.com", true)],
        project: true,
        upstream: true,
        submodule: false,
    };
    let refused =
        CallFacts { refused_hosts: vec!["evil.example".to_owned()], ..CallFacts::default() };
    let facts =
        CallFacts { remotes: vec![(PathBuf::from("/home/u/p"), vec![remote])], ..refused.clone() };
    let input = |requirements: Requirements| DecisionInput {
        requirements,
        scope: Scope::Machine,
        origin: Origin::Shell,
        mode: Mode::Auto,
        conversation_policy: ConversationPolicy::new("/home/u/.local/share/efr/scratch/c"),
    };
    for line in ["git fetch", "git push", "curl -sSf https://index.crates.io/config.json"] {
        let requirements = Requirements::none().with_command(line).with_command_dir("/home/u/p");
        let request = crate::exits::fact_requests(line, Some(Path::new("/home/u/p")), &locations);
        assert_eq!(request.remotes.is_empty(), !line.starts_with("git"), "{line}");
        // NOTE: both with facts: a git push with facts that name no host is for the user.
        let before = plain.decide(&input(requirements.clone().with_facts(CallFacts::default())));
        let after = engine.decide(&input(requirements.clone().with_facts(refused.clone())));
        assert_eq!(before, after, "{line}");
        let remotes = engine.decide(&input(requirements.with_facts(facts.clone())));
        let hosts: Vec<&str> =
            remotes.exits().flat_map(|need| need.hosts.iter().map(String::as_str)).collect();
        if line.starts_with("git") {
            assert_eq!(hosts, ["github.com"], "{line}");
        }
    }
}

proptest! {
    #[test]
    fn host_words_never_panic_and_stay_canonical(word in "\\PC{0,40}") {
        let _ = is_ip_literal(&word);
        if let Some((host, port)) = host_and_port(&word) {
            prop_assert!(port != 0);
            prop_assert!(host.is_ascii());
            if !host.contains('%') {
                prop_assert_eq!(host.to_ascii_lowercase(), host.clone());
                prop_assert!(!host.ends_with('.'));
            }
        }
        let entries = std::slice::from_ref(&word);
        let _ = HostList::compile(entries, entries, &[table(&word)], &[]);
        let _ = parse_pacman(&word);
    }

    #[test]
    fn a_suffix_never_matches_itself(label in "[a-z0-9]{1,10}", suffix in "[a-z]{1,8}\\.[a-z]{2,6}") {
        let entry = format!("*.{suffix}");
        let list = allow(&[entry.as_str()]).unwrap();
        prop_assert_eq!(list.decide(&suffix, 443), HostVerdict::NotListed);
        prop_assert_eq!(list.decide(&format!("{label}.{suffix}"), 443), PLAIN);
        prop_assert_eq!(list.decide(&format!("{label}{suffix}"), 443), HostVerdict::NotListed);
    }
}

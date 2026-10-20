use pretty_assertions::assert_eq;

use super::{
    GitTokens, Helper, HelperKind, HelperScope, TokenFile, git_tokens, helper_kind, token_hosts,
};

fn entries(list: &[(&str, &str)]) -> Vec<(String, String, String)> {
    list.iter()
        .map(|(key, value)| ("file:.git/config".to_owned(), (*key).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn token_hosts_npmrc_scoped_and_default() {
    let npmrc = "\
; the project's registries
registry=https://registry.npmjs.org/
@acme:registry=https://npm.acme.test/
//npm.acme.test/:_authToken=${ACME_TOKEN}
always-auth=true
";
    assert_eq!(
        token_hosts(TokenFile::Npmrc, npmrc),
        ["npm.acme.test", "registry.npmjs.org", "registry.yarnpkg.com"]
    );
    let legacy = "_auth=dXNlcjpwYXNz\nemail=a@b.test\n";
    assert_eq!(
        token_hosts(TokenFile::Npmrc, legacy),
        ["registry.npmjs.org", "registry.yarnpkg.com"]
    );
    let password = "//npm.acme.test/:username=u\n//npm.acme.test/:_password=cGFzcw==\n";
    assert_eq!(
        token_hosts(TokenFile::Npmrc, password),
        ["npm.acme.test", "registry.npmjs.org", "registry.yarnpkg.com"]
    );
}

#[test]
fn token_hosts_yarnrc_yml() {
    let yml = "\
npmRegistryServer: \"https://npm.acme.test\"
npmScopes:
  acme:
    npmRegistryServer: \"https://scope.acme.test\"
    npmAuthToken: \"${ACME_TOKEN}\"
";
    assert_eq!(
        token_hosts(TokenFile::YarnrcYml, yml),
        ["npm.acme.test", "registry.npmjs.org", "registry.yarnpkg.com", "scope.acme.test"]
    );
    let ident = "npmAlwaysAuth: true\nnpmAuthIdent: \"u:p\"\n";
    assert_eq!(
        token_hosts(TokenFile::YarnrcYml, ident),
        ["registry.npmjs.org", "registry.yarnpkg.com"]
    );
    let registries = "npmRegistries:\n  \"//npm.acme.test\":\n    npmAuthToken: x\n";
    assert_eq!(token_hosts(TokenFile::YarnrcYml, registries)[0], "npm.acme.test");
}

#[test]
fn token_hosts_yarnrc() {
    let yarnrc = "registry \"https://npm.acme.test/\"\n\"//npm.acme.test/:_authToken\" \"abc\"\n";
    assert_eq!(
        token_hosts(TokenFile::Yarnrc, yarnrc),
        ["npm.acme.test", "registry.npmjs.org", "registry.yarnpkg.com"]
    );
    assert!(token_hosts(TokenFile::Yarnrc, "registry \"https://npm.acme.test/\"\n").is_empty());
}

#[test]
fn token_hosts_bunfig() {
    let table = "\
[install]
registry = \"https://registry.npmjs.org\"

[install.scopes]
acme = { url = \"https://npm.acme.test/\", token = \"$ACME_TOKEN\" }
";
    assert_eq!(
        token_hosts(TokenFile::Bunfig, table),
        ["npm.acme.test", "registry.npmjs.org", "registry.yarnpkg.com"]
    );
    let url = "[install]\nregistry = \"https://u:secret@npm.acme.test/\"\n";
    assert_eq!(
        token_hosts(TokenFile::Bunfig, url),
        ["npm.acme.test", "registry.npmjs.org", "registry.yarnpkg.com"],
        "a password in a URL is a credential too"
    );
}

#[test]
fn token_hosts_cargo_registry_token() {
    let config = "\
[registries.acme]
index = \"sparse+https://cargo.acme.test/index/\"
token = \"cargo_token\"

[source.crates-io]
replace-with = \"acme\"
";
    assert_eq!(
        token_hosts(TokenFile::CargoConfig, config),
        ["cargo.acme.test", "crates.io", "index.crates.io", "static.crates.io"]
    );
    let registry = "[registry]\ntoken = \"t\"\n";
    assert_eq!(
        token_hosts(TokenFile::CargoConfig, registry),
        ["crates.io", "index.crates.io", "static.crates.io"]
    );
}

#[test]
fn token_hosts_none_without_token_key() {
    let cargo = "\
# token = \"a comment\"
[registries.acme]
index = \"sparse+https://cargo.acme.test/index/\"
credential-provider = \"cargo:token\"
[http]
cainfo = \"/etc/ssl/acme.pem\"
";
    assert!(token_hosts(TokenFile::CargoConfig, cargo).is_empty());
    let npmrc = "registry=https://npm.acme.test/\nauth-type=web\nmy_token_file=/x\n";
    assert!(token_hosts(TokenFile::Npmrc, npmrc).is_empty());
    let yml = "npmRegistryServer: \"https://npm.acme.test\"\nenableTelemetry: false\n";
    assert!(token_hosts(TokenFile::YarnrcYml, yml).is_empty());
    assert!(token_hosts(TokenFile::Bunfig, "").is_empty());
}

#[test]
fn token_hosts_reads_at_most_max_bytes() {
    let mut text = "#".repeat(TokenFile::MAX_BYTES - 1);
    text.push('\n');
    text.push_str("//npm.acme.test/:_authToken=x\n");
    assert!(token_hosts(TokenFile::Npmrc, &text).is_empty());
    let mut multibyte = "é".repeat(TokenFile::MAX_BYTES / 2 + 1);
    multibyte.insert_str(0, "_authToken=x\n");
    assert_eq!(token_hosts(TokenFile::Npmrc, &multibyte).len(), 2, "cut on a char boundary");
}

#[test]
fn token_files_name_every_file_that_efrd_reads() {
    let names: Vec<&str> = TokenFile::FILES.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        names,
        [".npmrc", ".yarnrc.yml", ".yarnrc", "bunfig.toml", ".cargo/config.toml", ".cargo/config"]
    );
}

#[test]
fn git_tokens_insteadof_with_userinfo() {
    let found = git_tokens(&entries(&[
        ("url.https://u:tok@git.test/.insteadof", "https://git.test/"),
        ("url.https://mirror.test/.insteadOf", "https://github.com/"),
        ("url.git@ssh.test:.insteadof", "https://ssh.test/"),
        ("remote.origin.url", "https://ghp_x@GitHub.com/o/r.git"),
        ("remote.origin.pushurl", "https://push.test/o/r.git"),
        ("remote.backup.url", "git@backup.test:o/r.git"),
        ("remote.ssh.url", "ssh://git@ssh2.test/o/r.git"),
        ("submodule.lib.url", "https://u:p@sub.test/lib.git"),
        ("branch.main.remote", "origin"),
    ]));
    assert_eq!(found.hosts, ["git.test", "github.com", "sub.test"]);
    assert!(!found.every_remote && !found.github_read);
}

#[test]
fn git_tokens_scoped_extraheader() {
    let found = git_tokens(&entries(&[
        ("http.https://git.test/.extraheader", "AUTHORIZATION: bearer x"),
        ("http.https://empty.test/.extraheader", ""),
        ("http.https://git.test/.sslverify", "false"),
    ]));
    assert_eq!(found, GitTokens { hosts: vec!["git.test".to_owned()], ..GitTokens::default() });
    let wildcard = git_tokens(&entries(&[("http.https://*.git.test.extraheader", "x")]));
    assert!(wildcard.every_remote, "a URL that efr cannot read denies every remote");
}

#[test]
fn git_tokens_unscoped_extraheader_denies_github_read() {
    let found = git_tokens(&entries(&[("http.extraheader", "AUTHORIZATION: basic eA==")]));
    assert_eq!(found, GitTokens { every_remote: true, github_read: true, ..GitTokens::default() });
    let reset = git_tokens(&entries(&[("http.extraHeader", "")]));
    assert_eq!(reset, GitTokens::default(), "an empty value resets the list");
}

/// The one helper of a config with the helper `value` in `key`.
fn one_helper(key: &str, value: &str) -> Helper {
    let found = git_tokens(&entries(&[(key, value)]));
    assert!(found.hosts.is_empty() && !found.every_remote && !found.github_read, "{found:?}");
    let [helper] = found.helpers.try_into().unwrap_or_else(|all| panic!("{value}: {all:?}"));
    helper
}

fn every(kind: HelperKind) -> Helper {
    Helper { kind, scope: HelperScope::Every, program: String::new() }
}

fn scoped(kind: HelperKind, host: &str) -> Helper {
    Helper { kind, scope: HelperScope::Host(host.to_owned()), program: String::new() }
}

#[test]
fn helper_kind_gh_forms() {
    for value in [
        "!gh auth git-credential",
        "!/usr/bin/gh auth git-credential",
        "! /usr/bin/gh  auth\tgit-credential ",
        "!'/opt/my tools/gh' auth git-credential",
        "!\"/opt/my tools/gh\" auth git-credential",
    ] {
        assert_eq!(helper_kind(value), HelperKind::Gh, "{value}");
    }
    for value in [
        "!gh auth git-credential --hostname x.test",
        "!gh auth git-credential; cat ~/token",
        "!gh auth git-credential\ncat ~/token",
        "!gh auth",
        "!gh",
        "!./gh auth git-credential",
        "!~/bin/gh auth git-credential",
        "!bin/gh auth git-credential",
        "!/usr/bin/gh-wrapper auth git-credential",
        "!\"/x/$(cat ~/t)/gh\" auth git-credential",
        "!$HOME/bin/gh auth git-credential",
        "!GH_CONFIG_DIR=/x gh auth git-credential",
        "!'/usr/bin/gh auth git-credential'",
        "!'/usr/bin/gh'auth git-credential",
        "!glab auth git-credential",
    ] {
        assert_eq!(helper_kind(value), HelperKind::Shell, "{value}");
    }
    assert_eq!(helper_kind("gh auth git-credential"), HelperKind::Unknown, "no `!`");
}

#[test]
fn helper_kind_known_programs() {
    for (value, kind) in [
        ("libsecret", HelperKind::Libsecret),
        ("/usr/lib/git-core/git-credential-libsecret", HelperKind::Libsecret),
        ("store", HelperKind::StoreDefault),
        ("/usr/lib/git-core/git-credential-store", HelperKind::StoreDefault),
        ("store --file=/home/u/p/app/.creds", HelperKind::StoreFile),
        ("store --file /tmp/c", HelperKind::StoreFile),
        ("/usr/lib/git-core/git-credential-store --file=x", HelperKind::StoreFile),
        ("cache", HelperKind::Cache),
        ("cache --timeout=300", HelperKind::Cache),
        ("manager", HelperKind::Manager),
        ("manager-core", HelperKind::Manager),
        ("/usr/local/bin/git-credential-manager", HelperKind::Manager),
        ("!f() { echo password=x; }; f", HelperKind::Shell),
    ] {
        assert_eq!(helper_kind(value), kind, "{value}");
    }
}

#[test]
fn helper_kind_unknown_absolute_path() {
    for value in [
        "/usr/local/bin/pass-git-helper",
        "/home/u/bin/git-credential-oauth",
        "/home/u/p/app/libsecret",
        "oauth",
        "pass-git-helper",
        "git-credential-store --file=x",
        "bin/libsecret",
    ] {
        assert_eq!(helper_kind(value), HelperKind::Unknown, "{value}");
    }
}

#[test]
fn git_tokens_gh_helper_not_every_remote() {
    let found = git_tokens(&entries(&[
        ("credential.https://github.com.helper", ""),
        ("credential.https://github.com.helper", "!/usr/bin/gh auth git-credential"),
        ("credential.https://gist.github.com.helper", ""),
        ("credential.https://gist.github.com.helper", "!/usr/bin/gh auth git-credential"),
    ]));
    assert!(found.hosts.is_empty() && !found.every_remote, "{found:?}");
    assert_eq!(
        found.helpers,
        [scoped(HelperKind::Gh, "github.com"), scoped(HelperKind::Gh, "gist.github.com")]
    );
    assert!(!HelperKind::Gh.denies(true), "a gh helper whose token the sandbox hides");
    assert!(HelperKind::Gh.denies(false));
}

#[test]
fn git_tokens_shell_helper_denies() {
    let helper = one_helper("credential.helper", "!f() { echo password=x; }; f");
    assert_eq!(helper, every(HelperKind::Shell), "the note never shows the command");
    assert!(helper.kind.denies(true));
    let helper = one_helper("credential.helper", "store --file=/home/u/p/app/.creds");
    assert_eq!(helper, every(HelperKind::StoreFile));
    assert!(helper.kind.denies(true));
}

#[test]
fn git_tokens_unknown_helper_denies() {
    let helper = one_helper("credential.helper", "/usr/local/bin/pass-git-helper -m map.ini");
    assert_eq!(
        helper,
        Helper {
            kind: HelperKind::Unknown,
            scope: HelperScope::Every,
            program: "pass-git-helper".to_owned()
        }
    );
    assert!(helper.kind.denies(true));
    assert_eq!(one_helper("credential.helper", "oauth").program, "oauth");
}

#[test]
fn git_tokens_libsecret_cache_manager_do_not_deny() {
    for helper in ["store", "cache --timeout=300", "libsecret", "manager", "manager-core", ""] {
        assert_eq!(
            git_tokens(&entries(&[("credential.helper", helper)])),
            GitTokens::default(),
            "{helper}"
        );
    }
    for kind in [HelperKind::Libsecret, HelperKind::StoreDefault, HelperKind::Cache] {
        assert!(!kind.denies(false), "{kind:?}");
    }
    assert!(!HelperKind::Manager.denies(false));
}

#[test]
fn git_tokens_scoped_risky_helper_denies_scope_only() {
    let helper = one_helper("credential.https://git.test.helper", "!cat ~/token");
    assert_eq!(helper, scoped(HelperKind::Shell, "git.test"));
    let helper = one_helper("credential.https://Git.Test:8443.helper", "/opt/x/helper");
    assert_eq!(helper.scope, HelperScope::Host("git.test".to_owned()));
}

#[test]
fn git_tokens_wildcard_scope_denies_every_remote() {
    let helper = one_helper("credential.https://*.example.test.helper", "!cat ~/token");
    assert_eq!(helper, every(HelperKind::Shell));
}

#[test]
fn git_tokens_path_scope_denies_its_host() {
    let helper = one_helper("credential.https://github.com/org.helper", "!cat ~/token");
    assert_eq!(helper, scoped(HelperKind::Shell, "github.com"));
}

#[test]
fn git_tokens_scope_without_scheme_denies_every_remote() {
    let helper = one_helper("credential.git.test.helper", "!cat ~/token");
    assert_eq!(helper, every(HelperKind::Shell));
}

#[test]
fn git_tokens_unscoped_risky_helper_denies_every_remote() {
    assert_eq!(one_helper("credential.helper", "!cat ~/token"), every(HelperKind::Shell));
    assert_eq!(one_helper("Credential.Helper", "!cat ~/token"), every(HelperKind::Shell));
    let twice = git_tokens(&entries(&[
        ("credential.helper", "!cat ~/token"),
        ("credential.helper", "!cat ~/token"),
    ]));
    assert_eq!(twice.helpers, [every(HelperKind::Shell)], "each once");
}

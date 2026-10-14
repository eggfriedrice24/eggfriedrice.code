use pretty_assertions::assert_eq;

use super::{GitTokens, TokenFile, git_tokens, token_hosts};

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
    assert_eq!(found, GitTokens { hosts: Vec::new(), every_remote: true, github_read: true });
    let reset = git_tokens(&entries(&[("http.extraHeader", "")]));
    assert_eq!(reset, GitTokens::default(), "an empty value resets the list");
}

#[test]
fn git_tokens_shell_credential_helper() {
    let found = git_tokens(&entries(&[(
        "credential.https://github.com.helper",
        "!/usr/bin/gh auth git-credential",
    )]));
    assert_eq!(found.hosts, ["github.com"]);
    assert!(found.every_remote && !found.github_read);
    assert!(git_tokens(&entries(&[("credential.helper", "!f() { echo x; }; f")])).every_remote);
    for helper in [
        "store --file=/home/u/p/app/.creds",
        "store --file /tmp/c",
        "/usr/lib/git-core/git-credential-store --file=x",
    ] {
        assert!(git_tokens(&entries(&[("credential.helper", helper)])).every_remote, "{helper}");
    }
    for helper in ["store", "cache --timeout=300", "libsecret", "manager", ""] {
        assert_eq!(
            git_tokens(&entries(&[("credential.helper", helper)])),
            GitTokens::default(),
            "{helper}"
        );
    }
}

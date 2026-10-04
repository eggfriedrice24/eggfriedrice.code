# efr-permissions

## Purpose

Pure permission policy. Before any tool call runs, `efr-conversation/src/turn.rs` asks
`Engine::decide(&DecisionInput { requirements, scope, origin, conversation_policy })`
and gets `Allow` (run it), `Ask` (park the turn on an approval) or `Deny` (return an
error to the model), with one reason per requirement.

Modules:

- `path_class`: `PathClass` and `Locations`. Every path a call touches is in one class,
  decided by the target path and never by the shell's working directory:

  | Class | Examples | Read | Write |
  |---|---|---|---|
  | Scratch | the conversation's `$SCRATCH` | free | free |
  | UserConfig | `~/.config`, `~/.zshrc`, other dot entries in `~`, extra config roots | free | approval |
  | UserData | `~/Documents`, `~/p`, `~/.local/share`, `~/.cache` | free | approval, free inside the turn's registered project |
  | System | everything outside `~` | free | approval |
  | Secrets | `~/.ssh`, `~/.gnupg`, `~/.password-store`, `~/.local/share/keyrings`, `~/.netrc`, the credential files of common tools (`~/.aws/credentials`, `~/.codex/auth.json`, `~/.git-credentials`, `~/.config/gh/hosts.yml`, `~/.docker/config.json`, `~/.kube/config`, `~/.npmrc`, `~/.pypirc`, `~/.config/gcloud` and more, listed in `path_class.rs`), `/etc/shadow`, `/etc/gshadow`, the daemon's `secrets/`, and the roots the config adds | denied | denied |

  Classification is lexical: `.` and `..` are resolved by name, nothing is read from
  the disk, and a relative path has no class. Tools resolve symbolic links before they
  declare a path, so when `/home` links to `/var/home` a tool declares
  `/var/home/u/.ssh/id_ed25519`. The daemon therefore builds `Locations` from
  `efr_scope::Home::path()` and adds `Home::canonical()` with `with_home_alias`; a
  path, a scratch directory or a root under any form of `~` is classified as the same
  path under `~`. A root outside `~` whose resolved form differs is added in both
  forms.
- `request`: `DecisionInput`, `Requirements` (paths with `Access`, a command line,
  network, interactive) and `ConversationPolicy` (the conversation's `$SCRATCH` and its
  own rules). `efr-tools` has its own `ToolRequirements`; the forbidden edge keeps the
  crates apart, so `turn.rs` copies one into the other.
- `policy`: `Policy`, an ordered list of `Rule { action, resource, effect }` in which
  the last match wins. `Policy::defaults()` is the table above as eight rules; the
  daemon appends the user's configured rules with `then`. Rules deserialize from TOML
  with unknown keys refused, and every rule is checked when a policy is built.
- `decision`: `Effect` (`Allow < Ask < Deny`), `Decision` and `Reason`, whose `Display`
  is the one line that goes into a log, an approval summary or a denied tool result.
- `engine`: `Engine::decide`.

## Tier

Tier 1.

## Allowed dependencies

`efr-protocol` only, for `Scope`, `Origin` and `ProjectId`. `xtask/src/deps.rs` holds
the allowlist, and forbids `efr-tools -> efr-permissions` so a tool can never grant
itself anything.

Third-party crates: `serde` (rules in the configuration) and `thiserror`.

## Invariant

The engine is a pure function: no file system, no git, no clock, no environment. Its
decisions follow these rules, each covered by a decision table in
`src/engine/tests.rs` and, where marked, a proptest property:

- Secrets are denied by default, for reading and writing. Only the machine policy (the
  user's configuration) can open one, explicitly; a conversation's rules never loosen
  a secret or a system path, they can only tighten it (property).
- A turn from the phone needs approval for everything outside `$SCRATCH`: an `Allow`
  becomes `Ask`, `Deny` stays `Deny`. Any origin other than shell, CLI or proxy,
  including one added to the protocol later, counts as remote (property: the phone is
  never looser than the shell).
- A call that declares no requirement is allowed for the shell, the CLI and the proxy,
  and needs approval for every remote origin, because the engine cannot see what an
  undeclared call does (fail closed).
- Every form of `~` classifies alike: a secret reached through a linked home is
  still a secret (property: both forms of a path get the same class).
- `~`, `/` and the directories above `~`, in any form of `~`, are never treated as a
  project, even when the registry lists one: the project rule then matches nothing. Only the user data of the
  turn's own registered project is free to write.
- A scratch path of `/`, `~` or a directory above `~`, in any form of `~`, makes
  nothing scratch.
- A relative path, or a requirement that no rule matches, is denied (fail closed).
  Adding a requirement never loosens a decision (property).
- A command pattern matches only a simple command line (plain words of
  `[A-Za-z0-9-_./:,+@%=]`), so allowing `git status` never allows `git status; rm -rf ~`.

What the engine cannot see, the caller owns: tools resolve symbolic links and relative
paths before they declare them, and a pattern trusts that a program name means in the
hidden shell what it says.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-permissions
```

Every test feeds literal inputs and asserts decisions; nothing touches the file system,
the network or the clock, and nothing needs Zig.

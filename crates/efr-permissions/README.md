# efr-permissions

## Purpose

Pure permission policy. Before any tool call runs, `efr-conversation/src/turn.rs` asks
`Engine::decide(&DecisionInput { requirements, scope, origin, mode, conversation_policy })`
and gets `Allow` (run it), `Contain` (run it at once in the `auto` sandbox), `Ask`
(park the turn on an approval) or `Deny` (return an error to the model), with one
reason per requirement. `docs/permissions.md` describes
the rules for users: the permission modes, the built-in tables, how a command line is
read, config protection, and the `[[permissions.rules]]` of `config.toml` with
examples.

The permission mode (`efr_protocol::Mode`: `manual`, `cautious`, `auto`) picks the
built-in policy of a turn, `Policy::base(mode)`, and the user's rules follow it.
`Engine::with_rules(locations, rules)` builds the three machine policies once; a turn
from a remote origin runs with at most `cautious` (`effective_mode`).

Modules:

- `path_class`: `PathClass` and `Locations`. Every path a call touches is in one class,
  decided by the target path and never by the shell's working directory:

  | Class | Examples | Read | Write |
  |---|---|---|---|
  | Scratch | the conversation's `$SCRATCH` | free | free |
  | UserConfig | `~/.config`, `~/.zshrc`, other dot entries in `~`, extra config roots, a repository's `.git` in `~` or `$SCRATCH` | free | approval |
  | UserData | `~/Documents`, `~/p`, `~/.local/share`, `~/.cache` | free | approval, free inside the turn's registered project |
  | System | everything outside `~` | free | approval |
  | Secrets | `~/.ssh`, `~/.gnupg`, `~/.password-store`, `~/.local/share/keyrings`, `~/.netrc`, the credential files of common tools (`~/.aws/credentials`, `~/.codex/auth.json`, `~/.git-credentials`, `~/.config/gh/hosts.yml`, `~/.docker/config.json`, `~/.kube/config`, `~/.npmrc`, `~/.pypirc`, `~/.config/gcloud` and more, listed in `path_class.rs`), `/etc/shadow`, `/etc/gshadow`, a process's `environ`, `root`, `cwd`, `fd`, `map_files` and `mem` under `/proc` (its tokens, and back doors to every other path), the daemon's `secrets/`, and the roots the config adds | denied | denied |

  `with_sealed_root` adds a secret root that no rule opens, which the daemon uses for
  its own `secrets/`. `with_write_sealed_root` adds a root that no tool may write and
  no rule opens for writing, while reading follows its class: the daemon passes efr's
  config directory and the real files behind the symbolic links in it. Classification
  is lexical: `.` and `..` are resolved by name,
  nothing is read from the disk, and a relative path has no class. Symbolic links may
  be resolved before a path is declared, so when `/home` links to `/var/home` a call
  may declare `/var/home/u/.ssh/id_ed25519`. The daemon therefore builds `Locations` from
  `efr_scope::Home::path()` and adds `Home::canonical()` with `with_home_alias`; a
  path, a scratch directory or a root under any form of `~` is classified as the same
  path under `~`. A root outside `~` whose resolved form differs is added in both
  forms.
- `request`: `DecisionInput`, `Requirements` (paths with `Access`: `Read`, `ReadTree`
  for a path read with everything below it, or `Write`; a command line and the
  directory it starts in; network; interactive; a `SettingsChange`, which only the
  daemon's settings tool declares; the shell tool's `needs` and `nested_shell`; and
  `CallFacts`, the facts about the line's files and programs that the daemon collects
  for the `auto` mode) and `ConversationPolicy` (the
  conversation's `$SCRATCH` and its own rules). `efr-tools` has its own `ToolRequirements`; the forbidden edge keeps the
  crates apart, so the daemon's toolbox copies one into the other.
- `command`: `analyze` splits a command line into simple commands on `;`, `&&`, `||`,
  `|` and newlines, reading quotes and backslashes as zsh does, and fails closed with a
  `Construct` for anything a command rule cannot judge: a command, process or history
  substitution, a parameter expansion, a group or brace expansion, a here-document, an
  output redirection to anything but `/dev/null` (`2>&1` and `>/dev/null` are fine),
  a background job, an assignment other than `LC_*`, `LANG`, `TZ` and a few more, a
  pattern that could expand to an option (`*.rs`, but not `src/*.rs`), and builtins
  such as `eval`, `exec`, `source`, `.`, `alias` and `export`. The lexer is an
  allowlist: a character it does not know makes the line a construct. A simple command
  with any other pattern outside quotes is marked, and matches only a command pattern
  that counts no operands and forbids no word without a dash, because zsh may turn the
  pattern into several words, none, or a forbidden one.
- `policy`: `Policy`, an ordered list of `Rule { action, resource, effect }` in which
  the last match wins. A `command` resource is a `CommandPattern`: the program, the
  words that must follow it (`args`, with `a|b` alternatives and a trailing `*`), the
  words that must not appear (`forbid`: `--long` with its abbreviations, `-x` inside a
  cluster, `-word` as `find` reads it, or a substring of an operand), `min_operands`
  and `max_operands`, `max_options` (with `max_operands`, 0 allows the `args` alone),
  `under` (`Under`: a directory, `"project"` or `"scratch"`) where the command must
  run, which matches only while the line's directory is known: before any `cd`,
  `pushd` or `popd` in it, and `check` (`Check`, in `policy/check.rs`): a check of the
  words that a list of words cannot express, `sed_print_only` for a `sed` that only
  prints and `ref_names` for git operands that can only be refs. A command rule with
  the action `network` matches the network access of that command.
  `Policy::base(mode)` is the built-in policy of a mode: `manual` is `any any ask`
  and the secrets rule; `cautious` is `Policy::defaults()`, the table above as eight
  rules followed by the read-only commands of `policy/defaults.rs` (`ls`, `cat`, `rg`,
  `git status`, `systemctl status`, `journalctl` without `--vacuum*`, `pacman -Q*`,
  `find` without `-exec` or `-delete`, `cd`, `pushd`, `popd`, `sed` that only prints
  and more; `env` and `printenv` are left out because they print tokens, and
  `systemctl show` must name a unit, `ps` may hold no `e` outside a long option and no
  `--format`, and `jq` may use no `env`, `$ENV`, program file or module, for the same
  reason); `auto` is a table of its own: reading allowed, writing allowed
  in the project, `$SCRATCH` and, for a shell call, the envelope roots
  (`Locations::with_envelope_root`), secrets denied, and every command line
  `contain`. `policy/defaults.md` is the read-only table for the docs, and tests keep
  it, the table in `docs/permissions.md` and the data equal. `Policy::new` and
  `Policy::push` refuse a rule with the effect `contain` and the `envelope`
  resource, which only the `auto` policy uses. Rules deserialize from TOML with
  unknown keys refused, and every rule is checked when a policy is built.
- `decision`: `Effect` (`Allow < Contain < Ask < Deny`), `Decision` and `Reason`, whose
  `Display` is the one line that goes into a log, an approval summary or a denied tool
  result. `Decision::exits` lists the exits of an `auto` call.
  The `Subject::Command` of a line of several simple commands that asks or is denied
  lists in `deciding` the words of each simple command that got the line's effect, in
  the order of the line, so an approval can name them.
- `engine`: `Engine::decide` and `effective_mode`; `Engine::with_support` sets what the
  machine's sandbox supports (`AutoSupport`: the egress, the bus proxy, undo).
  `Engine::path_facts` tells where a path stands for a call in `auto` (`PathFacts`: its
  class, and whether it lies in a write root, on a floor or in a synced folder), so the
  conversation builds the facts of an exit record from the roots that the engine uses.
- `exits`: `predict`, which finds the exits of a shell call in `auto` from its line
  (read through a lenient split that never fails), its declared paths, its network
  need, the model's `needs` and the daemon's `CallFacts`; each `ExitNeed` has a kind,
  the grants a "yes" opens, the bind of a write (`WriteBind`) and whether only the
  user may approve it. `PRIVILEGE_EXITS` holds the programs that only `auto` treats as
  a privilege exit. `unsandboxed_line_problem` is the one-command rule of an exit that
  runs outside the sandbox. `fact_requests` tells the daemon which paths, `rm -r`
  directories and program words of a line its `CallFacts` must describe, from the same
  lenient split. Its `builtins` are the program words that the shell runs itself in
  every place of the line, a zsh builtin or reserved word such as `:`, `cd` or
  `printf` (a fixed list, `exits/programs.rs`); behind `env`, `sudo`, `command`,
  `exec` or `find -exec` only a file runs.
- `tables`: `SANDBOX_MASKS`, `PROTECTED_NAMES` and `PERSISTENCE_FLOORS`, which the
  engine and the daemon's sandbox planner share, as they share the secrets
  (`secret_paths`, `Locations::secret_paths`).

## Tier

Tier 1.

## Allowed dependencies

`efr-protocol` only, for `Scope`, `Origin`, `Mode` and `ProjectId`. `xtask/src/deps.rs`
holds the allowlist, and forbids `efr-tools -> efr-permissions` so a tool can never
grant itself anything.

Third-party crates: `serde` (rules in the configuration), `schemars` (the JSON schema of
those rules, which `efr-config` publishes) and `thiserror`.

## Invariant

The engine is a pure function: no file system, no git, no clock, no environment. Its
decisions follow these rules, each covered by a decision table in
`src/engine/tests.rs`, `src/engine/tests/commands.rs`, `src/engine/tests/modes.rs` or
`src/engine/tests/protection.rs`, `src/engine/tests/settings.rs` and, where marked, a
proptest property:

- The floors hold in every mode: an interactive call and a privileged program ask, a
  remote origin runs with at most `cautious` and asks outside `$SCRATCH`, secrets stay
  denied unless a user rule names them, a conversation's rules only tighten, and no
  tool writes a write-sealed root.
- A change of efr's settings (the settings tool) asks in every mode, also in `auto`
  and also when every rule allows everything; no rule is read for it. A turn from a
  remote origin is denied it. `Engine::names_secrets` says whether a rule's resource
  names secrets, the rules the settings tool refuses to write.
- A write-sealed root (efr's config) is denied for writing whatever any rule says,
  the user's and the conversation's included; a write of a directory above it, or
  above a secret no rule opens, asks even when a rule allows it.
- In the `auto` mode a shell call is at least `Contain`: no rule lifts the sandbox,
  and a path or network requirement that a built-in rule asks about is contained too.
  Each exit asks on a reason of its own (`Cause::Exit`), a secret or efr's config is
  denied, a user's `ask` or `deny` rule keeps `Cause::Rule`, and `nested_shell` is
  denied. A remote turn runs as `cautious`, so it is never contained. `read_file` and
  `write_file` keep the `cautious` path rules, plus an exit for a sandbox mask and for
  a floor inside a write root. `tests/it/corpus.rs` runs the 110 lines of the command
  corpus (`tests/fixtures/auto-corpus.toml`) and checks the phase 1 result of each.
- `PRIVILEGED` holds the same six programs in every mode, so a user's `docker` rule
  keeps working in `cautious`; `auto` adds `exits::PRIVILEGE_EXITS`.
- A write of the project's root itself is not a project write, so it asks.

- Secrets are denied by default, for reading and writing. Only the machine policy (the
  user's configuration) can open one, explicitly: with a rule that names secrets, the
  class or an `under` path at or below a secret location. A rule for a wider resource
  (`read any allow`, `under = "~"`, the project) never opens a secret and can only
  make a decision about one stricter; a conversation's rules never loosen a secret or
  a system path, they can only tighten it (property). A sealed root, the daemon's own
  credentials, is denied whatever any rule says.
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
- A command line is allowed only when every simple command in it is (property: no
  operator joins an asked command to an allowed one), so allowing `git status` never
  allows `git status; rm -rf ~`. A line that `command::analyze` cannot split matches
  only the rules whose resource is `any`, which ask by default (property: no line with
  a `$(` or backquote outside single quotes is allowed by the defaults).
- `sudo`, `sudoedit`, `doas`, `su`, `pkexec` and `run0`, also behind a wrapper such as
  `env` or inside a substitution, always need approval: no command pattern matches
  them and an `allow` from a rule for every command line becomes `ask`.
- A path read with everything below it (`ReadTree`, which the shell tool declares for
  recursive searches, recursive listings and globs) needs approval when a secret that
  no rule allows lies below it, so `rg TOKEN ~/.aws` asks although `rg` is allowed and
  `~/.aws` is user config; naming a secret itself, as `cat ~/.ssh/id_ed25519` does, is
  denied.

What the engine cannot see, the caller owns: tools resolve relative paths before they
declare them, and the daemon adds what each declared path reaches through a symbolic
link (`ToolRequirements::with_real_paths`), so the shell tool's `cat notes`, where
`notes` links into `~/.ssh`, also declares the key. A link that a glob expands to, or
one below the root of a recursive search that the program follows, is not seen. A
pattern trusts that a program name means in the hidden shell what it says. The
hidden zsh removes global and suffix aliases, and every alias or function named like
a program of a command rule (the daemon passes them as
`efr_shell::ShellConfig::trusted_programs`); a program earlier on the `PATH` under
that name still runs instead. No repository configuration may run a program for
`git status` either: a `.git` is user config, so the model cannot write one without
approval, but configuration that a repository already has, or includes from its work
tree, still runs.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-permissions
```

Every test feeds literal inputs and asserts decisions; nothing touches the file system,
the network or the clock, and nothing needs Zig.

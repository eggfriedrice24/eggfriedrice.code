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
  config directory and the real files behind the symbolic links in it.
  `with_efr_root` and `with_efr_binary` add efr's own runtime, data and state roots
  (each with the places below it that stay open: `$SCRATCH` and efr's trash) and efr's
  programs, which no tool writes in any mode. The `auto` sandbox adds more:
  `with_envelope_root` (a write root of every contained call), `with_private_root` and
  `with_cache_root` (write roots whose writes stay private: the private tmp and the
  cache overlays), `with_rebuildable_names` and `with_trash_root` (what a build or efr
  can make again), `with_floor_root`, `with_floor_group_root` and
  `with_link_floor_root` (floors with no group, with a `FloorGroup`, or with the group
  and the names of a dotfile link), `with_synced_root`, `with_sandbox_mask` and
  `with_credential_socket` (the socket of a credential agent, such as
  `$SSH_AUTH_SOCK`).
  Classification
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
  daemon's settings tool declares; `destructive`, for a call that deletes or moves a
  file (a delete or a move of `apply_patch`); the shell tool's `needs` and
  `nested_shell`; and
  `CallFacts`, the facts about the line's files and programs that the daemon collects
  for the `auto` mode, with each declared link and the real path it reaches, whose
  write takes the names of the link, and from phase B the answer to each `CoverAsk`
  of `FactRequest::covers` in `covered`: a `Coverage`, which `CallFacts::coverage`
  reads as `Unknown`, never covered, when no fact says) and `ConversationPolicy` (the
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
  result. `Decision::exits` lists the exits of an `auto` call, and
  `Decision::view_grants` the grants of its views (`Subject::View`), which need no
  exit.
  The `Subject::Command` of a line of several simple commands that asks or is denied
  lists in `deciding` the words of each simple command that got the line's effect, in
  the order of the line, so an approval can name them.
- `engine`: `Engine::decide` and `effective_mode`; `Engine::with_support` sets what the
  machine's sandbox supports (`AutoSupport`: the egress, the bus proxy, undo with
  `sandbox.ask_destructive = "unless_undoable"`, the home store that copies each home
  write, the mount view, the network view). With `undo`, a delete or a move of a file tool in `auto`
  asks no question when each path that it writes is covered: by the turn's snapshot
  in the turn's project and `$SCRATCH`, by the home store elsewhere.
  `Engine::path_facts` tells where a path stands for a call in `auto` (`PathFacts`: its
  class, whether it lies in a write root, on a floor or in a synced folder, and what
  can make it again, `Rebuild`: a cache, the build output of a registered project or
  efr's trash), so the conversation builds the facts of an exit record from the roots
  that the engine uses. `Engine::decide_cache_delete` decides the deletes that a
  contained call made in a cache overlay: one `write` exit per real cache file, with
  the source `cache_delete`, for the classifier.
- `exits`: `predict`, which finds the exits of a shell call in `auto` from its line
  (read through a lenient split that never fails), its declared paths, its network
  need, the model's `needs` and the daemon's `CallFacts`; each `ExitNeed` has a kind,
  the grants a "yes" opens, the bind of a write (`WriteBind`), its `Approver` (the
  classifier, the classifier when a user message names the exit, or only the user),
  the `names` that name it, what efrd copies into its home store first (`copies`: a
  `Home` file, or a `HomeDir` with what it holds for a dir target, an `above_root`
  write and the parent that a `Parent` bind opens; `backup_before_write` when it
  copies anything, and `covered` when the facts say that every copy is covered), the
  same file below a dotfile link for a floor edit of the real file behind it (`link`,
  which names the copy) and the `Ecosystem` of a package manager's fetch
  (`REGISTRY_HOSTS`). A destructive part of a line has no exit when the machine has
  undo and every `CoverAsk` of the part is covered (the module `exits/cover.rs`): each
  path at or below a target for `rm -r`, `dd of=`, `truncate`, `: >` and
  `find -delete`, strictly below the turn's project, `$SCRATCH` or a registered
  project and never in a git dir or on a floor; the tracked files and the refs of the
  repository for `git reset --hard`, `checkout`, `switch`; the tracked files for
  `restore`; the untracked files for `clean -f`; the refs for `branch -D`; the one
  stash entry for `stash drop` and `clear`. `shred`, `git clean -x` and `-X`,
  `git reflog` and `git gc --prune` ask nothing, so they are never covered. Every other
  exit of the line keeps its route. The engine never reads user text: `names_match` is the one
  check of the user's messages, for the turn's route and the exit record alike. Path
  forms match as text, only as whole path components; word forms and the phrases of
  `NAME_PHRASES` match whole words, a stem of `GENERIC_STEMS` names nothing alone, and
  the bare stem of a program, a unit, a desktop entry or an agent config names nothing
  (only its name with the suffix). `DESKTOP_IPC_READ` holds the read
  and reload forms of the desktop programs that the classifier may judge. A
  `desktop_ipc` exit grants the one desktop that its program talks to
  (`Grant::Desktop`: `hyprctl` Hyprland, `swaymsg` sway, `i3-msg` i3, `niri msg` niri,
  `wlr-randr`, `wtype` and `wl-copy` Wayland, `xrandr`, `xdotool` and the other X
  tools X11); efrd finds the socket. `ydotool` talks to its own daemon and gets no
  grant. A desktop socket that the model names in `needs.sockets` gets the grant of
  its desktop, never its path, and only the user approves it. `nmcli` and `iwctl` ask
  NetworkManager and iwd on the system bus, so they are `bus` exits with
  `Grant::Bus { bus: System }`: the classifier judges their read forms, such as
  `nmcli device status` and `iwctl station wlan0 show`, and only the user approves
  every other form, such as `nmcli radio wifi off`.
  `PRIVILEGE_EXITS` holds the programs that only `auto` treats as
  a privilege exit. `unsandboxed_line_problem` is the one-command rule of an exit that
  runs outside the sandbox. `fact_requests` tells the daemon which paths, `rm -r`
  directories and program words of a line its `CallFacts` must describe, from the same
  lenient split, and which coverage asks it must answer (`covers`): the asks of each
  destructive part and the home store asks of each path that the line writes
  (`FactRequest::with_writes` adds the declared write paths). `tool_fact_requests`
  does the same for a file tool, from its declared paths. Its `builtins` are the program words that the shell runs itself in
  every place of the line, a zsh builtin or reserved word such as `:`, `cd` or
  `printf` (a fixed list, `exits/programs.rs`); behind `env`, `sudo`, `command`,
  `exec` or `find -exec` only a file runs.
- `exits/view.rs` (phase D1): the network view rule. `exits::views` gives
  `Grant::NetView` to a shell call whose line only reads the host's network, on a
  machine that runs NetView calls (`AutoSupport::netview`): at least one view command
  and nothing but view commands and text helpers. View commands are `ss` without `-K`,
  `-D` or `-N` (and their long forms and abbreviations), the read forms of `ip` (an
  object such as `addr`, `link`, `route` or `neigh` with no verb or `show`, `list`,
  `ls`, `lst`, `get`, and no option that is a prefix of `-batch`, `-netns` or `-all`), `nstat`
  without `-d`, and `cat`, `head`, `tail`, `grep` or `wc` of a file below `/proc/net`,
  `/proc/self/net` or `/proc/<pid>/net`. The helpers are `grep`, `head`, `tail`, `wc`,
  `sort`, `uniq`, `cut`, `tr`, `column`, `cat`, `echo`, `printf`, `date`, `sleep`, `:`,
  `true` and `tee`, each matched by its row of the read-only table where one exists;
  a view command may run behind `watch` or `timeout`, and in a `while`, `if` or `{ }`.
  No interpreter, `find`, `git`, `xargs`, assignment or other wrapper. `predict` then
  finds no `host_view` exit for the line; the engine adds one `Subject::View` reason
  per grant, which is `Contain` and never an exit. The rule is a fit, not a security
  layer: a NetView call can run any code, and its seccomp profile, Landlock rights and
  the refusal of `listen` carry the whole load. `iw`, `ifconfig`, `netstat`, the
  write forms of `ip` and any line that does more stay `host_view` exits.
- `jobs` (phase C): `job_refusal` names the exits that no background job may hold, in
  every mode and whatever the user answers: `privilege`, `masked_read`, `persistence`,
  a `desktop_ipc` form that types or starts programs, the whole message bus, and the
  socket of a service manager or a container engine (`SERVICE_SOCKETS`). The engine
  denies such a part of a job's line with `Cause::NotAsJob` (`JOB_REFUSAL`: run that
  part in the foreground, and the rest as a job).
- `network` (phase D2): the host list of efr's network proxy, which the config check,
  the engine and the proxy share. `Preset` holds the six presets of spec 8.3 and their
  hosts (`@github-read` also has `release-assets.githubusercontent.com`; no preset has
  `api.github.com`, which takes writes with a token); `REGISTRY_HOSTS` is a view of
  them. `HostList::compile` reads `network.allow`, `network.deny`, the
  `[[network.hosts]]` tables (`HostTable`) and the pacman mirror hosts, and refuses an
  entry with a `HostError`: `*` alone, a `*.suffix` with fewer than two labels, a name
  outside ASCII (write the `xn--` form), a bad label, an unknown `@preset`, an IP
  address outside `[[network.hosts]]`, port 0. `HostList::decide` gives `Allowed` (with
  `http` and `private`), `Denied` or `NotListed` for a host and a port: deny wins over
  every allow; a plain entry allows 443, an `http` entry also 80, an entry with a port
  only that port; `*.example.com` matches the names below `example.com`, not
  `example.com` itself. `host_and_port` reads a host word of the model or of a URL
  (port 80 for `http://`; `None` outside ASCII), and `is_ip_literal` finds every form
  of an IP address that getaddrinfo reads. `parse_pacman` reads the `Server =` and
  `Include =` lines of the pacman files that efrd reads. `network/tokens.rs` holds the
  line scans of DP25: `token_hosts` finds a token key or a URL with a password in a
  project's `.npmrc`, `.yarnrc.yml`, `.yarnrc`, `bunfig.toml` or cargo config and
  gives every host that the file names plus the ecosystem's registry hosts;
  `git_tokens` finds the remotes, `insteadOf` bases, `extraHeader`s and credential
  helpers of the git config that carry a credential. `Engine::with_hosts` gives the
  engine the list (`Engine::hosts`; empty by default). `FactRequest::remotes` names
  the directories of git commands that reach a remote, and `CallFacts` gets their
  remotes (`RemoteFact`, each URL a `RemoteUrl`) and `refused_hosts`, the hosts that
  the proxy refused. Nothing reads the list, the scans or these facts yet; the
  engine, the proxy and efrd use them later in phase D2.
- `tables`: `SANDBOX_MASKS`, `PROTECTED_NAMES` and `PERSISTENCE_FLOORS`, which the
  engine and the daemon's sandbox planner share, as they share the secrets
  (`secret_paths`, `Locations::secret_paths`). `PERSISTENCE_FLOOR_GROUPS` gives each
  floor its `FloorGroup`, which decides the phrases that name its files.

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
- A delete or a move of a file (`Requirements::destructive`) asks in every mode, from
  every origin and also when every rule allows everything, on its own reason
  (`Subject::Destructive`, `Cause::Destructive`). The one exception is a turn in `auto`
  on a machine with undo, when the facts say that `efr undo` can bring back every path
  that the call writes. Its paths are judged as writes on their own reasons, so a
  denied path stays denied (`src/engine/tests/destructive.rs`).
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
  `write_file` keep the `cautious` path rules, plus an exit for a sandbox mask, for a
  floor inside a write root, and for a write that a built-in rule asks about outside
  the turn's project and `$SCRATCH` (its path reason is then `Contain` with
  `Cause::ExitDecides`, so a lost exit still asks). `tests/it/corpus.rs` runs the 110
  lines of the command corpus (`tests/fixtures/auto-corpus.toml`) and checks the phase
  A result of each (`pa`) and, on a machine with undo and the daemon's coverage
  answers of the row, the phase B result (`pb`), and on a machine with the network
  view and the mount view too, the phase D1 result (`pd`, the `pb` result where a row
  has none); the `p1` column keeps the phase 1 result. A phase B result differs only
  where the snapshot covers a destructive line, which becomes routine, and a phase D1
  result only for `df`, which reads the mount view in the sandbox.
- Every exit starts with the approver of its kind (`ExitKind::user_only`), and only
  the places of spec 9.1 relax it: every `write` goes to the classifier, also one that
  runs in the exit child; an `upload`, an `above_root` path that holds no second
  project, floor, efr path or secret, and a floor file edit go to it only when a user
  message names them; a floor file edit only while `AutoSupport::floor_backup` holds, a
  fact says that the target is a file or a new file in an existing dir, and the line
  makes no link and touches no `*.wants` dir, unless it installs a file under the rule
  of named persistence below. On a machine with undo, a `destructive`
  exit goes away when the snapshot covers every target of its part (spec 10.7,
  `src/engine/tests/coverage.rs`), never for `shred`, a target at or above a root, in a
  git dir, on a floor or in a `sandbox.write_roots` entry. An `outside` exit asks the user when a
  fact about a program word is missing, a program word changed in the turn or lies
  in a real write root (not the private tmp or a cache), or the line runs an
  interpreter on code that efr cannot read (`python3 -c`, `sh -c`, a script). A `destructive` exit goes to
  the classifier only when efr knows every target and each lies in a real write root.
  Named persistence that is no file edit goes to the classifier only when a user
  message names it, efrd says that it can copy what it changes, and the classifier can
  read what it installs (plan PB19, PB19a, PB20, `src/exits/persist.rs`):
  `systemctl --user enable`, `reenable`, `preset`, `mask`, `add-wants` and
  `add-requires` of units by name (`CoverAsk::UnitLinks`), when each unit file existed
  in a unit dir before the turn, outside every write root, and fits in 8 KiB; `ln -s`
  into a `*.wants` or `*.requires` dir and `cp` or `ln` into a floor, when the source
  holds the same rule (`FactRequest::sources`, `CallFacts::sources`); `crontab <file>`
  under that rule, `crontab -` fed a literal by `echo` or `printf`, and `crontab -r`
  (`CoverAsk::CronTable`); `loginctl enable-linger` and `disable-linger` for the
  user's own account (`CoverAsk::Linger`). The text of each installed file goes into
  `ExitNeed::installs`, and the turn puts it in the record. `systemctl --user link`,
  `edit`, `revert`, `enable <path>`, `crontab -e`, `at`, `batch`,
  `systemd-run --user`, a link into a floor file, the git surface, `sandbox.protect`,
  a privilege, a masked read, a synced write and a socket of a credential agent or
  under a mask stay with the user; a socket inside a secret or efr's own roots is a
  floor (`src/engine/tests/auto.rs`).
- A write of efr's own programs, or of its runtime, data or state roots outside
  `$SCRATCH` and efr's trash, is denied in every mode (`Cause::EfrOwn`), and in `auto`
  a shell write there is a `config` exit.
- A shell call that starts a background job (`Requirements::background`) has the exits
  of the same line in the foreground, except that no delete counts as covered by undo:
  the job's call answers at once, and undo of its turn needs `--force` while it may
  still write. A part that no job may hold is denied in every mode
  (`Cause::NotAsJob`). Outside `auto` each job asks the user (`Subject::Job`,
  `Cause::JobAsks`), whatever the rules say, and a remote turn runs as `cautious`, so it
  asks too (`src/engine/tests/jobs.rs`, and the corpus table
  `background_flag_does_not_change_prediction`).
- On a machine that runs NetView calls, a shell line in `auto` that only reads the
  host's network (`exits/view.rs`) runs contained with `Grant::NetView` and no exit, so
  no classifier and no question. A line that does anything else gets no view and keeps
  its exits; `nmcli` and `iwctl` are `bus` exits in every phase
  (`src/engine/tests/auto.rs`, `netview_rule_cases`).
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

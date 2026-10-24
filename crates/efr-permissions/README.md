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

The permission mode (`efr_protocol::Mode`: `manual`, `cautious`, `auto`, `full`) picks
the built-in policy of a turn, `Policy::base(mode)`, and the user's rules follow it.
`Engine::with_rules(locations, rules)` builds the three machine policies once (full
mode uses the one of `auto`); a turn from a remote origin runs with at most `cautious`
(`effective_mode`). A mode newer than this crate gets the `manual` policy and fails
closed.

Full mode (`Mode::Full`) runs the model's commands as the user, with no sandbox and no
question. `Engine::decide` decides a full call as an `auto` call on a machine with no
sandbox (`AutoSupport::for_full`), so it finds the same exits, home copies and undo
cover, and then settles the reasons (`full.rs`, `settle`): what the sandbox would hold
(`Contain`) and what a built-in row asks about run with `Cause::Full`; each exit runs
and keeps its reason, so `Decision::exits` lists it for the record; a floor stays
denied; a privileged program, a `privilege`, `persistence` or `destructive` exit, a
change of efr's settings, a file tool delete that undo does not cover and a user's or a
conversation's `ask` or `deny` rule still decide; a view of the host is dropped. For a
full shell call the engine adds one reason per delete that `exits::critical_deletes`
finds (`exits/critical.rs`, below): `Subject::CriticalDelete` with `Cause::CriticalDelete`
asks, whatever the rules say, and in a background job it is denied with
`Cause::NotAsJob`, as a job's `destructive` exit is. `Decision::critical` lists them,
and `CriticalDelete::describe` gives the line for the user. Full mode guards against
mistakes, not against a hostile model: it reads only the line, so a script or a program
that the line starts can do what the user can.

Modules:

- `path_class`: `PathClass` and `Locations`. Every path a call touches is in one class,
  decided by the target path and never by the shell's working directory:

  | Class | Examples | Read | Write |
  |---|---|---|---|
  | Scratch | the conversation's `$SCRATCH` | free | free |
  | UserConfig | `~/.config`, `~/.zshrc`, other dot entries in `~`, extra config roots, a repository's `.git` in `~` or `$SCRATCH` | free | approval |
  | UserData | `~/Documents`, `~/p`, `~/.local/share`, `~/.cache` | free | approval, free inside the turn's registered project |
  | System | everything outside `~` | free | approval |
  | Secrets | `~/.ssh`, `~/.gnupg`, `~/.password-store`, `~/.local/share/keyrings`, `~/.netrc`, the credential files of common tools (`~/.aws/credentials`, `~/.codex/auth.json`, `~/.git-credentials`, `~/.config/git/credentials`, `~/.config/gh/hosts.yml`, `~/.config/hub`, `~/.config/github-copilot`, `~/.gcm`, `~/.docker/config.json`, `~/.kube/config`, `~/.npmrc`, `~/.pypirc`, `~/.config/gcloud` and more, listed in `path_class.rs`), `/etc/shadow`, `/etc/gshadow`, a process's `environ`, `root`, `cwd`, `fd`, `map_files` and `mem` under `/proc` (its tokens, and back doors to every other path), the daemon's `secrets/`, and the roots the config adds | denied | denied |

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
  `$SSH_AUTH_SOCK`; the engine also knows such sockets by a word of their path, also
  `credential` for git's credential cache from v0.0.4), and `with_ssh_control_path`
  (a `ControlPath` of the user's ssh config, v0.0.5: a socket that it matches counts
  as a credential agent's socket).
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
  directory it starts in; network; interactive; `may_prompt`, the soft flag of a line
  that may ask for a password through ssh, which gives no reason and which the check
  point reads for the call's time limit and the question's text; a
  `SettingsChange`, which only the
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
- `command/git.rs` (v0.0.4, DL21 to DL26): the one table of git's global options
  (`OPTIONS`, as of git 2.56.0), each with its form (a value in the next word, a value
  after `=`, or none) and its class (`plain`, `place`, `steer`, `print`). `exact` finds
  the subcommand by the table and reads an option that it does not know as a flag.
  `views` gives the readings of a git command for a rule that asks or denies: every
  global option and its value removed; an unknown option read both as a flag and with
  a value in the next word, and after four of them every later word that does not
  start with `-` as a possible subcommand; a `-c alias.<name>=<value>` of the line read
  as its command, or `unreadable` when efr cannot read it (`!`, a quote,
  `--config-env`); and the dirs of `-C` (each relative to the one before) and
  `--work-tree`. `plain_view` removes only the `plain` options (`--no-pager`, `-P`,
  `--no-optional-locks`, `--no-advice`, `--no-lazy-fetch` and the pathspec options),
  for a rule that allows. The exits take the positions of git's options from `exact`
  too (DL28): the upload and destructive forms of `exits/programs.rs`, the remote
  facts of `exits/remotes.rs` (the dir of the `-C` options, each relative to the one
  before), the delete guard of `exits/critical.rs`, the cover asks of
  `exits/cover.rs` and the git code check of `exits/git_code.rs`. Each keeps its own
  idea of which options it accepts, so `cover.rs` still accepts only `--no-pager`,
  `-P`, `--no-optional-locks` and one `-C`.
- `command/globals.rs` (v0.0.5, OQL4): a table of the global options of `systemctl`,
  `cargo`, `docker` and `kubectl` (`TABLES`), each option a flag, an option with a
  value (in the same word or the next) or an option that can set an alias of a
  subcommand (`cargo --config` with a file or an `alias` key, `kubectl --kuberc`).
  `readings` gives the places where the subcommand may stand for a rule that asks or
  denies: each option of the table skipped with its value, `+toolchain` skipped for
  `cargo`, an unknown option read both as a flag and with a value in the next word,
  after four of them every later word that does not start with `-`, and `unreadable`
  for an alias or past 64 places, so the rule matches. The tables hold only the
  options whose form is sure; a missing option is read both ways. `cargo -C` gives a
  dir for `under`. A rule that allows reads no reading.
- `command::directory_change` (v0.0.5, OQL9): where a `cd`, `chdir`, `pushd` or
  `popd` goes: home, one directory that the word names (absolute, `~` or below `~`),
  or unknown (a relative target, `cd -`, a stack entry, `popd`, `pushd` alone, two
  operands, a pattern, or a `cd` behind a wrapper such as `time`).
  `policy/place.rs` walks the line: before the first change each command runs in the
  start dir (`Place::Start`), after one in the start dir or any target so far
  (`Place::Among`, at most 32 dirs), after an unknown one anywhere (`Place::Unknown`).
- `policy`: `Policy`, an ordered list of `Rule { action, resource, effect }` in which
  the last match wins. A `command` resource is a `CommandPattern`: the program, the
  words that must follow it (`args`, with `a|b` alternatives and a trailing `*`), the
  words that must not appear (`forbid`: `--long` with its abbreviations, `-x` inside a
  cluster, `-word` as `find` reads it, or a substring of an operand), `min_operands`
  and `max_operands`, `max_options` (with `max_operands`, 0 allows the `args` alone),
  `under` (`Under`: a directory, `"project"` or `"scratch"`) where the command must
  run, and `check` (`Check`, in `policy/check.rs`): a check of the
  words that a list of words cannot express, `sed_print_only` for a `sed` that only
  prints and `ref_names` for git operands that can only be refs. A command rule with
  the action `network` matches the network access of that command. For the program
  `git`, `Rule::runs` reads the rule's effect (DL20): a rule that asks or denies
  matches the raw words or any view of `command/git.rs`'s `views`, so `ask git push`
  asks for `git -C ~/p/app push`, and its `under` also counts the dirs of `-C` and
  `--work-tree`; a rule that allows, the built-in read-only rows included, matches the
  raw words or the plain view only, so `allow git log` matches `git --no-pager log`
  but not `git -C ~/other log`. Every check of the pattern reads the view's words, so a
  global option never counts as an operand or a forbidden word. The last match still
  wins, and a conversation's rules read the same views. A rule that asks or denies, for
  any program, also reads the forms of `command::run_forms` (OQL3): the program by its
  base name (`/usr/bin/git push`) and, behind a wrapper of `WRAPPERS` (`env`, `nice`,
  `timeout`, `xargs` and the rest), each later word that does not start with `-` as the
  program, so `env git -C ~/p/app push` meets `deny git push`. efr does not know which
  words a wrapper takes as values, so the rule may match more than runs, which only
  asks or denies more. A rule that allows reads neither form. A rule reads at most
  `MAX_FORMS` (16) forms of its program in one command, because each form reads the
  rest of the command and a git form reads it again for each view; past the cap the
  rule matches in any place, so `env git git git ...` asks or is denied. For
  `systemctl`, `cargo`, `docker` and `kubectl`, a rule that asks or denies also reads
  each place of `command/globals.rs`'s `readings` (OQL4), as a read form. `under` of a
  rule that allows matches only in `Place::Start`, before any change of directory; of
  a rule that asks or denies, in any dir of the place and of git's or cargo's `-C`, and
  in every dir for `Place::Unknown` (OQL9), as a read form, so such a match only
  makes the effect stricter. For a line that `analyze` cannot split, the engine also
  reads each command that the lenient split of `exits/scan.rs` finds in it
  (`exits::lenient_commands`) against the command rules that ask or deny, with an
  unknown place (`Engine` `tightened_opaque`): the strictest one that matches decides
  when it is at least as strict as the rules for every command line, so
  `cd "$DIR" && git push` meets `deny git push`, also in full mode.
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
  `contain`; `full` has the rows of `auto`. `policy/defaults.md` is the read-only table for the docs, and tests keep
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
  write, the mount view, the network view). A full call decides with
  `AutoSupport::for_full`: no egress (each network use is a `host` exit), no bus proxy
  (each bus use is a `bus` exit), the host's mounts and network, and `undo` and the
  home store as efrd set them; it needs no probe. With `undo`, a delete or a move of a file tool in `auto`
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
  (read through a lenient split that never fails; it reads substitutions 64 levels
  deep, and one `sh -c` segment of unread code stands for a deeper one, so only the
  user approves such a line outside the sandbox), its declared paths, its network
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
  `Grant::Bus { bus: System }`, also with the filtered bus proxy of phase E: the
  classifier judges their read forms, such as `nmcli device status` and
  `iwctl station wlan0 show`, and only the user approves every other form, such as
  `nmcli radio wifi off`.
  The `rustup` forms that install (`toolchain install`, `install`, `update`,
  `target add`, `component add`, `self update`) are `outside` exits, because in the
  sandbox they write only the private layer of `~/.rustup`. The install puts programs
  on the user's `PATH`, so the classifier judges only a release channel (`stable`,
  `beta`, `nightly`, `1.N` or `1.N.M`, with an optional date and target triple) with
  plain target and component names, and only when a user message names `rustup`,
  `toolchain` or the channel. A line that sets `RUSTUP_*` or `CARGO_HOME`, a path, a
  custom toolchain, an unknown option and `rustup toolchain link` are for the user.
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
- `full`: `settle`, the settle pass of a full call (DF3), a pure function of the
  reasons, the job flag and which causes are built-in rows of the deciding machine.
- `exits/critical.rs` (full mode, DF4): `exits::critical_deletes` finds the deletes of
  a line that full mode guards, each a `CriticalDelete` with its simple command and
  its target. It is pure and reads only the line, the locations and the turn's roots.
  It reads the line through the same lenient split as `predict`, behind the same
  wrappers (`sudo`, `env`, `nice`, `timeout`, `setsid`, `nohup`, `xargs`), in each
  substitution and in each branch that may not run (`false && rm -rf ~`; a
  substitution below the 64 levels that the split reads is an unreadable target), and one
  level into the literal string of `sh -c`, `bash -c`, `zsh -c` and `eval`. The
  delete forms are: `rm -r`, a pattern operand of a plain `rm`, `find` with `-delete`
  or an `-exec` of `rm`, `shred`, `srm` or `wipe` (its roots), the sources of `mv`,
  the destination of `rsync --del*`, `chmod -R`, `chown -R`, `chgrp -R`, `setfacl -R`,
  `shred -u`, `srm`, `wipe`, and `git clean -fd` (the work tree of its dir, `-C` and
  `--work-tree`, or its paths). A target is critical when it is `/`, `~` or a dir above
  `~`; a system dir outside `~` (a dir directly below `/`, a dir of `SYSTEM_DIRS` or
  one directly in it, and the disks below `/run/media` and `/media`, `tables.rs`); the
  turn's project root or a registered project root; or a dir that holds what
  `Envelope::holds` counts (efr's config or own paths, a secret, a floor, two or more
  projects or real write roots). `Envelope::above_root_approver` reads the same
  `holds`, so the two cannot drift. A target that efr cannot read is critical too: a
  word with an expansion (`$HOME` and `${HOME}` at its start read as `~`), a pattern
  of a tree form whose literal dir is empty, critical or unreadable (`*`, `~/*`,
  `"$X"/*`), a relative word where the dir is not known, and the input of `xargs`
  unless a `find` with plain roots that are not critical gives it. Two forms are
  narrower: a pattern of a plain `rm` is critical only in `/`, `~`, a dir above `~` or
  a system dir (`rm *.o` runs), and `git clean` in the turn's own project root is not
  critical, because the project snapshot covers it. A literal `cd` before a command
  moves its dir (`Segment::cds`); because a `cd` can fail or run in a subshell, the
  dir before each `cd` counts too. A brace word such as `~/{p,x}` is expanded. A
  delete inside a script or a program is not seen. `auto` never reads the guard.
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

  From phase E, on a machine that runs the filtered bus proxy
  (`AutoSupport::bus_proxy`), `exits::views` also gives `Grant::BusFilter { bus }`,
  the call's own filtered bus, to a line with at least one read form of a systemd tool,
  in any place of the line (after `;`, `&&`, `|`, behind a wrapper): `systemctl` and
  `systemctl --user` with no verb or with `status`, `list-units`, `list-unit-files`,
  `list-sockets`, `list-timers`, `list-jobs`, `list-dependencies`, `is-active`,
  `is-failed`, `is-enabled`, `show` or `cat`; `loginctl` with no verb or a verb that only
  reports (`list-sessions`, `show-session`, `user-status` and the rest); `hostnamectl`,
  `timedatectl` and `localectl` with no verb, `status` or `show`, and `timedatectl
  timesync-status` and `show-timesync`. These are the forms whose recorded bus methods
  all lie in the proxy's default rules (`bus`). A form with `-H`, `--host`, `-M`,
  `--machine`, `--root`, `--image` or `--image-policy` (also a prefix of a long option
  or a cluster of short ones), or a word that only the shell can read, is no read form.
  `notify-send` is a read form of the session bus too (v0.0.4, DV18), except a form
  with `-w`, `--wait`, `-A` or `--action` (`notify_waits`), which waits for a signal
  that the filter never lets back. `predict` then finds no `bus` exit for the read
  form, and the view's reason is `Contain`. Every other bus form keeps its `bus` exit
  and approver: the other read verbs of `systemctl`, `systemctl --user start` and the
  like, `resolvectl`, `coredumpctl`, `busctl`, `gdbus`, `notify-send --wait`, `nmcli`
  and `iwctl`, and the model's `needs.bus`. A `bus` exit for the whole session bus
  (`Grant::Bus { bus: Session }`, from a program, `needs.bus` or a socket need of the
  bus: `/run/user/*/bus` or a socket that `Locations::with_session_bus` added, which
  efrd does for the `unix:path=` of `DBUS_SESSION_BUS_ADDRESS` and its real path) is `Approver::User` in every case, with the note `WHOLE_SESSION_BUS_NOTE` (`the
  whole session bus can start programs outside the sandbox`): the user manager starts
  programs outside the sandbox, and an unlocked keyring gives its secrets. The engine
  does not look for a keyring owner first. The whole system bus keeps its approver. The rule is a fit, not a security layer: `systemctl --failed; ./x.sh`
  gives the script the same filtered bus, and the proxy's rules carry the load. A view
  is no exit, so `job_refusal` never sees it: a job reads its own filtered bus. The
  conversation drops the bus view beside the network view and beside an approved
  `Grant::Bus` of the same bus.
- `jobs` (phase C): `job_refusal` names the exits that no background job may hold, in
  every mode and whatever the user answers: `privilege`, `masked_read`, `persistence`,
  a `desktop_ipc` form that types or starts programs, the whole message bus, and the
  socket of a service manager or a container engine (`SERVICE_SOCKETS`). The engine
  denies such a part of a job's line with `Cause::NotAsJob` (`JOB_REFUSAL`: run that
  part in the foreground, and the rest as a job). In every mode a socket of
  `SERVICE_SOCKETS` that the model names in `needs.sockets`, such as
  `$XDG_RUNTIME_DIR/systemd/private`, is a `privilege` exit that only the user approves
  (phase E), as a container engine's socket is: through it a call starts any unit
  outside the sandbox.
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
  `git_tokens` finds the remotes, `insteadOf` bases and `extraHeader`s of the git
  config that carry a credential, and the credential helpers that can hand a call a
  token: `helper_kind` sorts a helper value into a `HelperKind` (gh's
  `gh auth git-credential`, libsecret, `store` with or without `--file`, `cache`, Git
  Credential Manager, another shell form, or a program that efr does not know), and
  `HelperScope` holds the host of a scoped helper, or every remote for an unscoped one
  and for a scope whose host efr cannot read (v0.0.4). `HelperKind::denies` says
  whether a helper denies its hosts: gh's only when efrd finds that the call can reach
  gh's token. `Engine::with_hosts` gives the
  engine the list (`Engine::hosts`; empty by default). `FactRequest::remotes` names
  the directories of git commands that reach a remote, and `CallFacts` gets their
  remotes (`RemoteFact`, each URL a `RemoteUrl`) and `refused_hosts`, the hosts that
  the proxy refused. `CallFacts::reach_ports` holds the forwarded ports of the
  conversation's running jobs (v0.0.4 shell items, DS43): a contained call reaches
  them on `127.0.0.1` and `[::1]` through its reach helper, so a URL on `localhost`,
  `127.0.0.1` or `[::1]` with such a port is no network use, with or without the
  proxy. A command whose every URL is such a URL has no `host` exit, the shell tool's
  network flag adds none for a line whose every URL is one, and a `needs.hosts` entry
  of one makes no exit and gets a note. A missing fact is an empty set; the fact only
  predicts, the sandbox enforces.
  Under `Egress::Proxy` the prediction reads the list (DP20, DP21): a program that
  fetches over HTTP (a package manager, `curl`, `wget`, `git clone`, `npx` and the
  other runners) has no exit when its URL hosts are on the list, or when its line names
  no host; the proxy decides. A URL host that the list does not allow and does not deny
  is a `host` exit with `Grant::Host` of that one host and port, so the call does not
  fail with a 403 first (port 80, from `http://`, means plain `GET` and `HEAD`). A
  denied host has no exit, because the proxy refuses it. `localhost`, an IP address and
  a port other than 443 and 80 keep `Grant::OpenNetwork` (`ProxyRoute::Open`): the
  proxy cannot serve a dev server, a service on the host or the LAN, or a client that
  does not use `HTTPS_PROXY`. The raw network programs (`nslookup`, `dig`, `ping`,
  `nc`, `socat` and the rest of `RAW_NETWORK_PROGRAMS`) keep `OpenNetwork` in every
  phase. `needs.hosts` follows the same routes; a word that is no ASCII host name, a
  host on the list and a denied host are dropped, and `Decision::notes` tells the model
  why (the turn adds the notes to the tool result). With `Egress::None` every network
  need is a `host` exit with `OpenNetwork`, as before.
- `exits/remotes.rs` (phase D2, DP22): the git commands that reach a remote (`fetch`,
  `pull`, `ls-remote`, `remote update`, `push`, `submodule update`). `fact_requests`
  names the directory of each one in `FactRequest::remotes` (not for a line that sets a
  `GIT_*` variable or a git command with `-c`, `--git-dir` or `--work-tree`: the facts
  of the directory would be wrong), and the prediction picks the remotes that the
  command uses from `CallFacts::remotes`: the named one, else every remote for `--all`
  and `remote update`, else the current branch's remote, else `origin`; every
  submodule URL for `submodule update`. Each fetch or push URL's host joins
  `ExitNeed::hosts` of the line's `host` and `upload` exits, so the reviewer sees each
  host behind a remote name. Under the proxy a fetch from an https remote follows the
  list as a URL host does, and a fetch from an ssh remote is an `outside` exit with its
  hosts as names (`ClassifierWhenNamed`); an ssh submodule URL comes from
  `.gitmodules`, which the model can write, so only the user approves it. A directory
  with no fact keeps the line's own hosts. A `git push` with no host at all (its facts
  dropped, a `cd` first, a local path, a URL that efrd could not read) is for the user:
  the naming rule would have nothing to check.
- `exits/git_code.rs` (v0.0.4, B4): the git code of a line that runs git outside the
  sandbox. `fact_requests` names a `GitCodeAsk` in `FactRequest::git_code` for each git
  command that reaches a remote (also `send-email`, with no submodules) and each gh command that runs git in the repository
  (`pr create`, `pr merge`, `pr checkout`, `repo sync`, `repo create --push`, `repo fork
  --clone`), with the line's `-C` (each one from the one before), `--git-dir` and
  `--work-tree`, and whether the line can reach the submodules. efrd answers in
  `CallFacts::git_code` (`efr_protocol::GitCodeFacts`). The prediction makes each
  `outside` and `upload` exit of such a line `Approver::User` when a part is writable,
  when the fact says not checked, when no fact came, or when efr cannot tell the
  repository (a `cd` first, a variable); `ExitNeed::notes` names each writable part
  (at most three, then a count) or says that efr could not check it. A git option that
  chooses the code (`-c`, `--config-env`, `--exec-path`, `--git-dir`, `--work-tree`,
  `--namespace`, `--receive-pack`, `--exec`, `--upload-pack`, `--template`, `clone -u`
  and `clone -c`, and `send-email`'s `--to-cmd`, `--cc-cmd`, `--header-cmd`,
  `--sendmail-cmd` and an `--smtp-server` with a `/`) makes every `outside` and `upload` exit of the line `Approver::User`
  (DV50). So does a variable of a git or gh line that chooses the code
  (`git_code::sets_git_env`: a `GIT_*`, `SSH_*` or `AUTOSSH_*` name, `HOME`,
  `XDG_CONFIG_HOME` or `PATH`), because efrd checks with the hidden shell's variables.
  With no facts at all (no sandbox in efrd), the route stays.
- `exits/control.rs` (v0.0.5, OQS17): an ssh control socket is a logged-in session,
  and it, not the host of the line, decides where a command runs. So each exit of a
  command that makes or uses one (`ssh` or `autossh` with `-S`, `-M` (ssh only), `-O`
  or an `-o` of `ControlPath`, `ControlMaster` or `ControlPersist`; `scp`, `sftp` or
  `sshfs` with such an `-o`; `rsync -e` or `--rsh` with such an ssh) is
  `Approver::User`, with a note. A `needs.sockets` socket that efrd says an ssh process
  listens on (`CallFacts::ssh_sockets`, from `FactRequest::sockets`), or that a
  `ControlPath` pattern of `Locations` matches (`matches_pattern`: each `%` token and
  `${NAME}` matches any text), is `Approver::User` with the same note, and the
  always-allow check refuses a rule of it as a credential agent's socket.
- `exits/programs.rs` (v0.0.4, DV25): `npm`, `pnpm`, `yarn` and `bun publish` and
  every cargo upload verb (`publish` with or without `--no-verify`, `yank`, `owner`)
  are `upload` exits that only the user approves, with notes that say what they run.
  Under the proxy `ssh`, `mosh`, `autossh` and a git command with an ssh URL on its
  line (`git clone git@host:o/r.git`, `ssh://`) are `outside` exits too (the proxy
  carries no ssh): each carries its host in `ExitNeed::hosts` and as its name, so the
  classifier judges it only when a user message names that host (DP20 (e), (f)). A
  destination that efr cannot read, such as `$HOST`, is for the user, and so is a form
  that the line steers (`ssh_steered`, `git_steers_ssh`, `sets_ssh_env`): `ssh -F`,
  `-J`, `-W`, an `-o` that can reach another host or run a local command, `mosh --ssh`
  or `--client`, a git config value, exec path, template or submodule clone of the
  line, and a line that sets a `GIT_*`, `SSH_*` or `AUTOSSH_*` variable (which also
  sends a `git push` to the user). With no proxy they stay `host` exits with the whole
  network.
- `tables`: `SANDBOX_MASKS`, `PROTECTED_NAMES` and `PERSISTENCE_FLOORS`, which the
  engine and the daemon's sandbox planner share, as they share the secrets
  (`secret_paths`, `Locations::secret_paths`). `PERSISTENCE_FLOOR_GROUPS` gives each
  floor its `FloorGroup`, which decides the phrases that name its files. From phase E
  `~/.local/share/dbus-1` is a floor (group `Autostart`): the bus starts the program of
  a service file there outside the sandbox when a client calls its name, and the
  filtered bus proxy lets a call reach a listed name.
- `bus` (phase E): the rules of the filtered bus proxy. `DEFAULT_RULES` holds the read
  methods that the probe recorded for the read forms of `systemctl`, `loginctl`,
  `hostnamectl`, `timedatectl` and `localectl`, and the calls of `notify-send`
  (`Notify`, `GetServerInformation`, `GetCapabilities` of
  `org.freedesktop.Notifications`), each an exact member at a path (no
  member glob, no `IFACE.*`, no rule with a path alone). `GetUnitProcesses` and
  `GetUnitByPID` are not in it, and the session bus has no `Properties` rule at
  `/org/freedesktop/systemd1`, so the user manager's `Environment` stays out.
  `tests/fixtures/bus-recording.toml` holds the recorded calls, and a test keeps it and
  the table equal. `BusRule` is one `[[dbus.allow]]` entry; `check_rules` refuses an
  entry with `*`, `IFACE.*`, a path alone, no call, a name that is not a well-known bus
  name, the bus driver, `Properties.Set`, a protected service (`FLOOR_NAMES`: secrets,
  portals, polkit, Flatpak, kwallet, dconf), a member of a service of the defaults that
  does not start with the word `Get`, `List`, `Can` or `Lookup`, or a member of any
  other service that starts with a word of `WRITE_MEMBER_PREFIXES`, without case
  (`PermissionsError::BusRule` with a `BusRuleProblem`). `BusFilter::compile` lays the
  entries of one bus over the defaults, and `BusFilter::proxy_args` gives `--filter`,
  `--log` and one `--call` per rule, never `--talk`, `--own`, `--see` or
  `--broadcast`.
- `always` (phase E): `AllowRule`, one `[[sandbox.allow]]` always-allow rule: `host`
  (one host and port), `write` (a directory, with an optional `project`) or `socket`
  (one socket path). `AllowRule::describe` gives the rule in words, and
  `AllowRule::check_shape` checks what needs no fact about the machine
  (`PermissionsError::AllowRule` with an `AllowRuleProblem`): one ASCII host name that
  is no pattern, IP address or `localhost`, a port above 0, and paths that are absolute
  or below `~/` with no `.` or `..` part. `CallFacts::real_paths` gives the real path of
  each write grant path and named socket of a call (`FactRequest::real_paths`: each
  path that the line, `needs.write` or a declared write writes below `~`, with its
  parents below `~`, and each socket of `needs.sockets`).

  `Engine::with_allow` checks each rule against the engine's locations and host list
  (`Engine::check_allow`, `always/check.rs`), so the daemon sets the rules last. A rule
  that fails is kept, disabled with its `AllowFloor`, and never covers a call: it never
  fails the engine, because a rule can turn bad with no edit (a new project, a new
  dotfile link). `Engine::allow_rules` lists the rules with their marks. A `host` rule
  may not name a host of `network.deny`. A `write` rule may not name `~`, a dir above or
  outside it, a secret, efr's config or own dirs, a mask, a synced folder, a git dir, a
  floor or a dir below one, a dir above a built-in floor of the home directory (such
  as `~/.config`), a write root or a dir above one, or a dir of more than one project. A
  `socket` rule may not name the socket of a service manager or a container engine, a
  path below `$XDG_RUNTIME_DIR/systemd/`, a bus, a credential agent, a desktop socket, a
  socket in a real write root, a mask, a secret or efr's own dirs.

  `Engine::always(decision, project)` gives the `AlwaysVerdict` of one call
  (`always/cover.rs`): `Covered { index }` when one enabled rule covers every exit of
  the call, else `Offer(rule)` with the rule that an `a` answer would save (it passes
  `check_allow` and covers the call), else `None`. Only an `auto` shell call whose only
  deciding reasons are its exits may be covered, and only when each exit is a `host`,
  `write` or `socket` exit that the classifier judges on its own, runs in the sandbox,
  holds only narrow grants (one `Grant::Host`, a contained `Grant::Write` bind that is
  no `ExitChild`, one `Grant::Socket`) and comes from no cache delete. So privilege,
  persistence, upload, desktop, masked-read, synced, above-root and outside exits, the
  bus and `OpenNetwork` are never covered. The apply checks: every write and socket
  grant needs its real path from the facts; a `write` rule covers a bind whose real path
  lies at or below its dir (and only in turns of its `project`, when it names one); a
  dir bind (`Target`, `Parent`, `MakeDir`) with a floor, a synced folder or a registered
  project at or below it, also one that does not exist yet, is never covered, while a
  file bind (`TargetOnly`, `MakeFile`) stays eligible; a socket that is a link or lies in
  a write root is never covered; and a user `deny` or `ask` rule for writes that names a
  grant path, a path below it or a dir above it (or `network` of every call, for a
  host) leaves the call to the classifier. A phone turn runs as `cautious`, which has no
  exits, so it never uses a rule.

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

- The floors hold in every mode: a privileged program asks, an interactive call asks
  (but in `auto`, where it runs contained, and in full mode, where it runs), a
  remote origin runs with at most `cautious` and asks outside `$SCRATCH`, secrets stay
  denied unless a user rule names them, a conversation's rules only tighten, and no
  tool writes a write-sealed root.
  `runs_as_another_user` reads a line for a privileged program as the engine does, so
  the check point makes the question of such a line local only in every mode.
- A delete or a move of a file (`Requirements::destructive`) asks in every mode, from
  every origin and also when every rule allows everything, on its own reason
  (`Subject::Destructive`, `Cause::Destructive`). The one exception is a turn in `auto`
  or full mode on a machine with undo, when the facts say that `efr undo` can bring
  back every path that the call writes. Its paths are judged as writes on their own reasons, so a
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
  has none), and on that machine with efr's proxy and the default allow list the phase
  D2 result (`pd2`, the phase D1 result where a row has none: the package fetches,
  `git fetch` and `git pull` with no remote fact, and a URL of a listed host run with
  no exit), and on that machine with the filtered bus proxy the phase E result (`pe`,
  the phase D2 result where a row has none: rows 8 to 11, the read forms of
  `hostnamectl`, `systemctl` and `loginctl`, run with the view `Grant::BusFilter` and no
  exit); the `p1` column keeps the phase 1 result. A phase B result differs only
  where the snapshot covers a destructive line, which becomes routine, and a phase D1
  result only for `df`, which reads the mount view in the sandbox. With always rules
  that name their resources, rows 79, 80, 85 and 89 still ask, and no rule covers them
  (`corpus_79_80_85_89_ask_with_bus_proxy_and_rules`).
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
  (`Cause::NotAsJob`). Outside `auto` and full mode each job asks the user
  (`Subject::Job`, `Cause::JobAsks`), whatever the rules say, and a remote turn runs as
  `cautious`, so it asks too (`src/engine/tests/jobs.rs`, and the corpus tables
  `background_flag_does_not_change_prediction` and `job_routes_match_the_corpus`,
  which checks the `job` column of each row on the phase E machine). A full job starts
  with no question, and also refuses a `destructive` exit and a delete that full mode
  guards; with no sandbox there is no filtered bus, so a read form of the bus is a `bus`
  exit, which no job may hold.
- In full mode (`src/engine/tests/full.rs`, `src/full/tests.rs`) the effect is never
  `Contain`, and a call asks only for a privileged program, a `privilege`,
  `persistence` or `destructive` exit, a delete that full mode guards, a change of
  efr's settings, a file tool delete that undo does not cover, a read or write of a
  directory with a secret or efr's config below it (no sandbox masks them), or a
  user's or a conversation's rule (property `full_never_contains`,
  `full_tree_read_with_a_secret_below_asks`, `full_tree_write_over_efr_config_asks`). Every other exit runs and stays
  in `Decision::exits`. Each floor of `auto` holds: a secret, efr's config and efr's
  own dirs stay denied, also when the delete of such a dir is a guarded delete too
  (the denial is stricter). No rule lifts a guarded delete's question; a user's `deny`
  rule still denies it. `nested_shell` runs. The decision does not change with the
  sandbox support of the machine. `tests/it/corpus.rs` checks the `full` column of each
  corpus row on the phase E machine and on one with no sandbox support
  (`full_corpus_rows_match`): the derived result of its phase D1 result, or the
  explicit `G` of rows 49, 53, 60, 95 and 106, the deletes that full mode guards; no row
  goes to the classifier, the floor rows are those of `auto`, and `auto` keeps every
  result (`auto_is_unchanged_by_full`).
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
  allows `git status; rm -rf ~`. A line that `command::analyze` cannot split is
  allowed only by the rules whose resource is `any`, which ask by default (property: no
  line with a `$(` or backquote outside single quotes is allowed by the defaults); a
  command rule that asks or denies can still make it stricter through the commands
  that the lenient split finds in it.
- A rule that asks or denies is never looser after v0.0.5's readings (OQL4, OQL9)
  than in v0.0.4 (property, `policy/tests/places.rs`): a match through a reading of
  the global options or through a dir that efr cannot prove only makes the effect
  stricter.
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

## Benches

Ignored tests that `just bench-perf` runs in release. They use the bench helpers of
`efr-test-support`, a dev-dependency only. A size that takes more than 10 s stops its
ladder, and a ladder that stops fails its bench. A cost that grows with the line has a
budget on its slope: below 1.3, where a linear cost has about 1. Each bench writes its
report before it checks its budget. These tests read the clock, and the scan bench
starts processes.

- `bench_perf_scan_nesting` (`src/exits/scan/tests.rs`): the lenient scan on `$(` 100,
  1k, 10k and 50k times, with and without the closing parens, on a thread with a
  stack of 2 MiB. Each depth runs in a child process (this test binary again, with
  `EFR_BENCH_SCAN_CASE`), because a stack overflow kills the process. Budget: every
  depth returns, with one segment for each level that the scan reads (64 at most) and
  one for the rest, and the slope stays below 1.3.
- `bench_perf_run_forms` (`tests/it/perf.rs`): `Engine::decide` with 20 user `ask`
  rules on `rm f1..fN` and `time rg x f1..fN`, N = 100, 1k, 5k and 10k, with the
  growth of the peak resident set. Budget: the slope of each line.
- `bench_perf_git_forms` (`tests/it/perf.rs`): `Engine::decide` with an `ask` rule
  for `git push` with `under` on `env` and then `git` N times, N = 100, 1k, 5k and
  10k. Each git form read its views over the rest of the line: 10k took 0.8 s.
  Budget: the line asks (the rule reads at most `MAX_FORMS` forms), and the slope.
- `bench_perf_rule_readings` (`tests/it/perf.rs`, v0.0.5): `Engine::decide` in
  `cautious` with `allow` for every command line and `ask` rules for
  `systemctl restart` and for `git push` with `under`, on three lines of N words, N =
  100, 1k, 5k and 10k: `systemctl` with N unknown options before `restart` (OQL4), N
  `cd`s before `git push` (OQL9), and `git push` with a substitution of N words, which
  `analyze` cannot split (OQL9). v0.0.4 allowed all three. Budget: each line asks,
  and the slope of each line. In `auto` the line of 10k `cd`s takes about 10 s,
  because each segment of the lenient split (`exits/scan.rs`) copies the targets of
  the `cd`s before it. The bench leaves the exits out.
- `bench_perf_fact_requests` (`tests/it/perf.rs`): `fact_requests` on `rm -rf` and
  `touch` lines of 100, 1k and 5k operands. Budget: the slope of each line.
- `bench_perf_decide_paths` (`tests/it/perf.rs`): `Engine::decide` on `rm -f` of 100,
  1k and 6,400 files with each file declared as a write and its facts, as efrd gives
  them, in the project and outside it. Outside the project each file is an exit, and
  each exit looks up the facts of its path in maps (`exits/facts.rs`), not in the
  lists of `CallFacts`. Budget: the slope of each place. Each exit outside still holds
  its own copy of the text of the `rm` command, which is most of the rest of the cost.
- `bench_perf_decide_line` (`tests/it/perf.rs`): the base cost of one
  `Engine::decide` and one `fact_requests` on a typical line and on a line of 100 KB.

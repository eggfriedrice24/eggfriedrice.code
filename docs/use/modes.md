# Permission modes

<!-- sources: README.md "The auto mode", "The full mode", "Use" (`,mode` bullet, approvals); docs/permissions.md "Permission modes", "How the engine decides", "Path classes", "The built-in rules", "Full mode"; docs/config.md [permissions]; shell/zsh/efr.plugin.zsh (`,mode`, EFR_MODE); crates/efr-cli/src/turn_settings.rs; crates/efr-cli/src/commands/settings.rs; crates/efr-cli/src/format/sandbox.rs; crates/efr-permissions/src/policy.rs, full.rs; crates/efr-daemon/src/full.rs; docs/adr/0015-full-mode.md -->

Before a tool call of the agent runs, efr decides what to do with it. The call runs,
it runs in the `auto` sandbox, it waits for your answer, or efr denies it. The
permission mode of the turn gives the built-in rules for this decision.

There are four modes:

| Mode | What runs with no question | What asks you |
|---|---|---|
| `manual` | only what your own rules allow | every read, write, command and network access |
| `cautious` (the default) | reads outside secrets, writes in `$SCRATCH` and in the turn's registered project, and read-only commands such as `ls`, `cat` and `rg` | every other write and command |
| `auto` | every shell command, in a kernel sandbox that can write the project | an action that leaves the sandbox, when a reviewer model does not decide it |
| `full` | every command, as you, with no sandbox | `sudo`, a process that outlives the turn, a delete of `~`, `/`, a system dir or a project, and some deletes and dir reads (see below) |

In every mode, efr denies secrets and a write of its own config.

## Choose a mode

The mode of a turn comes from the first of these that gives one:

1. A flag for one prompt: `efr send --mode auto <prompt>` or `efr new --mode auto
   <prompt>`.
2. The choice of the terminal: `,mode auto`.
3. `permissions.mode` in `config.toml`.
4. The default, `cautious`.

`,mode auto` sets the mode for the prompts of this terminal only. Other terminals
keep their own choice, and `,new` keeps the choice of its terminal. efr keeps a value
only when the daemon accepts it. `,mode default` clears the choice, so the config
file decides again. Tab completes the names of the modes.

`,mode` alone shows the mode that the next prompt gets, where it comes from, and the
choices:

```text
mode = auto  # this terminal (,mode); choices: manual, cautious, auto, full
```

To set the default for every terminal, change the config file:

```sh
efr config set permissions.mode auto
```

The daemon reloads the file, and the next turn uses the new value.

`EFR_MODE` gives a new shell its first mode, as `EFR_MODEL` and `EFR_EFFORT` do for
the model and the effort. The zsh plugin ignores `EFR_MODE=full` and says so:
`efr: EFR_MODE=full is ignored here; use ,mode full`.

A turn keeps its mode until it ends. A steer with `,!` takes no settings.

### Where you see the mode

- In sticky mode, the choices of the terminal stand as a dim tag before the robot,
  such as `auto gpt-5.4 🤖`.
- When a prompt asks for a setting that the config does not give, the first line of
  the reply names it after the scope, such as `app · mode auto, model gpt-5.4`.
- A full turn starts with a red line and shows `full` in red in the status row.

## Registered projects

`cautious` and `auto` write freely in a registered project. A registered project is a
directory that you list with `efr project add`:

```sh
efr project add             # the git work tree of the current directory
efr project add ~/dotfiles  # a directory that you name
efr project list
efr project remove ~/dotfiles
```

A turn whose hidden shell is in a project's root, or below it, runs in that project.
The next tool call uses the new set of projects, with no restart. Your home directory
and `/` are a project only when you name them. `auto` never uses your home directory
as a project: register a narrower directory.

## manual

`manual` asks before each read, write, command and network access, except what your
own `[[permissions.rules]]` allow. Secrets are denied. Use it when you want to see
each step.

## cautious

`cautious` is the default. With no question, the agent can:

- read files outside the secrets;
- write in `$SCRATCH`, the scratch directory of the conversation;
- write below the root of the turn's registered project;
- run read-only commands, such as `ls`, `cat`, `head`, `rg`, `grep`, `df`, `ps`,
  `git status`, `git diff` and `git log`.

Every other write and command asks you. A write to the root of the project itself,
or to a directory above it, asks too, because it can replace or remove the whole
project. [Permissions and rules](../permissions.md#the-built-in-rules) lists each
read-only command.

## auto

In `auto`, each shell command of the agent runs at once in a kernel sandbox. The
sandbox holds the command and every program that it starts: build scripts, tests and
git hooks too. The command can write the turn's project, `$SCRATCH`, a private `/tmp`
and private copies of the tool caches. It reaches the package registries and GitHub
through efr's network proxy.

An action that leaves the sandbox is an exit, such as `sudo`, `git push` or a write to
`~/.zshrc`. A reviewer model decides most exits from your messages. Some exits always
ask you. The file tools (`read_file`, `write_file`, `edit`, `apply_patch`) follow the
rules of `cautious`.

`auto` needs Linux 7.1 or newer and bubblewrap. Without them, `auto` runs as
`cautious`, and efr says why. `,mode auto` warns at once, such as:

```text
efr: auto needs the sandbox; turns run as cautious: Landlock ABI 6 found; auto needs 9 (Linux 7.1). efr sandbox check shows more.
```

Each turn then starts with a dim line, such as `auto is not available here; this turn
runs as cautious: Landlock ABI 6 found`.

[The sandbox](sandbox-basics.md) tells what the sandbox allows, and
[The network proxy](network.md) tells what network it has.

## full

In `full`, the agent's commands run as you, with no sandbox and no questions. Full
mode is a protection against mistakes. It is not a protection against a hostile model
or a prompt injection. Where `auto` can run, `auto` is the safer choice.

### Turn it on

1. Allow full mode in `config.toml` yourself:

   ```sh
   efr config set permissions.allow_full true
   ```

   The settings tool of the agent never changes this key.
2. Choose full mode where you want it:
   - `,mode full` for the prompts of one terminal, and `,mode default` to stop;
   - `efr send --mode full` for one prompt;
   - `permissions.mode = "full"` next to `permissions.allow_full = true` for every
     turn that names no mode.

`,mode full` asks the daemon first. When the config does not allow full mode, it fails
with the reason, and the terminal keeps its old mode. If not, it prints three lines:

```text
efr: full mode: commands run with your rights, with no sandbox and no questions.
efr: efr reads each line: it denies secrets and its own config, and asks for sudo, for a process that outlives the turn and for a delete of ~, / or a project.
efr: a script or a program that a line starts is not read; it can do what you can.
```

`efr status` shows `full mode` with `allowed`, `off (permissions.allow_full is
false)` or `refused (<reason>)`.

### What efr still does in full mode

- It reads each command line before it runs. It denies secrets, its own config and its
  own dirs.
- It asks before `sudo`, before a process that outlives the turn (such as
  `systemd-run --user`, `crontab` or a change of `~/.zshrc`), and before a delete of
  `~`, `/`, a system dir or a project. It also asks before a delete that `efr undo`
  cannot revert, and before a read or write of a whole dir that holds a secret or
  efr's config.
- Only your own machine answers the `sudo` question and the delete question. A phone
  can answer the question about a process that outlives the turn. A delete question
  waits 2 minutes.
- It takes a snapshot before each call, so `efr undo` reverts the project and the
  files that efr saw.
- At the end of a turn, one dim line lists what efr saw full mode let through, such as
  `efr saw 2 actions that full mode let through: write ~/notes.txt, upload (git
  push)`.

efr does not read a script or a program that a line starts, such as a build step or
`python3 -c`. Such a program can do what you can do: read a secret, send an API key
to a host, or write `config.toml`.

### Where full mode never runs

- A turn from the phone runs with at most `cautious`.
- A turn that a job notice starts runs with at most `auto`.
- efrd refuses full mode when it runs as root, when `snapshot.enabled` or
  `undo.enabled` is `false`, and when the hidden shell is no zsh.

When efrd refuses full mode, a prompt that names full mode fails with the reason, such
as `full mode cannot run: efrd runs as root`. A turn that gets less than full mode for
another reason starts with `this turn runs as <mode>: <reason>`, such as `this turn
runs as auto: a turn that a job notice starts runs with at most auto`.
[Permissions and rules](../permissions.md#full-mode) has every detail of full mode.

## What holds in every mode

- Secrets are denied, unless a rule of yours names them. The daemon's own `secrets/`
  is denied even then.
- No tool writes efr's config. A change of efr's settings through the settings tool
  asks you, with the diff of the file.
- A program that runs commands as another user (`sudo`, `doas`, `su`, `pkexec`,
  `run0`) asks, and only your own machine answers.
- Your own `[[permissions.rules]]` come after the built-in rules of the mode, so your
  rules win where they match.

## Answer a question

When the agent asks for approval, `y` allows and `n` denies. Ctrl+C interrupts the
turn. For a long command line, the question names the commands in it that ask, such
as `asks for: hostnamectl, systemctl --failed`.

When you are away, after `,away` or 10 minutes with no input, a question waits only
5 minutes. Then its call does not run, and the agent names it in its answer.

## See also

- [Permissions and rules](../permissions.md): the engine, the path classes and your
  own rules.
- [config.toml reference](../config.md#permissions): the `[permissions]` keys.
- [ADR 0015](https://github.com/eggfriedrice24/eggfriedrice.code/blob/main/docs/adr/0015-full-mode.md): the decision for full mode.

# Background jobs

<!-- sources: README.md "Use" (the jobs bullet, the Ctrl+B bullet, the notice line); docs/sandbox.md "Background jobs" (all subsections, with "Ports of a job", "Moving a call to the background", "Undo and jobs"); docs/permissions.md "Background jobs"; docs/config.md [jobs]; crates/efr-cli/src/cli.rs (jobs); crates/efr-cli/README.md (the efr jobs row, "The input row": Ctrl+X, Ctrl+B); crates/efr-cli/src/commands/jobs.rs and its tests; docs/adr/0010-background-jobs.md, 0016-job-port-forwarding.md, 0018-moving-a-call-to-the-background.md -->

A background job is a command that runs on after its call and after its turn, such as
a dev server, a watcher or a slow build. The agent starts a job when a command takes
long or must keep running. You can also move a running command of the agent into a
job with Ctrl+B.

## How a job starts

The agent starts a job with a shell call and `background: true`. The call answers at
once with the job's id (`j1`, `j2` and so on in each conversation) and its output
file. While you follow the turn, the view shows:

```text
job j3 started; output $SCRATCH/jobs/j3.out; ends in 60 minutes
```

`&`, `nohup` and `setsid` in a normal command do not make a job. Their processes stop
when that command ends.

### Jobs and the permission modes

A job can start in every mode:

- In `auto`, the job's command line goes through the same checks as any line. A line
  with no exit starts at once in the sandbox. An exit goes to the reviewer or to you,
  and a grant lasts for the whole job.
- In `cautious` and `manual`, efr asks you before each job starts. An approved job
  runs outside the sandbox, with your rights.
- In `full`, a job starts with no question and runs with your rights.

The question of a job names its lifetime:

```text
runs as a job for up to 60 minutes; these grants stay open until it ends
```

Only a terminal of yours can answer this question, never a phone. While you are away,
the question expires, and no job starts.

### What cannot run as a job

In every mode, efr refuses a job whose line needs one of these. A job has no terminal
for a password (its stdin is `/dev/null`), and a process that such a grant starts can
outlive the job:

- `sudo` or another way to get more rights,
- a read of a masked file, such as a project `.env`,
- a change of a file that runs code later, such as `~/.zshrc`,
- a desktop command that types or starts programs,
- the whole D-Bus bus, or the socket of a container engine or a service manager.

The agent then reads `this cannot run as a job; run that part in the foreground, and
the rest as a job`. Your yes does not change this.
[Permissions and rules](../permissions.md#background-jobs) has the full rules.

## How a job runs

- A job runs at the lowest CPU priority (`jobs.nice`, 19) and the idle disk priority.
- It starts in the directory of the hidden shell, with the environment of the hidden
  shell.
- In `auto`, a job writes where the sandbox of its turn writes. A job in the sandbox
  cannot commit: the agent commits in the foreground.
- efr writes the job's output to `$SCRATCH/jobs/<id>.out`. The file keeps the first
  1 MiB and the last 8 MiB of the output. A command in the sandbox can read this file,
  but it cannot change it.
- efr removes the output of a job 7 days after the job ends.

Some programs keep their output in a buffer when it goes to a file. Run them with
`stdbuf -oL`, or with their own flag for line output.

### Limits

| Setting | Default | What it sets |
|---|---|---|
| `jobs.enabled` | `true` | `false` refuses every new job. Jobs that run go on. |
| `jobs.default_minutes` | `60` | The time limit when the agent names none. |
| `jobs.max_minutes` | `480` | The longest time limit that the agent can ask for. |
| `jobs.max_running` | `4` | The most jobs that run at one time in a conversation. |
| `jobs.max_running_daemon` | `16` | The most jobs that run at one time in all conversations. |
| `jobs.nice` | `19` | The CPU priority of a job, from 0 to 19. |
| `jobs.forward_ports` | `true` | Forward the ports of a job in the sandbox. |

[config.toml reference](../config.md#jobs) lists every key.
[ADR 0010](https://github.com/eggfriedrice24/eggfriedrice.code/blob/main/docs/adr/0010-background-jobs.md) records the decision for jobs.

## Open a dev server in your browser

In `auto`, a job runs in the sandbox with its own network. When such a job listens on
a TCP port, efr listens on the same port of your `127.0.0.1` and sends each
connection to the job. So your browser opens the job's server at
`http://localhost:8765`. The view shows:

```text
job j1 listens on 127.0.0.1:8765 -> http://localhost:8765
```

Nobody is asked: a forward only lets your programs connect to the job. The job gets
no new right on your machine.

- Only the programs of your own user can connect.
- A port counts when it listens for 1 second.
- efr forwards TCP only, not UDP.
- The forwards end when the job ends.

efr does not forward some ports, and the view says why:

- a port below 1024;
- a port of a service where a client can send a password or a token first, such as
  5432 or 6379;
- a port that another program of your machine, or another job, uses;
- more than 8 ports of one job, or more than 32 ports of all jobs.

```text
job j1 listens on 127.0.0.1:8765; not forwarded: 127.0.0.1:8765 is in use on this machine
```

The agent reads each of these results too, in its next step, as a short note from
efr. So it gives you the right address, or it starts the server on another port.

A later call of the same conversation in the sandbox reaches the job's server on the
same port. So `curl http://localhost:8765` in a call checks the dev server that the
agent started. A call in the sandbox that names a port of its job that efr does not
forward does not run, and nobody is asked: full network would reach the other program
on that port, not the job. The agent reads why. A job outside the sandbox already listens on your loopback, so it gets
no forward. [Ports of a job](../sandbox.md#ports-of-a-job) has the full list of ports
and rules.

## How a job ends

A job ends when one of these occurs:

- its command ends;
- you stop it, or the agent stops it;
- its time limit passes (60 minutes unless the agent asks for more);
- efrd stops.

Esc and the end of a turn never stop a job. The time limit counts the time on the
clock, so a suspend of the computer counts too.

### Stop a job

- In the input row of a turn that you follow, Ctrl+X stops the newest running job of
  the conversation. efr asks first: `? stop job j3 (npm run dev)`, then `y` or `n`.
- `efr jobs stop j3` stops a job and shows its last 10 lines of output.

### Notices and wake turns

When a job ends, efr tells the agent with a job notice. A job notice is never your
input, and it never counts as your permission.

When the conversation is idle, a notice starts a short turn of its own, a wake turn.
The turn is marked `🤖 (job notice, not your input)`. A wake turn runs at most 20 tool
calls. Outside `auto`, its commands ask you, as in any turn of that mode. At most 6
wake turns start in a conversation in an hour (`jobs.max_wakes_per_hour`), and at most
12 in all conversations (`jobs.max_wakes_per_hour_daemon`). `0` means that a notice
never starts a turn; it waits for your next prompt.

The agent can also ask to hear about an output line that matches a pattern, such as
the line where a dev server is ready. Each pattern can wake the agent at most 3 times
(`jobs.max_wakes_per_watch`).

Between turns, your next prompt shows one line for each job that ended:

```text
efr: job j3 ended (exit 0): <title>
```

## List your jobs

`efr jobs` lists the jobs of this terminal's conversation, newest first:

```text
j2  running         ends in 3h 0m      0 B  npm run dev
j1  stopped by you  ended just now  2.0 KB  cargo build --release
```

A running job with forwards shows its ports, such as `(ports 8765, 5173)`. `--all`
lists the jobs of every conversation. `efr history` shows each start and end of a job,
and `--verbose` adds the last lines of each output.

## Move a running command to the background

When a shell call of the agent takes long, such as a slow build or a test run, press
Ctrl+B in the input row. The command runs on as a job, and the turn goes on at once.

- Ctrl+B acts also when the input row holds text. The row keeps its text.
- In tmux, press Ctrl+B twice, because tmux takes the first one.
- While the row is empty and the call ran for 2 seconds, the hint of the row starts
  with `ctrl+b background`.

A call in the sandbox moves at once, with no question. It keeps its sandbox, so the
move allows nothing new. A call outside the sandbox asks first, because the command
keeps running with your rights after the turn:

```text
? move this command to the background
│ ...
│ it keeps running with your full rights after the turn, for up to 60 minutes
│ y move · n no
```

The view then shows `(moved to job j3)` in place of the result. The agent reads the
output so far, the job's id and the output file. A job notice tells the agent when the
command ends.

A moved command is different from a job that the agent starts:

- Its time limit (60 minutes) counts from the move.
- It gets no port forwards. To open a server in your browser, the agent must start the
  server as a job.
- The next call of the agent gets a new hidden shell in the same directory. The
  variables, functions and aliases of the old shell do not move to the new one.
- Nobody follows a moved command. When it asks for a password, efr stops it.

Some commands cannot move. The view then says `the command was not moved: <reason>`:

- a line with `sudo`, `ssh` or another program that reads the terminal;
- a command that waits for input now: answer it first;
- a line that efr does not allow as a job;
- a call in the sandbox that holds the sandbox's tool cache;
- the most jobs that efr allows run now.

[Moving a call to the background](../sandbox.md#moving-a-call-to-the-background) has
the details.

## Jobs and undo

`efr undo` refuses while a job of the conversation runs:

```text
job j3 runs; stop it first (efr jobs stop j3)
```

When a job ends, its file changes join the turn that started it. The undo of that turn
then needs `--force`. [Undo and diff](undo-and-diff.md) tells how undo works.

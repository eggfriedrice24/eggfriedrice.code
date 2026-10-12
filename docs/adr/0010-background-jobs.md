# 0010: Background jobs start from efrd, outlive their turn and never speak for the user

Status: accepted, 2026-10-12.

## Context

Phase C of the auto spec (sections 3.18, 13.8 and 16.5) adds background jobs: a
`shell` call with `background: true` starts a command that runs on after its call and
its turn, such as a dev server, a watcher or a slow build. Before phase C, every
process of a call ended with the call. A job breaks that rule on purpose. It holds
grants for minutes or hours while nobody watches it, it writes output that the model
reads later, and its end can start a turn that no user started. These choices are
expensive to reverse: they set which process starts a job, where its files live, how
it ends, and what its output can do in the conversation.

## Decision

1. **efrd starts the launcher, not the hidden zsh.** A foreground call runs as a job of
   the hidden shell, which holds a terminal. A job must not hold a terminal, must not
   block the shell, and must outlive the call. So efrd spawns `efr-sbx run --job`
   itself, with its own scrubbed environment, and hands the job the hidden shell's
   environment through a file (`env`) that the zsh integration writes at each prompt.
   `--job` needs a spec with a job plan, and a spec with a job plan needs `--job`, so
   the shell's wrapper can never start a job, and a job never runs on the terminal.

2. **efrd owns the output pipe.** The job's stdout and stderr are a pipe that efrd
   reads. efrd writes `$SCRATCH/jobs/<id>.out` (the head and the tail) below the
   descriptor of the dir, with no link followed, and every contained call reads that
   dir but cannot write it. A file that the job writes itself could be cut, changed
   or replaced by the job or by a later call, and the notice would then tell the model
   a forged end.

3. **The call dir is `$R/sbx/<conversation>/jobs/<call>/`.** The checks for a running
   call (undo, the cache collector) read the direct entries of the shell dir. A job
   there would count as a call that runs for hours, and undo and the collector would
   wait for it.

4. **Each contained job has cache layers of its own on the disk.** One layer lock per
   conversation would make every foreground call wait for a job, and a layer on tmpfs
   would fill memory with a build. The job sees the user's cache as its lower layer,
   and efr never applies a job's cache deletes.

5. **A lifeline, not `PR_SET_PDEATHSIG`.** The launcher's stdin is a pipe whose write
   end only efrd holds. The launcher starts nothing before a start byte, which efrd
   writes after the launcher is model-side on the socket, and it stops the job at the
   end of the pipe. `PR_SET_PDEATHSIG` fires when the thread that forked ends, and the
   blocking threads of tokio end when they are idle.

6. **A sweep by session id, not yet a cgroup.** The launcher calls `setsid`, so its pid
   is the job's session id. Every stop ends with SIGKILL to each process that still has
   that session, and the session stays model-side on the socket while any process has
   it. A process that ends its launcher and then calls `setsid` escapes the sweep. A
   cgroup per job closes that, but it changes the systemd unit (`Delegate=yes`), so it
   waits for a later phase. The docs name the limit.

7. **A notice is an event of its own and never enters a running turn.** A job's end is
   `job_ended`; the notice that the model reads is `job_notice`, which a turn records
   before its `turn_started`. It is never `prompt_queued` or `turn_steered`, so it is
   never in the messages that the reviewer reads, and the naming rule never finds a
   name in it. The text escapes the job's output and drops control and format
   characters. A notice waits for the end of a running turn: a steer would put output
   of a program in the middle of a turn that a user watches.

8. **A job of a wake turn never wakes.** A wake turn is a turn that a notice started
   in an idle conversation in `auto`. If its jobs could wake the conversation, wake
   turns could chain for hours with nobody there. So a job that a wake turn starts
   holds its notices for the next prompt, and the wake turns of an hour are counted
   per conversation and for the daemon, over restarts.

9. **Jobs start in every mode; outside `auto` each job asks the user.** In `cautious`
   and `manual` no reviewer decides a job, and no rule of the mode skips the question.
   Some kinds never run as a job in any mode: privilege, masked reads, persistence,
   desktop commands that type or start programs, the bus, and container engine or
   service manager sockets. A process that such a grant starts is not in the job's
   tree, and it outlives the stop and the time limit.

## Consequences

- A job's grants stay open for its lifetime, up to its time limit. The question and
  the reviewer's record say so, and a phone never answers a job's question.
- A job in the exit child runs as the user. It holds no terminal and no descriptor of
  one, but it can open a terminal by its path and read efr's runtime files. A thin
  mount namespace for such jobs waits for the mount view of a later phase.
- Undo refuses while a job writes in its roots, and a job's writes join its start turn
  at its end. See ADR 0009.
- A dev server in a contained job listens on the sandbox's own loopback. The user
  cannot reach it before a grant to listen comes.
- The hidden shell writes its exported values, secrets too, to a file in the runtime
  root. The sandbox masks that root, and the file is 0600.

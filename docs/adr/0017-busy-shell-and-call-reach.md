# 0017: A busy hidden shell, and the reach of a contained call

Status: accepted, 2026-10-21.

## Context

Two live findings of v0.0.4 led to this record.

**A busy hidden shell.** In `auto`, the model ran `rsync -a ... -e 'ssh -o
PreferredAuthentications=password' u@host:... $SCRATCH/...`, an upload that the user
approved, with the model's timeout of 30 s. efr read only program names to decide
whether a line can wait for a password, so the call kept 30 s. At 30 s the call
answered the model, and rsync went on in the hidden shell and waited for a password
that no client showed. efr kept only the process of such a command: no call, no line,
no wait. No look ran for it, no answer could reach it, and nothing ended it. Each later
call of the conversation waited for its own timeout and then failed with a text that
told the model about `nested_shell`. The model told the user to type into a prompt
that did not exist. The same work found a gap of today: a sandboxed command can turn
line mode off and print a password prompt, and efr then sent the user's answer into
the sandbox.

**The reach of a contained call.** ADR 0016 forwards the loopback ports of a contained
job to the host. The agent's own check of the server, `curl http://127.0.0.1:8765` in a
later contained call, asked for the whole network: each contained call has a network
namespace of its own, with only `lo`.

These choices are expensive to reverse. The first changes who can answer a command and
when, and keeps a command that waits for a password alive after its call. The second
puts one more helper into each contained call of a conversation with a forwarding job,
and efrd decodes more frames from the sandbox.

## Decision

1. **Two flags, not one.** The hard flag `interactive` keeps its effects for the
   programs that read the terminal (`sudo`, `ssh` and the others): a question outside
   `auto`, a job refusal, the long limit and the words of the question. A new soft
   flag `may_prompt` marks a line that may ask for a password through ssh: `rsync` with
   a remote operand or a remote shell (`-e`, `--rsh`, `RSYNC_RSH`), `scp`, `sftp`,
   `sshfs`, `ssh-copy-id`, `mosh`, `autossh`, `ssh-add`, `ssh-keygen` without `-N`, and
   `git` over the network, also with `GIT_SSH_COMMAND`, `GIT_SSH` or `core.sshCommand`.
   The soft flag adds no reason, no question and no job refusal. It gives the long
   limit to a call outside the sandbox, and the question of such a call says that the
   user may need to type a password. Adding these programs to the hard list would
   break an allow rule for `git pull` in `cautious` and `background: true` for
   `git clone`.

2. **Past the timeout, a run that waits for input goes on while a person follows.**
   Every shell call gets an input limit, `shell.interactive_timeout_minutes` (60). Past
   its timeout, a run goes on while its command waits for input and a client that can
   answer follows, up to that limit from its start, asked once per quiet period. A
   flagged call outside the sandbox gets the same limit whoever approved it: the user,
   the user's rule, the reviewer or full mode. The limit holds only while a person who
   can answer follows, so a call that nobody follows still answers at its timeout. A
   contained call keeps only a plain visible wait (decision 5).

3. **A command left running keeps its identity and is watched.** When a call answers
   while its command goes on, the hidden shell keeps the call id, the first 200
   characters of the line, the start, whether it runs in the sandbox, the flag, who
   approved the call and its last wait. A task of efr-shell looks at it once per quiet
   period, as the run did, and reports each change of its wait with the row of the
   screen that asks. efrd records three events of the conversation for it
   (`shell_command_detached`, `shell_command_input_changed`,
   `shell_command_ended`), and the call's `tool_call_completed` gets `left_running`.
   `input.respond` with the call's id answers it, with the same checks of kind,
   process group and terminal modes as for a running call. `shell.interrupt` (scope
   `terminal`) sends SIGINT only to a job that holds the terminal and is not the shell
   itself, or closes the hidden shell. `shell.status` reads the live state, so an event
   of an efrd that stopped since never shows as busy. The next call fails at once while
   the command waits for input, and the model reads which command holds the shell.

4. **A kept wait needs a person's approval.** A wait for a password of a command left
   running outside the sandbox stays while a client that can answer follows. While
   nobody can answer, it stays only when the line has the hard or the soft flag and
   the user or the user's own rule approved it. Every other such wait stops at once,
   also in full mode, which is no person's approval of the line. So a model line that
   fakes a `sudo` prompt in an exit that the reviewer allowed never waits for the user
   at a later prompt. Any wait of a command left running ends at the input limit from
   its start: SIGINT, then a close of the hidden shell after a short grace.

5. **No answer that may be a secret reaches a contained command.** A prompt in the
   sandbox can be a fake. efr-shell refuses, for a contained run or a contained command
   left running, a hidden answer, an answer to a visible wait that looks like a
   password prompt, and every manual answer. Such a wait stops the command at the look
   that sees it. Clients never ask for such an answer, and a question about a
   contained call never names a password. This also closes the gap of today.

6. **The user types in one place.** Texts name `,shell`, and the notice says `type the
   password only after ,shell; never at this prompt`. The answer line of the view
   takes the keys at once, no steer goes while it shows, and its text never goes back
   to the command line. `,shell` works in sticky mode too, because a typed `efr shell`
   there goes to the model.

7. **A confined helper in each contained call reaches the job ports.** A contained
   foreground call in `auto` with a network namespace of its own, in a conversation
   whose running contained jobs forward ports while `jobs.forward_ports` is on, gets
   the helper `efr-sbx reach`. The inner stage starts it as it starts the forwarder of
   ADR 0016: a starter and a server, before the call's own Landlock domain, and the
   child starts only after the starter ended. The server makes its own session, names
   itself `efr-reach`, turns off dumps, and applies its own Landlock domain (no file
   right, `RESOLVE_UNIX` only on the reach socket, the TCP connect right handled with
   no rule, the signal and abstract socket scopes) and its own seccomp profile (only
   `AF_UNIX`, `AF_INET` and `AF_INET6` sockets; `execve`, `execveat` and `pidfd_getfd`
   refused). It listens on `127.0.0.1` and `::1` of the call's namespace for each
   forwarded port of the start before its ready byte, so a port of the start is the
   job's in the call. It keeps `bind` and `listen`, because a forward can come during
   the call and Landlock rules cannot grow after `restrict_self`; a bind in the call's
   own namespace reaches only the call, which can bind the same ports itself. A helper
   that does not start fails nothing: the call runs with no reach.

8. **efrd routes each connection again.** efrd binds `$CALL/reach.sock` (0600) before
   the launch. The helper's first connection sends `Hello`, and efrd sends `Reach` and
   `Unreach` for the conversation's ports as they change; the helper answers `Bound`,
   which efrd only logs. Each data connection begins with `Connect` and a port. efrd
   finds the job by the call's conversation and the port on every connection, takes
   one place of the job's 64, and opens the connection inside the job through the
   job's forwarder. efrd never opens a TCP connection on the host for a call. The new
   frames keep the 28 bytes and the hand decoder of ADR 0016, and each connection takes
   only its own kinds. A call has at most 32 connections at one time.

9. **The engine knows the reach.** The fact `CallFacts::reach_ports` holds the
   forwarded ports of the conversation's running jobs. A URL on `localhost`,
   `127.0.0.1` or `[::1]` with such a port is no network use, and such a `needs.hosts`
   entry makes no exit. When a line names a loopback port of a job that began to
   forward less than 3 s ago, efrd waits for that forward up to the rest of the 3 s on
   its clock. The fact only predicts; the sandbox enforces.

Rejected:

- Adding `rsync`, `scp` and `git` to the hard list: decision 1.
- Keeping every unattended password wait outside the sandbox: a reviewer-allowed line
  could then fake a prompt and wait for the user at a later prompt (decision 4).
- A static reach helper with one Landlock bind rule per port of the start and no bind
  later: tighter, but it misses a forward that comes after the call started, which is
  the common case when the model checks a server right after it starts the job.
- The proxy route: a `host` exit of `localhost` with the whole network, as before. It
  asks, it opens far more than one port, and the reviewer allows it only when a
  message names the host.
- Joining the job's network namespace: the call would share the job's grants and
  listens, and the call's own servers would collide with the job's.
- A helper outside the sandbox that joins the call's namespaces: it would run with
  every capability of the call's user namespace until it confines itself. The pattern
  of ADR 0012 and ADR 0016 (start inside, confine, then serve) is proven and smaller.
- The bridge of ADR 0012 for the reach: the bridge serves the proxy socket and its
  grants; one helper with two jobs would mix two trust paths.

## Consequences

- An answer can reach a command after its call ended. The checks of an answer hold,
  only a client with scope `terminal` answers (never the phone), and an unattended
  password wait stays only for a flagged line that a person approved.
- An exit can run up to 60 minutes while a person follows, also after the reviewer or
  full mode allowed it. The turn waits; the user can stop it.
- A process that a stuck command moved to its own session (`setsid`) survives the
  close of the hidden shell at the limit, as with every close of a hidden shell.
- A call can use a job's grants through a server of the job that relays. A call and a
  job of one conversation share `$SCRATCH` already, so this is no new flow, and the
  job's proxy budget still counts each byte that leaves. `docs/sandbox.md` names it as
  a known limit.
- A port of the start is in use in the call, and a port that comes during the call
  stays the call's when the call listens on it first.
- Each contained call of a conversation with a forwarding job costs one more process
  (two spawns, in parallel with the bridge and the overlay mounts), one socket and one
  control task in efrd. In the cost test of `efr-sbx`, the helper adds about 2.5 ms at
  the median to a call's start on the desktop and about 5 ms in the VM (debug builds).
  A call of any other conversation costs nothing more.
- efrd parses four more 28-byte frame kinds from the sandbox, with the same hand
  decoder and property tests.

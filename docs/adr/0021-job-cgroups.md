# 0021: A cgroup for each background job, below efrd's delegated cgroup

Status: accepted, 2026-10-24.

## Context

ADR 0010 holds the processes of a background job by its session: the launcher calls
`setsid`, and every stop ends with SIGKILL to each process of that session (decision
6). A process of an exit child job that calls `setsid` itself leaves the session. When
its launcher is gone too, it escapes the stop, the time limit and the sweep, and the
end of the job does not name it. ADR 0010 named the fix, a cgroup per job, and left it
for later because it changes the systemd unit (`Delegate=yes`).

A job also uses memory and CPU time that nobody can see: `efr jobs list` and the
model's `jobs` tool showed only the size of the output.

cgroup v2 has what the fix needs:

- `cgroup.kill` (Linux 5.14) kills every process of a cgroup and of its children in
  one write, also a process that left its session or its parent's tree.
- `cgroup.events` says `populated 0` when no process is left.
- `memory.current`, `memory.peak` (Linux 5.19) and `cpu.stat` give the memory and the
  CPU time of every process of the cgroup.
- A process cannot leave its cgroup unless it can write the `cgroup.procs` file of a
  common ancestor of the two cgroups.

A process may make cgroups only in a subtree that it owns. systemd gives a unit its
subtree with `Delegate=yes`. systemd's docs (<https://systemd.io/CGROUP_DELEGATION/>,
"Delegation") say: "The service manager sets the `user.delegate` extended attribute
(readable via `getxattr(2)` and related calls) to the character `1` on cgroup
directories where delegation is enabled (and removes it on those cgroups where it is
not). This may be used by service programs to determine whether a cgroup tree was
delegated to them." It names kernel 5.6 and systemd 251 as the least versions. The docs
do not say whether the per-user service manager sets the mark. On the user's machine
(systemd 262, the user manager), `systemctl --user show efrd -p Delegate` says
`Delegate=yes`, and the unit's cgroup is the user's, but it has no extended attribute
at all; only `user@1000.service`, which the system manager made, has `user.delegate`.
So the mark is a sure yes when it is there, and its absence says nothing under the
user manager. The manager's own answer is the `Delegate` property of the unit, which
`systemctl show` reads from the manager. cgroup v2 also has the rule of no internal
processes: a cgroup that gives a controller, such as `memory`, to its children must
hold no process itself.

The project adds no dependency (the user's rule for v0.0.5), so efrd reads and writes
the cgroup v2 files itself.

## Decision

1. **`Delegate=yes` in efrd's unit.** `systemd/efrd.service`, which `just install` and
   both AUR packages install, gets `Delegate=yes`. The PKGBUILDs check the line. efrd
   never changes a unit that is installed already; the user installs the new one.

2. **efrd moves itself into `<unit>/daemon` at its start.** Before it starts any
   process of its own, efrd reads its cgroup from `/proc/self/cgroup` and finds out
   whether it is delegated. A `user.delegate` or `trusted.delegate` mark of `1` on the
   cgroup is a yes at once. Without a mark, efrd asks the user manager once:
   `systemctl --user show -p LoadState -p MainPID -p Delegate -- <unit>`, where the unit
   is the last part of the cgroup path and must be a `.service` name. `systemctl` must
   be a trusted program (`trusted_programs.rs`: on efrd's own `PATH`, in a dir that only
   root writes); it gets a clean environment with `XDG_RUNTIME_DIR` and 2 s. Only
   `LoadState=loaded`, `MainPID` equal to efrd's pid and `Delegate=yes` is a yes. No
   trusted `systemctl`, an error, no answer in time, an unknown unit, another main
   process or `Delegate=no` is a no, and each says why. `systemctl` ends before the
   move, so the unit's cgroup holds only efrd then. Then it makes `<unit>/daemon` and `<unit>/jobs`, moves itself into
   `<unit>/daemon`, and gives `<unit>` and `<unit>/jobs` the `memory`, `cpu` and `pids`
   controllers that `<unit>` has. The hidden shells start later, so they are in
   `<unit>/daemon` with efrd. A controller that the unit lacks costs only its numbers.

3. **One cgroup per job, joined before the start byte.** efrd makes
   `<unit>/jobs/<conversation>-j<n>` and writes the launcher's pid into it after the
   spawn and before the start byte. The launcher starts nothing before that byte
   (ADR 0010, decision 5), so every process of the job is born in the job's cgroup. The
   launcher (`efr-sbx`) needs no change. A call that the user moves with Ctrl+B
   (ADR 0018) moves into a cgroup of its own at the move: first its old hidden shell,
   then every process below it.

4. **The end kills the cgroup.** After the stop of the launcher and the sweep of the
   session, efrd writes `1` to `cgroup.kill`, waits up to 1 s for `populated 0`, reads
   the most memory that the job used and its CPU time, and removes the dir. A kernel
   with no `cgroup.kill` gets SIGKILL to each pid of `cgroup.procs` through a pidfd.
   The names of processes that are still there go in the end's survivors, as for the
   session. The session sweep stays: it is the only sweep when the job has no cgroup.

5. **The numbers are on the wire.** `JobInfo::cgroup` (`JobCgroup`) holds the memory
   and the CPU time: now while the job runs (`jobs.list` reads the files of each
   running job), the most memory at its end. `efr jobs list` and the model's `jobs`
   tool show them for a running job.

6. **A fallback that says so.** When efrd does not run as its unit (started by hand, a
   test daemon), when the unit's cgroup is not delegated (an older unit), with no
   cgroup v2, or when efrd cannot make or join a job's cgroup, the job runs as before:
   held by its session only. `JobCgroup::off` says why, and `efr jobs list` shows it.
   Only `Deps::from_process` turns the cgroups on, and only when efrd's cgroup is its
   unit `efrd.service`, so a test daemon never moves itself, and neither does an efrd
   that runs in a terminal's scope, also a delegated one.

7. **The peer check covers the subtree.** A process in efrd's unit cgroup that is not
   efrd counts as model-side on the socket (DV49). With decision 2, efrd and its shells
   are in `<unit>/daemon` and each job in `<unit>/jobs/...`, so the rule counts every
   cgroup below the unit's too. It still only reduces a peer.

8. **At start, the cgroups of the last efrd go.** Before any job runs, efrd kills and
   removes every dir that is left in `<unit>/jobs`. systemd kills the whole subtree when
   the unit stops, so this only finds what a crash left.

## Consequences

- A job's stop and its time limit end every process of the job, also one that called
  `setsid` and lost its launcher, unless the process moved itself out on purpose (see
  below).
- `efr jobs list` and the model see the memory and the CPU time of each running job.
- Because of the controllers, the CPU time of `<unit>/daemon` and of `<unit>/jobs`
  is shared by weight: a busy job cannot take all the CPU from efrd and its shells.
- A job in the exit child runs as the user, who owns the delegated subtree. It can
  write its pid into another cgroup of the subtree on purpose and leave its job's
  cgroup. The session sweep and the peer check still hold it there. A contained job
  runs in a cgroup namespace of its own (`--unshare-cgroup`) and sees no cgroup file of
  the host.
- The user must install the new unit and run `systemctl --user daemon-reload` for the
  fix. Until then, each job says `efrd's cgroup is not delegated (Delegate=yes)`.
- efrd's own cgroup is now `<unit>/daemon`. A tool that reads the unit's processes from
  systemd still finds efrd, below the unit.

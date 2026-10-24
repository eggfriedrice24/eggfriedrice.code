# 0020: efrd gives freed heap back with glibc's allocator controls

Status: accepted, 2026-10-24.

## Context

efrd is a long-lived process. A load of many turns allocates and frees much memory,
and glibc keeps the freed heap in its arenas. The memory bench
(`bench_perf_daemon_memory`: 20 conversations of 50 turns at the same time, each turn
with a tool output of 32 KiB, then 3 hours of idle time on the clock) measured this on
`fd75d275` with glibc 2.44, 32 CPUs and efrd's 4 tokio workers. Values are MiB, the
range of 6 runs:

| | glibc defaults |
|---|---|
| live heap after the load (`mallinfo2`: in use plus mapped) | 46 to 49 |
| free heap in the arenas after the load | 214 to 262 |
| resident set after the load (the peak) | 294 to 344 |
| resident set 3 s after the actors' idle trim | 283 to 341 |
| resident set 30 s after the idle trim | 180 to 214 |
| live heap after the idle trim | 8 to 11 |

So after the idle trim efrd kept about 190 MiB for about 10 MiB of live data. glibc
gives memory back only inside `free`, never on a timer. Two of its behaviours keep the
memory:

- The dynamic threshold. Each `free` of a large mapped block raises the mmap threshold
  to the size of that block, and the trim threshold to two times that value, up to
  32 MiB and 64 MiB. After a load with large tool outputs, a free top of an arena
  must be very large before `free` gives it back.
- The arenas. glibc makes up to 8 arenas per CPU (256 here). The workers and the
  blocking threads spread the churn over many arenas, and `free` trims only the top
  of an arena, never a free block below a live one.

`malloc_trim(0)` walks every arena and gives back every free page, also those below
live blocks. It took 7 to 9 ms after the load.

`unsafe_code` is denied in the whole workspace. `mallopt` and `malloc_trim` have no
wrapper in rustix or nix; the only way to call them is FFI through `libc`, which needs
an exception in `cargo xtask tidy`, as ADR 0007 did for efr-sbx.

## Decision

efrd uses two glibc calls, in one module, `crates/efr-daemon/src/malloc.rs`. `lib.rs`
opts it out with `#[allow(unsafe_code)]` on its module item, and `xtask/src/tidy.rs`
lists the file. Both calls are compiled only for Linux with glibc
(`cfg(all(target_os = "linux", target_env = "gnu"))`); elsewhere they do nothing.

| Function | Call | Why it is sound |
|---|---|---|
| `init()` | `mallopt(M_TRIM_THRESHOLD, 128 * 1024)` | integers only; it writes two words of glibc's parameters under the main arena's lock. glibc's own `free` writes the same kind of words on any thread without a lock, and `main` calls it before any other thread starts |
| `trim()` | `malloc_trim(0)` | an integer only; it takes the lock of each arena that it trims and gives back only pages with no live block |

1. `main.rs` calls `init` first, before the runtime starts a thread. It sets the trim
   threshold to a fixed 128 KiB, the default value, so a `free` that leaves a free top
   of 128 KiB gives it back. In glibc, `mallopt(M_TRIM_THRESHOLD)` and the tunable
   `glibc.malloc.trim_threshold` both call the same function, `do_set_trim_threshold`
   in `malloc/malloc.c`, which sets `mp_.trim_threshold` and `mp_.no_dyn_threshold = 1`
   (`malloc/arena.c` registers the tunable with `TUNABLE_CALLBACK (set_trim_threshold)`,
   and `__libc_mallopt` calls it for `M_TRIM_THRESHOLD`). So the call also stops the
   dynamic threshold, as the tunable that the measurement used does. The glibc manual
   says the same of the tunable: "If the tunable is set, the dynamic adjustment is
   disabled". One difference: the tunable applies before the first allocation, and
   the call after the few allocations of the Rust runtime before `main`.
2. One task (`crates/efr-daemon/src/heap_trim.rs`, safe Rust) calls `trim` once on a
   blocking thread after the conversation actors trim. An actor that is idle for 10
   minutes drops its turns and adds one to `ConversationDeps::idle_trims`, a `watch`
   counter. efr-conversation knows nothing of the allocator. The task trims when 30 s
   passed on the injected clock with no turn running, no turn end and no other actor
   trim, and at most once in 5 minutes. It follows the turns from the committed
   batches of the store's writer (`turn_started` and the four turn ends).

What it saves (the same bench; the measurement used the tunable for the threshold):

| | defaults | threshold only | threshold and trim |
|---|---|---|---|
| peak | 294 to 344 | 211 to 247 | 211 to 247 |
| 30 s after the idle trim | 180 to 214 | 122 to 151 | - |
| after `malloc_trim(0)` | - | - | 49 to 53 |
| load time (minimum) | 1.00 | 1.07 | 1.07 |

On the branch of this change (`perf-wave3`, after the other memory fixes of wave 3), the
bench with the calls themselves gives a peak of 168 to 177 MiB, 152 to 160 MiB 3 s after
the idle trim, and 58 to 66 MiB after the heap trim (34% to 37% of the peak). Its
budget: below 50% of the peak.

## Alternatives

All on the same bench, MiB.

- **`malloc_trim(0)` only, the default threshold.** The idle resident set falls from
  about 190 to 105 to 122, and the peak stays at about 300 to 324. The fixed threshold
  adds no measurable cost and halves the rest, so the decision takes both.
- **`mallopt(M_ARENA_MAX, 2)` and the trim.** The peak falls to 149 to 160 and the idle
  resident set after the trim to 49 to 56. But the threads then share arenas and wait
  on their locks: the CPU-bound load took 1.35 times as long, and the growth bench
  1.17 times. The user chose the threshold, which costs nothing in a turn.
- **`GLIBC_TUNABLES` in `systemd/efrd.service`.** No new exception, but efrd copies its
  own environment into the hidden shells (`run.rs`), so every program of the agent
  (cargo, rustc, tests) would get the setting unless efrd removed it. It works only
  when systemd starts efrd, and without a trim it saves only about 40 to 55 MiB at
  idle (arena_max=2: about 149; trim_threshold: about 135).
- **jemalloc** (`tikv-jemallocator`). With its defaults it starts 37 MiB higher and
  keeps 260 MiB at +30 s. Tuned (4 arenas, 1 s decay, a background thread) it keeps
  70 MiB, with a TTFT of 10 to 15% more. It is a new dependency with a C build in CI
  and AUR, and it puts efrd 0.39 MB over its size budget (+0.89 MB).
- **mimalloc.** With its defaults it keeps about 400 MiB (v3) or 190 MiB (v2). With
  `MIMALLOC_PURGE_DELAY=0` it keeps about 100 MiB, with a TTFT of about 30% more. It is
  a new dependency too.

## Consequences

- efrd's resident set after an idle period is about one third of its peak, not near
  the peak. A turn does not get slower: the trim runs only when no turn runs. The
  first turn after a trim may fault in some pages again.
- The rest of efr-daemon stays safe Rust; a reviewer reads one file of about 60 lines.
  A new allocator call goes into this module, with its row in the table above and a
  `// SAFETY:` comment, or it does not go in.
- `libc` is a dependency of efr-daemon for this module alone; the README says so. It
  was already in the workspace and the lock (efr-pty, efr-sbx), so `deny.toml` stays
  the same. The release efrd grows by about 13 KB.
- A test that runs the daemon in-process (`TestDaemon`) runs the heap trim task with
  the real `malloc_trim`, but not `main`; a bench that wants the threshold calls
  `efr_daemon::init_malloc` itself.
- On a target without glibc, such as musl, efrd keeps its freed heap as before.

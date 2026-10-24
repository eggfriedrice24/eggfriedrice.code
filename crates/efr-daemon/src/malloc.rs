//! glibc's allocator controls, so that efrd gives freed heap back to the system
//! (`docs/adr/0020-malloc-trim.md`).
//!
//! glibc keeps the heap that efrd frees in its arenas. After a load of 20
//! conversations of 50 turns, efrd held about 300 MiB, of which only about 50 MiB was
//! live; after the idle trim of the actors, about 10 MiB was live and the resident set
//! was still about 190 MiB. Two calls give that memory back: [`init`] at the start of
//! `main` makes glibc's trim threshold fixed, and [`trim`] gives the free pages of every
//! arena back once the actors dropped their turns (`heap_trim.rs` decides when).
//!
//! This is the one module of efr-daemon that may hold `unsafe` code: rustix and nix
//! have no wrapper for `mallopt` or `malloc_trim`, so both are FFI calls through
//! `libc`. Each takes integers only and passes no memory. On a target without glibc,
//! both functions do nothing.

/// The trim threshold that [`init`] sets, in bytes: glibc's default of 128 KiB, made
/// fixed. A `free` that leaves a free top of an arena of this size or more gives it
/// back to the system.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const TRIM_THRESHOLD: libc::c_int = 128 * 1024;

/// Sets glibc's trim threshold to a fixed 128 KiB. A value set with `mallopt` also
/// stops glibc's dynamic threshold: without it, each `free` of a large mapped block
/// raises the mmap threshold, and the trim threshold to two times that value, up to
/// 64 MiB, so efrd kept most of the freed heap after a load. This is the parameter that
/// `GLIBC_TUNABLES=glibc.malloc.trim_threshold=131072` sets; the call does not reach
/// the hidden shells, which copy efrd's environment.
///
/// `main` calls it first, before the runtime starts a thread, so every allocation of
/// the daemon has the setting. A test that runs the daemon in-process calls it itself.
pub fn init() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: `mallopt` takes two integers and passes no memory. With
        // `M_TRIM_THRESHOLD` it writes two words of glibc's parameters
        // (`mp_.trim_threshold` and `mp_.no_dyn_threshold`) under the lock of the main
        // arena. Other threads read these words without that lock, but glibc's own
        // `free` writes `mp_.trim_threshold` and `mp_.mmap_threshold` on any thread
        // without a lock (the dynamic threshold), so a call while other threads
        // allocate does nothing that glibc does not do itself; `main` calls it before
        // any other thread starts all the same. `M_TRIM_THRESHOLD` is a parameter that glibc knows, so the call cannot
        // fail, and its result (1) says nothing more.
        unsafe {
            libc::mallopt(libc::M_TRIM_THRESHOLD, TRIM_THRESHOLD);
        }
    }
}

/// Gives the free pages of every arena back to the system. True when glibc gave
/// memory back. The call holds the lock of each arena while it walks its free chunks:
/// about 8 ms in all after a large load. Call it on a blocking thread.
pub(crate) fn trim() -> bool {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: `malloc_trim` takes an integer (the free room to keep at the top of
        // the main heap; 0 keeps none), passes no memory and takes the lock of each
        // arena that it trims, so it is sound from any thread while others allocate. It
        // gives back only pages that hold no live allocation.
        unsafe { libc::malloc_trim(0) == 1 }
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        false
    }
}

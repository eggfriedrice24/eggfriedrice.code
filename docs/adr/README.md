<!-- sources: the title and the Status line of each docs/adr/*.md -->

# Decision records

A decision record tells why the project made a choice that is expensive to reverse.
Each record starts with a `Status:` line and a date. Most records then have the
sections Context, Decision and Consequences.

- [0001: One crate per bounded context, checked by an allowlist](0001-crate-boundaries.md). Status: accepted, 2026-10-03.
- [0002: Shell marks come from a scanner in efr-screen](0002-shell-marks-in-efr-screen.md). Status: accepted, 2026-10-03.
- [0003: Every screen runs on its own std thread](0003-screen-thread-per-actor.md). Status: accepted, 2026-10-03.
- [0004: Pin libghostty-vt by git rev, not crates.io 0.2.2](0004-ghostty-pin.md). Status: accepted, 2026-10-03.
- [0005: Syntax highlighting engine for efr-render](0005-syntax-highlighting-engine.md). Status: open, 2026-10-04.
- [0006: The auto mode runs commands in a kernel sandbox](0006-auto-sandbox.md). Status: accepted, 2026-10-06.
- [0007: An unsafe exception for efr-sbx's fds module](0007-efr-sbx-unsafe.md). Status: accepted, 2026-10-06.
- [0008: The snapshot keys live in `[snapshot]`, not `[undo]`](0008-snapshot-config-keys.md). Status: accepted, 2026-10-08.
- [0009: Undo plans in `efr-snapshot`, writes in efrd, and a home store](0009-undo-and-home-store.md). Status: accepted, 2026-10-11.
- [0010: Background jobs start from efrd, outlive their turn and never speak for the user](0010-background-jobs.md). Status: accepted, 2026-10-12.
- [0011: Read-only views of the host's network and mounts, with the kernel as the only guard](0011-netview-and-mount-view.md). Status: accepted, 2026-10-13.
- [0012: A network proxy in efrd, with a bridge in each call](0012-network-proxy.md). Status: accepted, 2026-10-19.
- [0013: A filtered bus for each call, and always-allow rules](0013-bus-proxy-and-always-allow.md). Status: accepted, 2026-10-19.
- [0014: Fold four small crates back](0014-fold-four-small-crates-back.md). Status: accepted, 2026-10-20.
- [0015: Full mode, a mode with no sandbox and no questions](0015-full-mode.md). Status: accepted, 2026-10-20.
- [0016: Port forwards for contained background jobs](0016-job-port-forwarding.md). Status: accepted, 2026-10-21.
- [0017: A busy hidden shell, and the reach of a contained call](0017-busy-shell-and-call-reach.md). Status: accepted, 2026-10-21.
- [0018: Moving a running call to the background](0018-moving-a-call-to-the-background.md). Status: accepted, 2026-10-21.
- [0019: Conversation search](0019-conversation-search.md). Status: accepted, 2026-10-22.
- [0020: efrd gives freed heap back with glibc's allocator controls](0020-malloc-trim.md). Status: accepted, 2026-10-24.

Record 0005 is still open. efr-render uses syntect for now, and the project can
change this choice later.

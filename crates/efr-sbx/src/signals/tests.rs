use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nix::sys::signal::{SigSet, Signal};
use signal_hook::consts::{SIGINT, SIGQUIT, SIGTSTP};

use super::{STOPS, catch_flags};

#[test]
fn a_stop_signal_while_its_handler_is_installed_sets_its_flag() {
    // The signal comes right before its handler is installed: in the window where a
    // handler that is not complete yet would lose it, or the default action would end
    // the process. Each comes on this thread, so the signal mask decides alone.
    let flags: [Arc<AtomicBool>; 3] = Default::default();
    let caught = [SIGINT, SIGQUIT, SIGTSTP].into_iter().zip(flags.iter().map(Arc::clone));
    catch_flags(caught, |signal| {
        if STOPS.contains(&signal) {
            signal_hook::low_level::raise(signal).unwrap();
        }
    })
    .unwrap();
    let [int, quit, tstp] = &flags;
    assert!(int.load(Ordering::SeqCst), "SIGINT was lost");
    assert!(quit.load(Ordering::SeqCst), "SIGQUIT was lost");
    assert!(!tstp.load(Ordering::SeqCst));
    // The mask is back: no child inherits a blocked signal.
    let mask = SigSet::thread_get_mask().unwrap();
    for signal in [Signal::SIGINT, Signal::SIGQUIT, Signal::SIGTSTP] {
        assert!(!mask.contains(signal), "{signal} stays blocked");
    }
}

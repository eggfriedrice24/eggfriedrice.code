//! The text of the shell tool for efr's bus filter (phase E, DE8 and DE11).

use efr_protocol::{BusKind, BusRefusal};
use pretty_assertions::assert_eq;

use super::super::bus_note;

fn refused(bus: BusKind, name: &str, member: &str, path: &str, count: u32) -> BusRefusal {
    BusRefusal {
        bus,
        name: name.to_owned(),
        member: member.to_owned(),
        path: path.to_owned(),
        count,
    }
}

#[test]
fn bus_note_names_the_refused_call() {
    let start = refused(
        BusKind::System,
        "org.freedesktop.systemd1",
        "org.freedesktop.systemd1.Manager.StartUnit",
        "/org/freedesktop/systemd1",
        1,
    );

    assert_eq!(
        bus_note(&[start], 0, false).as_deref(),
        Some(
            "[efr: the bus filter refused a call: bus=system name=org.freedesktop.systemd1 \
             member=org.freedesktop.systemd1.Manager.StartUnit path=/org/freedesktop/systemd1. \
             For the whole bus for one command, call shell again with needs.bus.]"
        )
    );
}

#[test]
fn bus_note_counts_each_call_and_the_rest() {
    let entries = [
        refused(BusKind::System, "org.x.Y", "org.x.Y.Do", "/a", 2),
        refused(BusKind::Session, "org.freedesktop.Notifications", "Notify", "/b", 1),
        refused(BusKind::System, "org.efrprobe.Activated", "", "", 1),
    ];

    let note = bus_note(&entries, 3, false).unwrap();

    assert_eq!(
        note,
        "[efr: the bus filter refused 7 calls: bus=system name=org.x.Y member=org.x.Y.Do \
         path=/a (2 times); bus=session name=org.freedesktop.Notifications member=Notify \
         path=/b; bus=system name=org.efrprobe.Activated; 3 more. For the whole bus for one \
         command, call shell again with needs.bus.]"
    );
    assert_eq!(
        bus_note(&[], 1, false).as_deref(),
        Some(
            "[efr: the bus filter refused a call. For the whole bus for one command, call \
             shell again with needs.bus.]"
        )
    );
}

#[test]
fn bus_note_says_the_filter_stopped() {
    let stopped = "[efr: the bus filter stopped during the command, so its bus calls after \
                   that failed.]";
    assert_eq!(bus_note(&[], 0, true).as_deref(), Some(stopped));
    let entry = refused(BusKind::System, "org.x.Y", "org.x.Y.Do", "/a", 1);
    let note = bus_note(&[entry], 0, true).unwrap();
    assert_eq!(note.lines().count(), 2, "{note}");
    assert_eq!(note.lines().last(), Some(stopped));
    assert_eq!(bus_note(&[], 0, false), None);
}

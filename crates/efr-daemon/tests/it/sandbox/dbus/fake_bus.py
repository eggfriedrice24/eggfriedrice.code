"""The fake services of efrd's real bus filter tests (efr's auto spec 11.1, phase E).

The test starts two private dbus-daemon buses and runs this file with `python3 -I`:

    fake_bus.py SYSTEM_ADDRESS SESSION_ADDRESS LOG HOME

It connects only to the two addresses that it gets, never to a bus of the machine. On
the system bus it owns org.freedesktop.hostname1, org.freedesktop.systemd1 and
org.freedesktop.login1; on the session bus org.freedesktop.systemd1 and
org.freedesktop.Notifications. It writes one JSON line to LOG for each method call that
reaches it, so a test can show that the filter let a call through or that a refused
call never came. It answers the read methods that systemd 255 and 262 send for the
tests' tools from a fixed table, and every other method with UnknownMethod, logged as
"unknown", so a failed test names a member that the table lacks. EnableUnitFiles makes
its link below HOME, as the real user manager would. The line {"ready": true} comes
after the last name request.
"""

import json
import os
import sys
import threading

from gi.repository import Gio, GLib

SYSTEM, SESSION, LOG, HOME = sys.argv[1:5]

PROPERTIES = "org.freedesktop.DBus.Properties"
MANAGER = "org.freedesktop.systemd1.Manager"
LOGIN = "org.freedesktop.login1.Manager"
NOTIFICATIONS = "org.freedesktop.Notifications"
UNIT_PATH = "/org/freedesktop/systemd1/unit/"
SESSION_PATH = "/org/freedesktop/login1/session/_31"
UID = os.getuid()

lock = threading.Lock()
log = open(LOG, "a", encoding="utf-8")


def write(entry):
    with lock:
        log.write(json.dumps(entry) + "\n")
        log.flush()


def strings(table):
    return {key: GLib.Variant("s", value) for key, value in table.items()}


HOSTNAME = strings({
    "Hostname": "efr-fake-host",
    "StaticHostname": "efr-fake-host",
    "PrettyHostname": "",
    "IconName": "computer-vm",
    "Chassis": "vm",
    "KernelName": "Linux",
    "KernelRelease": "7.1.0-fake",
    "OperatingSystemPrettyName": "efr fake OS",
})
SYSTEMD = strings({"Version": "262-fake"})
UNIT = strings({"ActiveState": "failed", "LoadState": "loaded", "SubState": "failed"})
SESSION_PROPERTIES = strings({"TTY": "tty1", "State": "active", "Seat": "seat0"})


def unit(name):
    """One row of ListUnits and ListUnitsByPatterns: a failed unit."""
    path = UNIT_PATH + name.replace("-", "_2d").replace(".", "_2e")
    return (name, "A fake failed unit", "loaded", "failed", "failed", "", path, 0, "", "/")


FAILED = {"system": "fake-failed.service", "session": "fake-user-failed.service"}


def properties(path, name):
    """The property table at `path` of the service `name`, or None."""
    if name == "org.freedesktop.hostname1" and path == "/org/freedesktop/hostname1":
        return HOSTNAME
    if name == "org.freedesktop.systemd1" and path == "/org/freedesktop/systemd1":
        return SYSTEMD
    if name == "org.freedesktop.systemd1" and path.startswith(UNIT_PATH):
        return UNIT
    if name == "org.freedesktop.login1" and path.startswith("/org/freedesktop/login1/"):
        return SESSION_PROPERTIES
    return None


def enable(args):
    """EnableUnitFiles: one link per unit below HOME, as systemd --user makes it."""
    wants = os.path.join(HOME, ".config/systemd/user/default.target.wants")
    os.makedirs(wants, exist_ok=True)
    changes = []
    for name in args[0]:
        link = os.path.join(wants, name)
        if not os.path.lexists(link):
            os.symlink("/dev/null", link)
        changes.append(("symlink", link, "/dev/null"))
    # The reply (b, a(sss)): no install hooks ran, and the links it made.
    return "(b" + "a(sss))", (False, changes)


def answer(bus, name, path, member, args):
    """The reply `(signature, value)` of a call, or None for UnknownMethod."""
    table = properties(path, name)
    if member == PROPERTIES + ".GetAll" and table is not None:
        return "(a{sv})", (table,)
    if member == PROPERTIES + ".Get" and table is not None and args[1] in table:
        return "(v)", (table[args[1]],)
    if name == "org.freedesktop.systemd1" and path == "/org/freedesktop/systemd1":
        rows = [unit(FAILED[bus])]
        if member in (MANAGER + ".ListUnitsByPatterns", MANAGER + ".ListUnits"):
            return "(a(ssssssouso))", (rows,)
        if member in (MANAGER + ".ListUnitFilesByPatterns", MANAGER + ".ListUnitFiles"):
            return "(a(ss))", ([("/etc/systemd/system/" + FAILED[bus], "enabled")],)
        if member == MANAGER + ".GetUnitFileState":
            return "(s)", ("enabled",)
        if member == MANAGER + ".GetUnit":
            return "(o)", (rows[0][6],)
        if member == MANAGER + ".ListJobs":
            return "(a(usssoo))", ([],)
        if member == MANAGER + ".StartUnit":
            return "(o)", ("/org/freedesktop/systemd1/job/1",)
        if member == MANAGER + ".EnableUnitFiles":
            return enable(args)
    if name == "org.freedesktop.login1" and path == "/org/freedesktop/login1":
        if member == LOGIN + ".ListSessionsEx":
            row = ("1", UID, "efr", "seat0", 1, "user", "tty1", False, 0, SESSION_PATH)
            return "(a(sussussbto))", ([row],)
        if member == LOGIN + ".ListSessions":
            return "(a(susso))", ([("1", UID, "efr", "seat0", SESSION_PATH)],)
        if member == LOGIN + ".ListUsers":
            return "(a(uso))", ([(UID, "efr", "/org/freedesktop/login1/user/_1000")],)
        if member == LOGIN + ".ListSeats":
            return "(a(so))", ([("seat0", "/org/freedesktop/login1/seat/seat0")],)
    if name == NOTIFICATIONS and path == "/org/freedesktop/Notifications":
        if member == NOTIFICATIONS + ".Notify":
            return "(u)", (7,)
        if member == NOTIFICATIONS + ".GetServerInformation":
            return "(ssss)", ("efr-fake", "efr", "1", "1.2")
    return None


def reply(conn, bus, msg):
    """Answers one method call on the main loop."""
    name = msg.get_destination() or ""
    path = msg.get_path() or ""
    member = f"{msg.get_interface() or ''}.{msg.get_member() or ''}"
    body = msg.get_body()
    args = body.unpack() if body is not None else ()
    try:
        found = answer(bus, name, path, member, args)
    except (IndexError, TypeError, OSError) as error:
        found = None
        write({"bus": bus, "error": str(error), "member": member})
    write({"bus": bus, "name": name, "path": path, "member": member,
           "answer": "ok" if found else "unknown"})
    if found is None:
        out = Gio.DBusMessage.new_method_error_literal(
            msg, "org.freedesktop.DBus.Error.UnknownMethod", f"the fake has no {member}")
    else:
        out = Gio.DBusMessage.new_method_reply(msg)
        out.set_body(GLib.Variant(found[0], found[1]))
    conn.send_message(out, Gio.DBusSendMessageFlags.NONE)
    return False


def connect(address, bus, names):
    flags = (Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT
             | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION)
    conn = Gio.DBusConnection.new_for_address_sync(address, flags, None, None)
    unique = conn.get_unique_name()

    def keep(_conn, msg, incoming):
        # Every call to this connection is answered by `reply`, never by GDBus.
        if incoming and msg.get_message_type() == Gio.DBusMessageType.METHOD_CALL \
                and msg.get_sender() != "org.freedesktop.DBus":
            GLib.idle_add(reply, conn, bus, msg)
            return None
        return msg

    conn.add_filter(keep)
    for name in names:
        result = conn.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus",
                                "org.freedesktop.DBus", "RequestName",
                                GLib.Variant("(su)", (name, 4)), None,
                                Gio.DBusCallFlags.NONE, -1, None)
        if result.unpack()[0] != 1:
            raise SystemExit(f"{bus}: could not own {name}")
    return conn, unique


connections = [
    connect(SYSTEM, "system", ["org.freedesktop.hostname1", "org.freedesktop.systemd1",
                               "org.freedesktop.login1"]),
    connect(SESSION, "session", ["org.freedesktop.systemd1", NOTIFICATIONS]),
]
write({"ready": True})
GLib.MainLoop().run()

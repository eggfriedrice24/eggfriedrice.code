import { describe, expect, it } from "vitest";

import { menuActions } from "@/components/ui/menu-actions";
import { sheetOptions } from "@/components/ui/action-sheet";
import { machineOf } from "@/features/machines/model/machine";
import type {
  MachineConnection,
  MachineStatus,
} from "@/features/machines/model/machine-connection";
import { DESKTOP, LAPTOP, machineRecord } from "@/features/machines/model/testing";
import type { PairedDevice } from "@/features/pairing/model/paired-device";
import {
  MACHINE_MENU,
  forgetFailedText,
  forgetSheet,
  machineAction,
  machineRowText,
  nodeName,
  versionText,
  zoneAt,
} from "./settings-format";

const ENDPOINT = "100.64.0.1:7419";
/** 2026-10-26 13:58:00 UTC. */
const SEEN = Date.UTC(2026, 9, 26, 13, 58);
const TWO_DAYS = 2 * 86_400_000;

const desktop = machineOf(machineRecord(DESKTOP), false);
/** The laptop, paired with a code that had no MagicDNS name. */
const laptop = machineOf(withoutFqdn(machineRecord(LAPTOP)), false);

function withoutFqdn(device: PairedDevice): PairedDevice {
  const copy = { ...device };
  delete copy.fqdn;
  return copy;
}

const STATUS: MachineStatus = {
  version: "0.0.5",
  protocol: 1,
  scopes: ["read", "operate", "approve"],
  seenAt: SEEN,
};

function connection(view: MachineConnection["view"], seenAt?: number): MachineConnection {
  return { client: { state: "off" }, view, seenAt };
}

describe("machineRowText", () => {
  it("draws a connected machine as the Settings board does", () => {
    const text = machineRowText(
      desktop,
      connection({ kind: "online", endpoint: ENDPOINT }, SEEN),
      STATUS,
      SEEN,
      0,
    );
    expect(text).toEqual({
      name: "desktop",
      dim: false,
      state: "✓ connected",
      stateTone: "state_success",
      node: "desktop.tail3f2a.ts.net",
      version: "efrd 0.0.5 · protocol 1",
      actionsLabel: "desktop actions",
    });
  });

  it("draws a connecting machine with its node and a quiet state", () => {
    const text = machineRowText(
      desktop,
      connection({ kind: "connecting", endpoint: ENDPOINT }),
      STATUS,
      SEEN,
      0,
    );
    expect(text.state).toBe("… connecting");
    expect(text.stateTone).toBe("text_muted");
    expect(text.node).toBe("desktop.tail3f2a.ts.net");
    expect(text.dim).toBe(false);
  });

  it("draws an offline machine dim, with when it was seen and no node", () => {
    const text = machineRowText(
      desktop,
      connection({ kind: "unreachable", endpoint: ENDPOINT }, SEEN),
      STATUS,
      SEEN + TWO_DAYS,
      0,
    );
    expect(text.state).toBe("offline · seen 2 days ago");
    expect(text.stateTone).toBe("text_subtle");
    expect(text.node).toBeUndefined();
    expect(text.dim).toBe(true);
    expect(text.version).toBe("efrd 0.0.5 · protocol 1");
  });

  it("takes the time it was seen from the kept status after a new start", () => {
    // A new run of the app: the connection has no time yet, the kept status has one.
    const text = machineRowText(
      desktop,
      connection({ kind: "retrying", endpoint: ENDPOINT }),
      STATUS,
      SEEN + 60_000,
      120,
    );
    expect(text.state).toBe("offline · seen 15:58");
  });

  it("says offline alone when the phone never saw the machine", () => {
    const text = machineRowText(
      desktop,
      connection({ kind: "unreachable", endpoint: ENDPOINT }),
      undefined,
      SEEN,
      0,
    );
    expect(text.state).toBe("offline");
    expect(text.version).toBeUndefined();
  });

  it("marks a machine that needs the user with the error glyph", () => {
    const repair = machineRowText(
      desktop,
      connection({ kind: "repair_needed", reason: "unknown_device" }),
      STATUS,
      SEEN,
      0,
    );
    expect([repair.state, repair.stateTone]).toEqual(["✗ re-pair needed", "state_error"]);
    const update = machineRowText(desktop, connection({ kind: "update_needed" }), STATUS, SEEN, 0);
    expect([update.state, update.stateTone]).toEqual(["✗ update needed", "state_error"]);
    const starting = machineRowText(
      desktop,
      connection({ kind: "starting", endpoint: ENDPOINT }),
      STATUS,
      SEEN,
      0,
    );
    expect([starting.state, starting.stateTone]).toEqual(["… efrd is starting", "text_muted"]);
  });

  it("says offline, not Tailscale, for an unreachable machine", () => {
    const text = machineRowText(
      laptop,
      connection({ kind: "unreachable", endpoint: ENDPOINT }, SEEN),
      undefined,
      SEEN,
      0,
    );
    expect(text.state).toBe("offline · seen 13:58");
  });
});

describe("nodeName", () => {
  it("gives the MagicDNS name, or the tailnet address without one", () => {
    expect(nodeName(desktop)).toBe("desktop.tail3f2a.ts.net");
    expect(nodeName(laptop)).toBe("100.64.0.2");
  });
});

describe("the row menu", () => {
  it("has Copy node name, Re-pair and a destructive Forget in its own section", () => {
    expect(menuActions(MACHINE_MENU)).toEqual([
      { id: "copy", title: "Copy node name", image: "doc.on.doc" },
      { id: "repair", title: "Re-pair", image: "arrow.triangle.2.circlepath" },
      {
        id: "forget",
        title: "",
        displayInline: true,
        subactions: [
          {
            id: "forget",
            title: "Forget this machine",
            image: "trash",
            attributes: { destructive: true },
          },
        ],
      },
    ]);
  });

  it("knows only its own ids", () => {
    expect(["copy", "repair", "forget", "pair", ""].map(machineAction)).toEqual([
      "copy",
      "repair",
      "forget",
      undefined,
      undefined,
    ]);
  });
});

describe("forgetSheet", () => {
  it("names the machine and says what stays on the machine", () => {
    expect(sheetOptions(forgetSheet("desktop", false))).toEqual({
      title: "Forget desktop?",
      message:
        "This iPhone deletes its pairing key for desktop and stops showing its conversations. Nothing changes on desktop. To use it again, pair with efr device add.",
      options: ["Forget desktop", "Cancel"],
      cancelButtonIndex: 1,
      destructiveButtonIndex: [0],
      userInterfaceStyle: "dark",
    });
  });

  it("says that the app goes back to pairing after the last machine", () => {
    expect(forgetSheet("desktop", true).message).toBe(
      "desktop is the last machine on this iPhone. Its pairing key is deleted and the app goes back to pairing. Nothing changes on desktop.",
    );
  });

  it("tells what to do when the key stays", () => {
    expect(forgetFailedText("desktop")).toEqual({
      title: "Could not forget desktop",
      message:
        "The pairing key of desktop is still on this iPhone. Forget desktop again after the next start of the app.",
    });
  });
});

describe("versionText", () => {
  it("gives the version and the build number", () => {
    expect(versionText("0.1.0", "14")).toBe("0.1.0 (14)");
    expect(versionText("0.1.0", undefined)).toBe("0.1.0");
    expect(versionText("0.1.0", "")).toBe("0.1.0");
    expect(versionText(undefined, undefined)).toBe("unknown");
  });
});

describe("zoneAt", () => {
  it("gives minutes east of UTC, the sign that seenText uses", () => {
    expect(zoneAt(SEEN)).toBe(-new Date(SEEN).getTimezoneOffset());
  });
});

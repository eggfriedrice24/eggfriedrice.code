import { describe, expect, it } from "vitest";

import { PROTOCOL_VERSION } from "@/lib/protocol/generated";
import type { PairingFailure } from "./pair";
import {
  ERROR_ACTION_LABEL,
  daemonLine,
  errorView,
  pairingProblem,
  pairingTitle,
  plainText,
  successWords,
  type PairingProblem,
} from "./pairing-view";

const CONTEXT = { machine: "desktop", repairing: undefined };

function title(problem: PairingProblem): string {
  return plainText(errorView(problem).title);
}

function reason(problem: PairingProblem): string {
  return plainText(errorView(problem).reason);
}

describe("pairingProblem", () => {
  it("gives null for a cancel, so the screen goes back to the intro", () => {
    expect(pairingProblem({ kind: "cancelled" }, CONTEXT)).toBeNull();
  });

  it.each<[PairingFailure, PairingProblem]>([
    [{ kind: "not_efr" }, { kind: "bad_code" }],
    [{ kind: "malformed" }, { kind: "bad_code" }],
    [{ kind: "code_version" }, { kind: "code_new" }],
    [{ kind: "unreachable" }, { kind: "tailnet", machine: "desktop" }],
    [{ kind: "wrong_daemon" }, { kind: "wrong_daemon", machine: "desktop" }],
    [{ kind: "token_refused", message: "the pairing token is wrong" }, { kind: "expired" }],
    [
      { kind: "refused", message: "the terminal refused the device" },
      { kind: "refused", machine: "desktop" },
    ],
    [
      { kind: "refused", message: "the pairing expired before it was finished" },
      { kind: "expired" },
    ],
    [
      { kind: "refused", message: "the pairing closed after too many wrong tokens" },
      { kind: "expired" },
    ],
    [
      { kind: "key_failed", message: "no key" },
      { kind: "key", message: "no key" },
    ],
    [
      { kind: "failed", message: "boom" },
      { kind: "failed", message: "boom" },
    ],
  ])("maps %j", (failure, problem) => {
    expect(pairingProblem(failure, CONTEXT)).toEqual(problem);
  });

  it("names both machines of a wrong code in a re-pair", () => {
    const failure: PairingFailure = {
      kind: "other_machine",
      daemonId: "019a9b1c-3d00-7a10-8b20-000000000007",
      host: "100.64.0.3",
      fqdn: "homelab.tail1234.ts.net",
    };
    expect(pairingProblem(failure, { machine: "homelab", repairing: "laptop" })).toEqual({
      kind: "other_machine",
      code: "homelab",
      repairing: "laptop",
    });
  });

  it("tells from efrd's protocol which side is too old", () => {
    const older = PROTOCOL_VERSION - 1;
    const newer = PROTOCOL_VERSION + 1;
    expect(
      pairingProblem({ kind: "protocol_mismatch", protocol: older, version: "0.0.3" }, CONTEXT),
    ).toEqual({
      kind: "protocol",
      direction: "efrd_old",
      machine: "desktop",
      protocol: older,
      version: "0.0.3",
    });
    expect(pairingProblem({ kind: "protocol_mismatch", protocol: newer }, CONTEXT)).toMatchObject({
      direction: "app_old",
      protocol: newer,
      version: undefined,
    });
    expect(pairingProblem({ kind: "protocol_mismatch" }, CONTEXT)).toMatchObject({
      direction: "unknown",
    });
  });
});

describe("errorView", () => {
  it("has the words of the five drawn errors (P-Errors)", () => {
    expect(errorView({ kind: "expired" })).toEqual({
      word: "code expired",
      title: [{ text: "This code has expired", look: "plain" }],
      reason: [
        { text: "Codes from ", look: "plain" },
        { text: "efr device add", look: "command" },
        {
          text: " only work for a short time. Run it again and scan the new code.",
          look: "plain",
        },
      ],
      action: "scan_again",
    });

    const wrong = errorView({ kind: "other_machine", code: "homelab", repairing: "laptop" });
    expect(wrong.word).toBe("wrong machine");
    expect(wrong.title).toEqual([
      { text: "This code is from ", look: "plain" },
      { text: "homelab", look: "mono" },
    ]);
    expect(plainText(wrong.reason)).toBe(
      "You're re-pairing laptop, but this code came from homelab. " +
        "Run the command on laptop and scan that code.",
    );
    expect(wrong.action).toBe("scan_again");

    const tailnet = errorView({ kind: "tailnet", machine: "desktop" });
    expect([tailnet.word, plainText(tailnet.title), tailnet.action]).toEqual([
      "tailscale not connected",
      "Tailscale is not connected",
      "open_tailscale",
    ]);
    expect(plainText(tailnet.reason)).toBe(
      "This iPhone isn't on your tailnet, so it can't reach desktop. " +
        "Turn Tailscale on, then come back and try again.",
    );

    const refused = errorView({ kind: "refused", machine: "desktop" });
    expect(refused.title).toEqual([
      { text: "Refused on ", look: "plain" },
      { text: "desktop", look: "mono" },
    ]);
    expect(plainText(refused.reason)).toBe(
      "Someone answered no in the terminal on desktop. Nothing was paired.",
    );
    expect(refused.action).toBe("try_again");

    const old = errorView({
      kind: "protocol",
      direction: "efrd_old",
      machine: "nas",
      protocol: 0,
      version: "0.0.3",
    });
    expect(old.word).toBe("efrd too old");
    expect(old.title).toEqual([
      { text: "efr on ", look: "plain" },
      { text: "nas", look: "mono" },
      { text: " is too old", look: "plain" },
    ]);
    expect(old.reason).toEqual([
      { text: "nas runs efrd ", look: "plain" },
      { text: "0.0.3", look: "mono" },
      {
        text: `, which speaks protocol 0. This app needs protocol ${PROTOCOL_VERSION}. Update efr on nas, then pair again.`,
        look: "plain",
      },
    ]);
    expect(old.action).toBe("done");
  });

  it("says less when efrd did not name its version or protocol", () => {
    expect(
      reason({
        kind: "protocol",
        direction: "efrd_old",
        machine: "nas",
        protocol: 0,
        version: undefined,
      }),
    ).toBe(
      `nas speaks protocol 0. This app needs protocol ${PROTOCOL_VERSION}. Update efr on nas, then pair again.`,
    );
    const unknown = errorView({
      kind: "protocol",
      direction: "unknown",
      machine: "nas",
      protocol: undefined,
      version: undefined,
    });
    expect(plainText(unknown.title)).toBe("efr on nas speaks another protocol");
    expect(plainText(unknown.reason)).toContain("nas speaks another protocol.");
  });

  it("tells the user to update the app when efrd is newer", () => {
    const view = errorView({
      kind: "protocol",
      direction: "app_old",
      machine: "nas",
      protocol: 9,
      version: "0.9.0",
    });
    expect(plainText(view.title)).toBe("This app is too old for nas");
    expect(plainText(view.reason)).toBe(
      `nas runs efrd 0.9.0, which speaks protocol 9. This app speaks protocol ${PROTOCOL_VERSION}. Update the app, then pair again.`,
    );
    expect(view.action).toBe("done");
  });

  it("builds the undrawn errors in the same layout (plan 5.2 item 8.19)", () => {
    expect(title({ kind: "camera" })).toBe("efr cannot use the camera");
    expect(errorView({ kind: "camera" }).action).toBe("open_settings");
    expect(title({ kind: "no_scanner" })).toBe("This iPhone cannot scan the code");
    expect(errorView({ kind: "no_scanner" }).action).toBe("done");
    expect(title({ kind: "code_new" })).toBe("This code is for a newer app");
    expect(errorView({ kind: "bad_code" }).reason).toContainEqual({
      text: "efr device add",
      look: "command",
    });
    expect(reason({ kind: "wrong_daemon", machine: "desktop" })).toBe(
      "The computer at the address of this code is not desktop. " +
        "Run efr device add on desktop again and scan the new code.",
    );
  });

  it("makes a sentence of the message of the flow", () => {
    expect(reason({ kind: "key", message: "the key store failed" })).toBe("The key store failed.");
    expect(reason({ kind: "failed", message: "This build has no key." })).toBe(
      "This build has no key.",
    );
    expect(reason({ kind: "failed", message: " " })).toBe("The app does not know why.");
  });

  it("has exactly one action, with a label, for every problem", () => {
    const problems: PairingProblem[] = [
      { kind: "expired" },
      { kind: "other_machine", code: "a", repairing: "b" },
      { kind: "tailnet", machine: "a" },
      { kind: "refused", machine: "a" },
      {
        kind: "protocol",
        direction: "unknown",
        machine: "a",
        protocol: undefined,
        version: undefined,
      },
      { kind: "code_new" },
      { kind: "bad_code" },
      { kind: "wrong_daemon", machine: "a" },
      { kind: "camera" },
      { kind: "no_scanner" },
      { kind: "key", message: "x" },
      { kind: "failed", message: "x" },
    ];
    for (const problem of problems) {
      const view = errorView(problem);
      expect(ERROR_ACTION_LABEL[view.action]).not.toBe("");
      expect(view.word).toMatch(/^[a-z ]+$/);
    }
  });
});

describe("the other words", () => {
  it("titles the nav row", () => {
    expect(pairingTitle(undefined)).toBe("Pair a machine");
    expect(pairingTitle("laptop")).toBe("Re-pair laptop");
  });

  it("says paired or re-paired on success", () => {
    expect(successWords("desktop", false)).toEqual({
      status: "✓ paired",
      hint: "opens a blank conversation on desktop",
    });
    expect(successWords("laptop", true).status).toBe("✓ re-paired");
  });

  it("writes the efrd line", () => {
    expect(daemonLine({ version: "0.0.5", protocol: 1 })).toBe("efrd 0.0.5 · protocol 1");
  });
});

import { describe, expect, it, jest } from "@jest/globals";
import { fireEvent, render, screen } from "@testing-library/react-native";
import type { ReactNode } from "react";
import { SafeAreaProvider } from "react-native-safe-area-context";

import { PROTOCOL_VERSION } from "@/lib/protocol/generated";

import type { PairingPhase } from "../model/pairing-session";
import type { ErrorAction, PairingProblem } from "../model/pairing-view";
import { pairedMachine } from "../model/testing";
import { NoMachineRoot } from "./no-machine-root";
import { PairingFlow, type PairingFlowProps } from "./pairing-flow";

jest.mock("@/lib/native/clipboard", () => ({
  copyText: jest.fn(() => Promise.resolve(true)),
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));

const DESKTOP = "019a9b1c-3d00-7a10-8b20-0000000000d1";

/** The safe area of an iPhone 17 Pro, as the canvas draws it. */
const METRICS = {
  frame: { x: 0, y: 0, width: 402, height: 874 },
  insets: { top: 62, left: 0, right: 0, bottom: 34 },
};

function Phone(props: { readonly children: ReactNode }) {
  return <SafeAreaProvider initialMetrics={METRICS}>{props.children}</SafeAreaProvider>;
}

/** Renders the modal in `phase`, with a mock for each callback. */
async function flow(phase: PairingPhase, overrides: Partial<PairingFlowProps> = {}) {
  const props = {
    title: "Pair a machine",
    phase,
    onClose: jest.fn<() => void>(),
    onScan: jest.fn<() => void>(),
    onCancel: jest.fn<() => void>(),
    onAction: jest.fn<(action: ErrorAction) => void>(),
    onStart: jest.fn<() => void>(),
    ...overrides,
  };
  await render(<PairingFlow {...props} />, { wrapper: Phone });
  return props;
}

function failed(problem: PairingProblem): PairingPhase {
  return { kind: "failed", problem };
}

describe("the pairing intro (P-Intro)", () => {
  it("shows the command, the hints and Scan code", async () => {
    const props = await flow({ kind: "intro" });
    expect(screen.getByRole("header", { name: "Pair a machine" })).toBeOnTheScreen();
    expect(
      screen.getByText(
        "Run this in a terminal on the machine you want to pair; it shows a code to scan.",
      ),
    ).toBeOnTheScreen();
    expect(screen.getByLabelText("efr device add")).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "Copy command" })).toBeOnTheScreen();
    expect(
      screen.getByText("this iPhone and the machine must be on the same tailnet"),
    ).toBeOnTheScreen();
    expect(
      screen.getByText("next: point the camera at the code in the terminal"),
    ).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "Scan code" }));
    expect(props.onScan).toHaveBeenCalledTimes(1);
  });

  it("closes with the close button", async () => {
    const props = await flow({ kind: "intro" });
    expect(screen.getByTestId("pairing-nav-row")).not.toHaveProp(
      "className",
      expect.stringContaining("opacity-60"),
    );
    await fireEvent.press(screen.getByRole("button", { name: "Close" }));
    expect(props.onClose).toHaveBeenCalledTimes(1);
  });

  it("titles a re-pair with the machine", async () => {
    await flow({ kind: "intro" }, { title: "Re-pair laptop" });
    expect(screen.getByRole("header", { name: "Re-pair laptop" })).toBeOnTheScreen();
  });
});

describe("the scanner step (P-Scan)", () => {
  it("dims the nav row, shows the hint and can cancel", async () => {
    const props = await flow({ kind: "scanning" });
    expect(screen.getByText("point the camera at the code in the terminal")).toBeOnTheScreen();
    expect(screen.getByTestId("pairing-nav-row")).toHaveProp(
      "className",
      expect.stringContaining("opacity-60"),
    );
    expect(screen.queryByRole("button", { name: "Scan code" })).toBeNull();
    await fireEvent.press(screen.getByRole("button", { name: "Cancel" }));
    expect(props.onCancel).toHaveBeenCalledTimes(1);
  });
});

describe("the confirm step (P-Wait)", () => {
  it("shows the machine, the device and the key, and waits", async () => {
    const props = await flow({
      kind: "confirm",
      machine: "desktop",
      device: "iPhone 17 Pro",
      fingerprint: "SHA256:0ed3a6ab957ff6f5",
      saving: false,
    });
    expect(screen.getByRole("header", { name: "Confirm on desktop" })).toBeOnTheScreen();
    expect(
      screen.getByText(
        "The terminal on desktop is asking whether to add this phone. " +
          "Check that it shows the same name and key, then answer there.",
      ),
    ).toBeOnTheScreen();
    expect(screen.getByLabelText("device, iPhone 17 Pro")).toBeOnTheScreen();
    expect(screen.getByLabelText("key, SHA256:0ed3a6ab957ff6f5")).toBeOnTheScreen();
    expect(screen.getByText("waiting for an answer on desktop")).toBeOnTheScreen();
    expect(
      screen.getByRole("status", { name: "waiting for an answer on desktop" }),
    ).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "Cancel" }));
    expect(props.onCancel).toHaveBeenCalledTimes(1);
  });

  it("cannot cancel after the yes, while the phone keeps the pairing", async () => {
    const props = await flow({
      kind: "confirm",
      machine: "desktop",
      device: "iPhone 17 Pro",
      fingerprint: "SHA256:0ed3a6ab957ff6f5",
      saving: true,
    });
    expect(screen.getByText("keeping the pairing on this iPhone")).toBeOnTheScreen();
    const cancel = screen.getByRole("button", { name: "Cancel" });
    expect(cancel).toBeDisabled();
    await fireEvent.press(cancel);
    expect(props.onCancel).not.toHaveBeenCalled();
  });

  it("shows the connecting step without the key box", async () => {
    await flow({ kind: "connecting", machine: "desktop" });
    expect(screen.getByRole("header", { name: "Connecting to desktop" })).toBeOnTheScreen();
    expect(screen.getByText("checking efrd on desktop")).toBeOnTheScreen();
    expect(screen.queryByLabelText(/^device,/)).toBeNull();
  });
});

describe("the success step (P-Success)", () => {
  const machine = pairedMachine(DESKTOP, "desktop");

  it("shows the machine and efrd, and starts", async () => {
    const props = await flow(
      { kind: "paired", machine, repaired: false },
      { daemon: { version: "0.0.5", protocol: 1 } },
    );
    expect(screen.getByText("✓ paired")).toBeOnTheScreen();
    expect(screen.getByRole("header", { name: "desktop" })).toBeOnTheScreen();
    expect(screen.getByText("desktop.tail3f2a.ts.net")).toBeOnTheScreen();
    expect(screen.getByText("efrd 0.0.5 · protocol 1")).toBeOnTheScreen();
    expect(screen.getByText("opens a blank conversation on desktop")).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "Start" }));
    expect(props.onStart).toHaveBeenCalledTimes(1);
  });

  it("waits for the efrd line until the machine is connected", async () => {
    await flow({ kind: "paired", machine, repaired: false });
    expect(screen.getByText("connecting")).toBeOnTheScreen();
    expect(screen.queryByText(/^efrd /)).toBeNull();
  });

  it("says re-paired after a re-pair (plan 5.2 item 8.19)", async () => {
    await flow({ kind: "paired", machine, repaired: true }, { title: "Re-pair desktop" });
    expect(screen.getByText("✓ re-paired")).toBeOnTheScreen();
    expect(screen.getByText("goes back to where you were")).toBeOnTheScreen();
  });
});

describe("the errors (P-Errors)", () => {
  it.each<[string, PairingProblem, string, string, ErrorAction]>([
    ["code expired", { kind: "expired" }, "This code has expired", "Scan again", "scan_again"],
    [
      "wrong machine",
      { kind: "other_machine", code: "homelab", repairing: "laptop" },
      "This code is from homelab",
      "Scan again",
      "scan_again",
    ],
    [
      "tailscale not connected",
      { kind: "tailnet", machine: "desktop" },
      "Tailscale is not connected",
      "Open Tailscale",
      "open_tailscale",
    ],
    [
      "refused",
      { kind: "refused", machine: "desktop" },
      "Refused on desktop",
      "Try again",
      "try_again",
    ],
    [
      "efrd too old",
      {
        kind: "protocol",
        direction: "efrd_old",
        machine: "nas",
        protocol: PROTOCOL_VERSION - 1,
        version: "0.0.3",
      },
      "efr on nas is too old",
      "Done",
      "done",
    ],
    [
      "camera off",
      { kind: "camera" },
      "efr cannot use the camera",
      "Open Settings",
      "open_settings",
    ],
    ["no scanner", { kind: "no_scanner" }, "This iPhone cannot scan the code", "Done", "done"],
  ])("shows %s with its one action", async (word, problem, title, label, action) => {
    const props = await flow(failed(problem));
    expect(screen.getByText(`✗ ${word}`)).toBeOnTheScreen();
    expect(screen.getByRole("header", { name: title })).toBeOnTheScreen();
    expect(screen.getAllByRole("button")).toHaveLength(2);
    await fireEvent.press(screen.getByRole("button", { name: label }));
    expect(props.onAction).toHaveBeenCalledWith(action);
  });

  it("puts the command in mono yolk inside the reason", async () => {
    await flow(failed({ kind: "expired" }));
    expect(screen.getByText("efr device add")).toHaveProp(
      "className",
      expect.stringContaining("text-accent-primary"),
    );
  });

  it("puts the machine of the title in mono", async () => {
    await flow(failed({ kind: "refused", machine: "desktop" }));
    expect(screen.getByText("desktop")).toHaveProp(
      "className",
      expect.stringContaining("text-mono-heading"),
    );
  });
});

describe("the no-machine root (R-NoMachine)", () => {
  it("shows the shell line and the intro without a nav row", async () => {
    const onScan = jest.fn<() => void>();
    await render(<NoMachineRoot onScan={onScan} />, { wrapper: Phone });
    expect(screen.getByText("no machine paired")).toBeOnTheScreen();
    expect(screen.getByTestId("block-cursor", { includeHiddenElements: true })).toHaveStyle({
      opacity: 0.35,
    });
    expect(screen.queryByRole("button", { name: "Close" })).toBeNull();
    expect(screen.getByLabelText("efr device add")).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "Scan code" }));
    expect(onScan).toHaveBeenCalledTimes(1);
  });
});

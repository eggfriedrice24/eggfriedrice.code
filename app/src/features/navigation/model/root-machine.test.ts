import { describe, expect, it } from "vitest";

import { machineOf } from "@/features/machines/model/machine";
import { loadedRegistry } from "@/features/machines/model/machine-registry";
import { DESKTOP, LAPTOP, machineRecord } from "@/features/machines/model/testing";
import { rootMachine } from "./root-machine";

const REGISTRY = loadedRegistry([
  machineOf(machineRecord(DESKTOP), false),
  machineOf(machineRecord(LAPTOP), false),
]);

describe("rootMachine", () => {
  it("takes the machine that the user picked last", () => {
    expect(rootMachine(REGISTRY, LAPTOP)?.name).toBe("laptop");
  });

  it("takes the first machine without a choice, or when the choice is gone", () => {
    expect(rootMachine(REGISTRY, null)?.name).toBe("desktop");
    expect(rootMachine(REGISTRY, "019a9b1c-3d00-7a10-8b20-0000000000ff")?.name).toBe("desktop");
  });

  it("has no machine when none is paired", () => {
    expect(rootMachine(loadedRegistry([]), LAPTOP)).toBeUndefined();
  });
});

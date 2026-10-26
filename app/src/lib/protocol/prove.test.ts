/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";

import { hex } from "./bytes";
import type { HelloResult } from "./generated";
import { PROVE_DOMAIN, proveFields, proveMessage, type ProveFields } from "./prove";

// The frozen fixtures of efr-protocol: the same JSON that the Rust tests read.
const fixtures = import.meta.glob<unknown>(
  [
    "../../../../crates/efr-protocol/fixtures/v1/device_prove_message.json",
    "../../../../crates/efr-protocol/fixtures/v1/hello_result_remote.json",
    "../../../../crates/efr-protocol/fixtures/v1/hello_result.json",
  ],
  { eager: true, import: "default" },
);

function fixture(name: string): Record<string, unknown> {
  const path = Object.keys(fixtures).find((key) => key.endsWith(`/${name}.json`));
  const value = path === undefined ? undefined : fixtures[path];
  if (typeof value !== "object" || value === null) {
    throw new Error(`no fixture ${name}`);
  }
  return { ...value };
}

function helloResult(name: string): HelloResult {
  const value = fixture(name);
  const prove = value["prove"];
  const result: HelloResult = {
    daemon_id: String(value["daemon_id"]),
    protocol: Number(value["protocol"]),
    version: String(value["version"]),
    challenge: String(value["challenge"]),
    paths: { scratch_root: "/s", data_dir: "/d" },
  };
  if (typeof prove === "object" && prove !== null) {
    result.prove = {
      peer_node: String("peer_node" in prove ? prove.peer_node : ""),
      daemon_node: String("daemon_node" in prove ? prove.daemon_node : ""),
      daemon_tailnet: String("daemon_tailnet" in prove ? prove.daemon_tailnet : ""),
    };
  }
  return result;
}

const NIL = "00000000-0000-0000-0000-000000000000";

describe("proveMessage", () => {
  it("gives the bytes of the efr-protocol vector", () => {
    const vector = fixture("device_prove_message");
    const fields: ProveFields = {
      challenge: String(vector["challenge"]),
      daemon_id: String(vector["daemon_id"]),
      protocol: Number(vector["protocol"]),
      device_id: String(vector["device_id"]),
      peer_node: String(vector["peer_node"]),
      daemon_tailnet: String(vector["daemon_tailnet"]),
    };
    expect(hex(proveMessage(fields))).toBe(vector["message_hex"]);
  });

  it("gives the bytes of the nil vector of the Rust tests", () => {
    const bytes = proveMessage({
      challenge: "abc",
      daemon_id: NIL,
      protocol: 1,
      device_id: NIL,
      peer_node: "n1",
      daemon_tailnet: "T1",
    });
    expect(bytes.length).toBe(127);
    expect(hex(bytes)).toBe(
      "000000136566722d6465766963652d70726f76652d763100000003616263000000243030303030303030" +
        "2d303030302d303030302d303030302d30303030303030303030303000000001310000002430303030" +
        "303030302d303030302d303030302d303030302d303030303030303030303030000000026e31000000" +
        "025431",
    );
    expect(PROVE_DOMAIN).toBe("efr-device-prove-v1");
  });

  it("counts the length of each field in bytes, not in UTF-16 units", () => {
    const bytes = proveMessage({
      challenge: "🙂",
      daemon_id: "",
      protocol: 1,
      device_id: "",
      peer_node: "",
      daemon_tailnet: "",
    });
    expect(hex(bytes.slice(23, 31))).toBe("00000004f09f9982");
  });
});

describe("proveFields", () => {
  it("takes the challenge and the prove facts of a remote hello result", () => {
    const fields = proveFields(helloResult("hello_result_remote"), "dev-1");
    expect(fields).toEqual({
      challenge: "3q2-7wAAAAAAAAAAAAAAAA",
      daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007",
      protocol: 1,
      device_id: "dev-1",
      peer_node: "nPHONE1CNTRL",
      daemon_tailnet: "T1234CNTRL",
    });
  });

  it("gives nothing for a hello result of the Unix socket", () => {
    expect(proveFields(helloResult("hello_result"), "dev-1")).toBeUndefined();
  });
});

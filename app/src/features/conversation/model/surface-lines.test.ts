// The quarantine of auto with the reviewer on (Q-Quarantine 7): no question, one quiet
// line per changed file that runs code on the card of its call.

import { describe, expect, it } from "vitest";

import { eventItem, run, toolCallStarted, turnStarted } from "@/lib/stream/testing";
import { surfaceLines, surfaceWhat } from "./exit-words";

/** A new hook at `path` that went to quarantine. */
function change(path: string) {
  return { path, rule: "hook_planted", quarantined: true };
}

describe("surfaceLines", () => {
  it("says what moved to quarantine and what stayed, relative to the working directory", () => {
    const changes = [
      {
        path: "/home/me/code/efr/.git/config",
        rule: "config_code_key",
        key: "core.fsmonitor",
        quarantined: true,
      },
      { path: "/home/me/code/efr/.git/hooks/pre-commit", rule: "hook_planted", quarantined: true },
      { path: "/home/me/code/efr/.envrc", rule: "protected_name_created", quarantined: false },
      { path: "/home/me/other/.git/config", rule: "some_new_rule", quarantined: true },
    ];
    expect(surfaceLines(changes, "/home/me/code/efr")).toEqual([
      ".git/config (core.fsmonitor); moved to quarantine",
      ".git/hooks/pre-commit (a new hook); moved to quarantine",
      ".envrc (a new agent or editor config); left in place",
      "~/other/.git/config (some new rule); moved to quarantine",
    ]);
  });

  it("names a change by its code key, else by its rule", () => {
    expect(surfaceWhat({ path: "/x", rule: "commondir_in_main_git_dir", quarantined: true })).toBe(
      "a commondir that points git at another config",
    );
    expect(
      surfaceWhat({ path: "/x", rule: "config_code_key", key: "core.pager", quarantined: true }),
    ).toBe("core.pager");
  });

  it("collects the changes of each sandbox_surface_changed on the tool call", () => {
    const state = run([
      eventItem(1, turnStarted("t1")),
      eventItem(2, toolCallStarted("t1", "c1", "shell")),
      eventItem(3, {
        kind: "sandbox_surface_changed",
        turn_id: "t1",
        call_id: "c1",
        changes: [change("/a")],
        quarantined: true,
      }),
      eventItem(4, {
        kind: "sandbox_surface_changed",
        turn_id: "t1",
        call_id: "c1",
        changes: [change("/b")],
        quarantined: true,
      }),
    ]);
    expect(
      state.turns
        .get("t1")
        ?.tools.get("c1")
        ?.surface?.map((one) => one.path),
    ).toEqual(["/a", "/b"]);
  });
});

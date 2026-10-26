import { QueryObserver, focusManager, onlineManager } from "@tanstack/react-query";
import { afterEach, describe, expect, it } from "vitest";

import {
  focusOfAppState,
  onlineOfNetInfo,
  wireQueryManagers,
  type QueryManagerSources,
  type Subscribe,
} from "./managers";
import { createQueryClient } from "./query-client";
import { FakeCaller, settle } from "./testing";

/** A source that a test drives, with the number of listeners it has now. */
class FakeSource<T> {
  private readonly listeners = new Set<(value: T) => void>();

  readonly subscribe: Subscribe<T> = (listener) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  get count(): number {
    return this.listeners.size;
  }

  emit(value: T): void {
    for (const listener of this.listeners) {
      listener(value);
    }
  }
}

function fakeSources(): {
  readonly active: FakeSource<boolean>;
  readonly online: FakeSource<boolean>;
  readonly sources: QueryManagerSources;
} {
  const active = new FakeSource<boolean>();
  const online = new FakeSource<boolean>();
  return {
    active,
    online,
    sources: { appActive: active.subscribe, networkOnline: online.subscribe },
  };
}

describe("wireQueryManagers", () => {
  afterEach(() => {
    focusManager.setFocused(undefined);
    onlineManager.setOnline(true);
  });

  it("feeds the focus and the network into the managers of Query", () => {
    const { active, online, sources } = fakeSources();
    const stop = wireQueryManagers(sources);
    expect(active.count).toBe(1);
    expect(online.count).toBe(1);

    active.emit(false);
    expect(focusManager.isFocused()).toBe(false);
    active.emit(true);
    expect(focusManager.isFocused()).toBe(true);

    online.emit(false);
    expect(onlineManager.isOnline()).toBe(false);
    online.emit(true);
    expect(onlineManager.isOnline()).toBe(true);

    stop();
    expect(active.count).toBe(0);
    expect(online.count).toBe(0);
  });

  it("drops the sources of an earlier wiring", () => {
    const first = fakeSources();
    const second = fakeSources();
    wireQueryManagers(first.sources);
    const stop = wireQueryManagers(second.sources);
    expect(first.active.count).toBe(0);
    expect(second.active.count).toBe(1);
    stop();
  });

  it("fetches a stale query again when the app comes back", async () => {
    const { active, sources } = fakeSources();
    const stop = wireQueryManagers(sources);
    const client = createQueryClient();
    client.mount();
    const caller = new FakeCaller({ "conversations.recent": () => ({ conversations: [] }) });
    const observer = new QueryObserver(client, {
      queryKey: ["machine", "m1", "probe"],
      queryFn: () => caller.call("m1", "conversations.recent", {}),
      staleTime: 0,
    });
    const unsubscribe = observer.subscribe(() => undefined);
    await settle();
    expect(caller.calls).toHaveLength(1);

    active.emit(false);
    await settle();
    expect(caller.calls).toHaveLength(1);
    active.emit(true);
    await settle();
    expect(caller.calls).toHaveLength(2);

    unsubscribe();
    client.unmount();
    stop();
  });

  it("works with any managers that take an event listener", () => {
    const setups: string[] = [];
    const { active, sources } = fakeSources();
    const focused: (boolean | undefined)[] = [];
    const stop = wireQueryManagers(sources, {
      focus: {
        setEventListener: (setup) => {
          setups.push("focus");
          setup((value) => focused.push(value));
        },
      },
      online: {
        setEventListener: (setup) => {
          setups.push("online");
          setup(() => undefined);
        },
      },
    });
    active.emit(true);
    expect(focused).toEqual([true]);
    stop();
    expect(setups).toEqual(["focus", "online", "focus", "online"]);
  });
});

describe("focusOfAppState", () => {
  it("reads active and background, and nothing else", () => {
    expect(focusOfAppState("active")).toBe(true);
    expect(focusOfAppState("background")).toBe(false);
    expect(focusOfAppState("inactive")).toBeUndefined();
    expect(focusOfAppState("unknown")).toBeUndefined();
  });
});

describe("onlineOfNetInfo", () => {
  it("counts the phone as online until NetInfo says it has no network", () => {
    expect(onlineOfNetInfo({ isConnected: true })).toBe(true);
    expect(onlineOfNetInfo({ isConnected: null })).toBe(true);
    expect(onlineOfNetInfo({ isConnected: false })).toBe(false);
  });
});

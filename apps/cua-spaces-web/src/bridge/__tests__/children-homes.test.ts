// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The host says which Spaces are agent homes (from their labels), so the store can hand them out.

import { describe, expect, it } from "vitest";

import type { DataAdapter } from "../adapter";
import { createChildrenStore } from "../children";

const adapter = (tree: unknown[]) => ({ call: async (op: string) => (op === "spaces.tree" ? tree : null) }) as unknown as DataAdapter;
const settle = () => new Promise((r) => setTimeout(r, 0));

describe("agent homes in the children store", () => {
  it("lists the homes, the ones made for the agent first, and ignores Spaces that are not homes", async () => {
    const store = createChildrenStore(
      adapter([
        { id: "local:z-home", parent: null, depth: 0, children: true, home: "openai-codex" },
        { id: "local:a-home", parent: null, depth: 0, children: true, home: "claude-code" },
        { id: "local:b-home", parent: null, depth: 0, children: true, home: "claude-code" },
        { id: "local:plain", parent: null, depth: 0, children: false, home: null },
        { id: "local:old-host", parent: null, depth: 0, children: false },
      ]),
    );
    store.homesOf("claude-code"); // starts the read
    await settle();
    expect(store.homesOf("claude-code")).toEqual(["local:a-home", "local:b-home", "local:z-home"]);
    expect(store.homesOf("openai-codex")).toEqual(["local:z-home", "local:a-home", "local:b-home"]);
    expect(store.node("local:a-home")?.home).toBe("claude-code");
    expect(store.node("local:plain")?.home).toBeUndefined();
    expect(store.node("local:old-host")?.home).toBeUndefined();
  });

  it("offers no home from a host that does not answer the tree", async () => {
    const store = createChildrenStore({
      call: async () => Promise.reject(Object.assign(new Error("unsupported"), { unsupported: true })),
    } as unknown as DataAdapter);
    store.homesOf("claude-code");
    await settle();
    expect(store.homesOf("claude-code")).toEqual([]);
  });
});

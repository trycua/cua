// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `thread.*`: the daemon's thread tools with the web bridge's own argument
// mapping, the answer untouched, and an old daemon answering `unsupported`.
import { describe, expect, it, vi } from "vitest";
import { threadPageFromTool } from "../../cua-spaces-web/src/bridge/ops/thread";
import type { BridgeContext } from "../src/bridge/context";
import { threadMethods } from "../src/bridge/thread";
import { ToolError } from "../src/model/backend";

const ctx = {} as BridgeContext;
const PAGE = { entries: [{ seq: 1, id: "e1", ts_ms: 1, author: { kind: "person", id: "acct_1", name: "Ada" }, kind: "message", text: "hi" }], cursor: 1, caught_up: true, unread: 0 };

describe("thread.*", () => {
  it("reads with the tool's arguments and answers its JSON as-is", async () => {
    const tool = vi.fn(async () => PAGE);
    const m = threadMethods(ctx, () => tool);
    const r = await m["thread.read"]!({ spaceId: "local:dev", after: 4, markRead: true, followMs: 99_000 }, {});
    expect(tool).toHaveBeenCalledWith("thread_read", { space: "local:dev", after: 4, mark_read: true, follow_ms: 25_000 });
    expect(r).toBe(PAGE);
    expect(threadPageFromTool(r).entries[0]!.author.name).toBe("Ada");
  });

  it("posts an approval's answer, and lists the members", async () => {
    const tool = vi.fn(async () => ({}));
    const m = threadMethods(ctx, () => tool);
    await m["thread.post"]!({ spaceId: "local:dev", text: "Approved", answer: { approval: 12, approve: true } }, {});
    expect(tool).toHaveBeenCalledWith("thread_post", { space: "local:dev", text: "Approved", answer: { approval: 12, approve: true } });
    await m["thread.members"]!({ spaceId: "local:dev" }, {});
    expect(tool).toHaveBeenLastCalledWith("agent_members", { space: "local:dev" });
  });

  it("sets an agent's rights by its name, not the Space", async () => {
    const tool = vi.fn(async () => ({}));
    const m = threadMethods(ctx, () => tool);
    await m["thread.setRights"]!({ spaceId: "local:dev", name: "hermes", task: "owner", approve: ["acct_mia"] }, {});
    expect(tool).toHaveBeenCalledWith("agent_set_rights", { name: "hermes", task: "owner", approve: ["acct_mia"] });
    await expect(m["thread.setRights"]!({ spaceId: "local:dev" }, {})).rejects.toMatchObject({ code: "bad_args" });
  });

  it("adds and removes agents, and runs the join codes and settings", async () => {
    const tool = vi.fn(async () => ({}));
    const m = threadMethods(ctx, () => tool);
    await m["thread.agentAdd"]!({ spaceId: "local:dev", name: "codex", harness: "openai-codex" }, {});
    expect(tool).toHaveBeenLastCalledWith("agent_add", { space: "local:dev", name: "codex", agent: "openai-codex" });
    await m["thread.agentRemove"]!({ spaceId: "local:dev", name: "codex" }, {});
    expect(tool).toHaveBeenLastCalledWith("agent_remove", { name: "codex" });
    await m["thread.joinCodeCreate"]!({ spaceId: "local:dev", ttlS: 900, canTask: "members", askBeforeJoin: true }, {});
    expect(tool).toHaveBeenLastCalledWith("thread_join_code_create", { space: "local:dev", ttl_s: 900, can_task: "members", ask_before_join: true });
    await m["thread.joinCodeList"]!({ spaceId: "local:dev" }, {});
    expect(tool).toHaveBeenLastCalledWith("thread_join_code_list", { space: "local:dev" });
    await m["thread.joinCodeCancel"]!({ spaceId: "local:dev", id: "jc_1" }, {});
    expect(tool).toHaveBeenLastCalledWith("thread_join_code_cancel", { space: "local:dev", id: "jc_1" });
    await m["thread.settings"]!({ spaceId: "local:dev", askBeforeJoin: false }, {});
    expect(tool).toHaveBeenLastCalledWith("thread_settings", { space: "local:dev", ask_before_join: false });
    await expect(m["thread.agentAdd"]!({ spaceId: "local:dev", name: "x" }, {})).rejects.toMatchObject({ code: "bad_args" });
    await expect(m["thread.joinCodeCancel"]!({ spaceId: "local:dev" }, {})).rejects.toMatchObject({ code: "bad_args" });
    await expect(m["thread.joinCodeCreate"]!({ spaceId: "local:dev", canTask: "all" }, {})).rejects.toMatchObject({ code: "bad_args" });
  });

  it("refuses a post without text", async () => {
    const m = threadMethods(ctx, () => vi.fn());
    await expect(m["thread.post"]!({ spaceId: "local:dev" }, {})).rejects.toMatchObject({ code: "bad_args" });
  });

  it("answers unsupported when the daemon or the SDK has no thread", async () => {
    for (const e of [new Error("NotFound: Spaces tool thread_read"), new ToolError("unknown tool thread_read")]) {
      const m = threadMethods(ctx, () => async () => Promise.reject(e));
      await expect(m["thread.read"]!({ spaceId: "local:dev" }, {})).rejects.toMatchObject({ code: "unsupported" });
    }
    // Any other failure stays what it was.
    const m = threadMethods(ctx, () => async () => Promise.reject(new ToolError("not_found: no Space local:gone")));
    await expect(m["thread.read"]!({ spaceId: "local:gone" }, {})).rejects.toThrow("no Space local:gone");
  });
});

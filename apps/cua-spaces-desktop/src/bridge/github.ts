// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings → Connections → GitHub (`github.status`, `github.rescan`,
// `github.use`, `github.enable`; the web bridge's ops/github.ts): the daemon's
// app methods of the same names, as the SwiftUI host runs them
// (`WebUIBridge+Pages.swift`). They answer the hosts, the accounts, the source
// in use and the scopes of the sign-in the host uses for an approved push or pull
// request, never a token; the page maps the answer. Only the arguments the method
// takes cross to the daemon, each checked for its type.
import { optionalString, string } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { daemonTool } from "./host-parts";

type Tool = (name: string, args?: Record<string, unknown>) => Promise<unknown>;

/** `tool`: the daemon's tools (tests pass their own). */
export function githubMethods(ctx: BridgeContext, tool: () => Tool = () => daemonTool(ctx, "The GitHub sign-in")): Handlers {
  return {
    "github.status": async () => tool()("github.status", {}),
    "github.rescan": async () => tool()("github.rescan", {}),
    "github.use": async (args) => {
      const host = string(args, "host");
      const login = optionalString(args, "login");
      return tool()("github.use", { host, ...(login ? { login } : {}) });
    },
    "github.enable": async (args) => {
      const host = string(args, "host");
      if (typeof args.enabled !== "boolean") throw Failure.badArgs("enabled: boolean");
      return tool()("github.enable", { host, enabled: args.enabled });
    },
  };
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// `bash`, `write` and `edit` that need only command execution in the Space,
// for backends whose cua-spacesd stand-in has no stdin pipe for the file
// daemon (CRASH_BACKEND=local). They keep the arguments of Pi's tools of the
// same names, so the campaign's plan and reference model are unchanged.

import { defineExtension, defineTool } from "@earendil-works/pi-durable"
import { createBashTool } from "@earendil-works/pi-durable/tools"
import { Type } from "typebox"
import { resolve } from "node:path"

const EDIT = [
  "const fs=require('fs');const p=process.env.P;const t=fs.readFileSync(p,'utf8');const o=process.env.OLD;",
  "if(t.split(o).length!==2){console.error('oldText is not unique in '+p);process.exit(3)}",
  "fs.writeFileSync(p,t.replace(o,()=>process.env.NEW))",
].join("")

export function execTools(work) {
  const run = async (api, ctx, argv, env) => {
    const result = await api.env.exec(argv, { env, cwd: work }, ctx)
    if (!result.ok) throw result.error
    if (result.value.exitCode !== 0) throw new Error(`exit ${result.value.exitCode}: ${result.value.stderr ?? ""}`)
  }
  const write = defineTool({
    name: "write",
    description: "Write a file",
    parameters: Type.Object({ path: Type.String(), content: Type.String() }),
    async execute(args, api, ctx) {
      await run(api, ctx, ["sh", "-c", 'mkdir -p "$(dirname "$P")" && printf %s "$C" > "$P"'], {
        P: resolve(work, args.path),
        C: args.content,
      })
      return { content: [{ type: "text", text: `wrote ${args.path}` }] }
    },
  })
  const edit = defineTool({
    name: "edit",
    description: "Replace unique text in a file",
    parameters: Type.Object({
      path: Type.String(),
      edits: Type.Array(Type.Object({ oldText: Type.String(), newText: Type.String() })),
    }),
    async execute(args, api, ctx) {
      for (const e of args.edits) {
        await run(api, ctx, [process.execPath, "-e", EDIT], { P: resolve(work, args.path), OLD: e.oldText, NEW: e.newText })
      }
      return { content: [{ type: "text", text: `edited ${args.path}` }] }
    },
  })
  return defineExtension({ name: "crash-exec-tools", tools: [createBashTool(), write, edit] })
}

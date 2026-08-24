import assert from "node:assert/strict"
import test from "node:test"
import { Bash } from "just-bash/browser"
import {
  BrowserBashAgent,
  normalizeBashArguments,
  sanitizeError,
  type AssistantMessage,
  type AgentEvent,
  type TurnClient,
} from "./src/browser-agent.ts"
import { createBrowserMcpCommands, createBrowserMcpToolCaller, type McpClient } from "./src/browser-mcp-commands.ts"
import { createBrowserSdkCommands, type BrowserSdk } from "./src/browser-sdk-commands.ts"
import { gzipSync, gunzipSync } from "./src/node-zlib-browser.ts"


const inertSdk: BrowserSdk = {
  listNamespaces: async () => [],
  listPools: async () => [],
  getPool: async () => ({}),
  createPool: async () => ({}),
  updatePoolServices: async () => undefined,
  deletePool: async () => undefined,
  listClaims: async () => [],
  createClaim: async () => ({}),
  getClaim: async () => ({}),
  deleteClaim: async () => undefined,
  listUserKeys: async () => [],
  createUserKey: async () => ({}),
  deleteUserKey: async () => undefined,
}

function sdkShell(overrides: Partial<BrowserSdk> = {}): Bash {
  const sdk: BrowserSdk = { ...inertSdk, ...overrides }
  return new Bash({ customCommands: createBrowserSdkCommands(sdk) })
}

test("exposes SDK methods as composable bash commands", async () => {
  const shell = sdkShell({
    listPools: async () => [
      { name: "beta", namespace: "team", phase: "Ready" },
      { name: "alpha", namespace: "team", phase: "Pending" },
    ],
  })

  const result = await shell.exec("listPools | jq -r '.[].name' | sort")

  assert.equal(result.exitCode, 0)
  assert.equal(result.stdout, "alpha\nbeta\n")
  assert.equal(result.stderr, "")
})

test("passes a JSON argument array to SDK methods", async () => {
  const calls: unknown[][] = []
  const shell = sdkShell({
    getPool: async (...args) => {
      calls.push(args)
      return { namespace: args[0], name: args[1] }
    },
  })

  const result = await shell.exec(`getPool '["team","alpha"]'`)

  assert.deepEqual(calls, [["team", "alpha"]])
  assert.equal(result.stdout, '{"namespace":"team","name":"alpha"}\n')
})

test("reads SDK argument arrays from stdin", async () => {
  const calls: unknown[][] = []
  const shell = sdkShell({
    listClaims: async (...args) => {
      calls.push(args)
      return [{ namespace: args[0], name: "claim-one" }]
    },
  })

  const result = await shell.exec(`printf '["team"]' | listClaims`)

  assert.deepEqual(calls, [["team"]])
  assert.equal(result.stdout, '[{"namespace":"team","name":"claim-one"}]\n')
})

test("emits no stdout for void SDK methods", async () => {
  const calls: unknown[][] = []
  const shell = sdkShell({
    deleteClaim: async (...args) => {
      calls.push(args)
    },
  })

  const result = await shell.exec(`deleteClaim '["team","claim-one"]'`)

  assert.deepEqual(calls, [["team", "claim-one"]])
  assert.equal(result.exitCode, 0)
  assert.equal(result.stdout, "")
})

test("returns argument and SDK failures as shell errors", async () => {
  const shell = sdkShell({
    getPool: async () => {
      throw new Error("pool unavailable")
    },
  })

  const malformed = await shell.exec("getPool not-json")
  assert.notEqual(malformed.exitCode, 0)
  assert.match(malformed.stderr, /JSON array/)

  const rejected = await shell.exec(`getPool '["team","alpha"]'`)
  assert.notEqual(rejected.exitCode, 0)
  assert.equal(rejected.stderr, "pool unavailable\n")
})

test("exposes exact MCP tool names as composable bash commands", async () => {
  const calls: Array<{ name: string; arguments: Record<string, unknown> }> = []
  const shell = new Bash({
    customCommands: createBrowserMcpCommands(async (name, arguments_) => {
      calls.push({ name, arguments: arguments_ })
      return { content: [{ type: "text", text: "docs result" }] }
    }),
  })

  const result = await shell.exec(`query_docs_vectors '{"query":"agent loop","limit":3}' | jq -r '.content[0].text'`)

  assert.deepEqual(calls, [{ name: "query_docs_vectors", arguments: { query: "agent loop", limit: 3 } }])
  assert.equal(result.exitCode, 0)
  assert.equal(result.stdout, "docs result\n")
  assert.equal(result.stderr, "")
})

test("reads MCP tool arguments from stdin", async () => {
  const calls: Array<{ name: string; arguments: Record<string, unknown> }> = []
  const shell = new Bash({
    customCommands: createBrowserMcpCommands(async (name, arguments_) => {
      calls.push({ name, arguments: arguments_ })
      return { structuredContent: { rows: [{ component: "agent" }] } }
    }),
  })

  const result = await shell.exec(`printf '{"sql":"SELECT component FROM code_files LIMIT 1"}' | query_code_db`)

  assert.deepEqual(calls, [{ name: "query_code_db", arguments: { sql: "SELECT component FROM code_files LIMIT 1" } }])
  assert.equal(result.stdout, '{"structuredContent":{"rows":[{"component":"agent"}]}}\n')
})

test("returns MCP argument and tool failures as shell errors", async () => {
  const shell = new Bash({
    customCommands: createBrowserMcpCommands(async () => ({
      isError: true,
      content: [{ type: "text", text: "read-only query rejected" }],
    })),
  })

  const malformed = await shell.exec("query_docs_db not-json")
  assert.notEqual(malformed.exitCode, 0)
  assert.match(malformed.stderr, /JSON object/)

  const rejected = await shell.exec(`query_docs_db '{"sql":"DELETE FROM pages"}'`)
  assert.notEqual(rejected.exitCode, 0)
  assert.equal(rejected.stderr, "read-only query rejected\n")
})

test("shares one lazy MCP connection and retries failed connections", async () => {
  let attempts = 0
  const client: McpClient = {
    callTool: async ({ name }) => ({ content: [{ type: "text", text: name }] }),
  }
  const callTool = createBrowserMcpToolCaller(async () => {
    attempts += 1
    if (attempts === 1) throw new Error("temporary connection failure")
    return client
  })

  await assert.rejects(callTool("query_docs_db", { sql: "SELECT 1" }), /temporary connection failure/)
  const results = await Promise.all([
    callTool("query_docs_db", { sql: "SELECT 1" }),
    callTool("query_code_vectors", { query: "screenshots" }),
  ])

  assert.equal(attempts, 2)
  assert.deepEqual(results, [
    { content: [{ type: "text", text: "query_docs_db" }] },
    { content: [{ type: "text", text: "query_code_vectors" }] },
  ])
})

test("clamps model-selected bash limits", () => {
  assert.deepEqual(
    normalizeBashArguments({ command: "echo ok", timeout_ms: 1, max_output_chars: 9_999_999 }),
    { command: "echo ok", timeout_ms: 250, max_output_chars: 100_000 },
  )
})

test("sanitizes control characters in bash execution errors", () => {
  assert.equal(sanitizeError(new Error("\u0000failed\nwith\u007fcontrols\u0085")), "failed with controls")
  assert.equal(sanitizeError(new Error("\u0000\u001f\u007f\u009f")), "Bash execution failed")
})

test("keeps one virtual filesystem per conversation", async () => {
  const unused: TurnClient = async () => ({ role: "assistant", content: "done", tool_calls: [] })
  const agent = new BrowserBashAgent(unused, createBrowserSdkCommands(inertSdk))
  await agent.executeBash("one", { command: "echo alpha > note.txt" })
  assert.equal((await agent.executeBash("one", { command: "cat note.txt" })).stdout, "alpha\n")
  assert.notEqual((await agent.executeBash("two", { command: "cat note.txt" })).exit_code, 0)
})

test("browser zlib shim fails only when gzip-backed commands execute", async () => {
  const agent = new BrowserBashAgent(async () => ({ role: "assistant", content: "", tool_calls: [] }), createBrowserSdkCommands(inertSdk))
  assert.equal((await agent.executeBash("zlib-shim", { command: "printf normal" })).stdout, "normal")

  assert.throws(() => gzipSync(new Uint8Array()), /gzip commands are unsupported in the browser/)
  assert.throws(() => gunzipSync(new Uint8Array()), /gzip commands are unsupported in the browser/)
})

test("runs tool calls until the assistant finishes", async () => {
  const requests: Parameters<TurnClient>[1][] = []
  const responses: AssistantMessage[] = [
    {
      role: "assistant",
      content: "",
      tool_calls: [{ id: "call-1", type: "function", function: { name: "bash", arguments: '{"command":"printf hello"}' } }],
    },
    { role: "assistant", content: "The command printed hello.", tool_calls: [] },
  ]
  const client: TurnClient = async (_id, messages, _signal, onDelta) => {
    requests.push(messages)
    onDelta?.(requests.length === 1 ? "Running bash." : "Bash finished.")
    return responses.shift()!
  }
  const events: AgentEvent[] = []
  const agent = new BrowserBashAgent(client, createBrowserSdkCommands(inertSdk))
  await agent.run("conversation-1", [{ role: "user", content: "Run it" }], event => events.push(event))

  assert.deepEqual(requests, [
    [{ role: "user", content: "Run it" }],
    [
      {
        role: "tool",
        tool_call_id: "call-1",
        content: JSON.stringify({ stdout: "hello", stderr: "", exit_code: 0, timed_out: false, truncated: false }),
      },
    ],
  ])
  assert.deepEqual(
    events.map(event => event.type),
    ["generation_start", "assistant_delta", "tool_start", "tool_result", "generation_start", "assistant_delta", "complete"],
  )
  assert.deepEqual(events[1], { type: "assistant_delta", delta: "Running bash." })
  assert.deepEqual(events[5], { type: "assistant_delta", delta: "Bash finished." })
})

test("normalizes timeout and truncation results", async () => {
  const agent = new BrowserBashAgent(async () => ({ role: "assistant", content: "", tool_calls: [] }), createBrowserSdkCommands(inertSdk))
  const truncated = await agent.executeBash("one", { command: "printf 123456", max_output_chars: 4 })
  assert.deepEqual(truncated, { stdout: "1234", stderr: "", exit_code: 0, timed_out: false, truncated: true })

  const maximum = await agent.executeBash("one", {
    command: `printf ${"x".repeat(100_001)}`,
    max_output_chars: 100_001,
  })
  assert.equal(maximum.stdout.length + maximum.stderr.length, 100_000)
  assert.equal(maximum.truncated, true)

  const timedOut = await agent.executeBash("one", { command: "sleep 2", timeout_ms: 250 })
  assert.equal(timedOut.exit_code, 124)
  assert.equal(timedOut.timed_out, true)
})

test("caller cancellation aborts bash execution and stops the tool loop", async () => {
  let requests = 0
  const controller = new AbortController()
  const events: AgentEvent[] = []
  const agent = new BrowserBashAgent(async () => {
    requests += 1
    return {
      role: "assistant",
      content: "",
      tool_calls: [
        { id: "call-cancel", type: "function", function: { name: "bash", arguments: '{"command":"sleep 2"}' } },
      ],
    }
  }, createBrowserSdkCommands(inertSdk))

  const run = agent.run(
    "conversation-cancel",
    [{ role: "user", content: "Run it" }],
    event => {
      events.push(event)
      if (event.type === "tool_start") {
        controller.abort()
      }
    },
    controller.signal,
  )

  await assert.rejects(run, { name: "AbortError" })
  assert.equal(requests, 1)
  assert.deepEqual(events.map(event => event.type), ["generation_start", "tool_start"])
})

test("batches multiple bash tool results into one follow-up turn", async () => {
  const requests: Parameters<TurnClient>[1][] = []
  const responses: AssistantMessage[] = [
    {
      role: "assistant",
      content: "",
      tool_calls: [
        { id: "call-left", type: "function", function: { name: "bash", arguments: '{"command":"printf left"}' } },
        { id: "call-right", type: "function", function: { name: "bash", arguments: '{"command":"printf right"}' } },
      ],
    },
    { role: "assistant", content: "Both commands completed.", tool_calls: [] },
  ]
  const client: TurnClient = async (_id, messages) => {
    requests.push(messages)
    return responses.shift()!
  }
  const events: AgentEvent[] = []
  const agent = new BrowserBashAgent(client, createBrowserSdkCommands(inertSdk))

  await agent.run("conversation-batch", [{ role: "user", content: "Run both commands" }], event => events.push(event))

  assert.equal(requests.length, 2)
  assert.deepEqual(requests[1], [
    {
      role: "tool",
      tool_call_id: "call-left",
      content: JSON.stringify({ stdout: "left", stderr: "", exit_code: 0, timed_out: false, truncated: false }),
    },
    {
      role: "tool",
      tool_call_id: "call-right",
      content: JSON.stringify({ stdout: "right", stderr: "", exit_code: 0, timed_out: false, truncated: false }),
    },
  ])
  assert.deepEqual(
    events.map(event => event.type),
    ["generation_start", "tool_start", "tool_start", "tool_result", "tool_result", "generation_start", "complete"],
  )
  assert.deepEqual(
    events
      .filter((event): event is Extract<AgentEvent, { type: "tool_start" }> => event.type === "tool_start")
      .map(event => event.toolCall.id),
    ["call-left", "call-right"],
  )
  assert.deepEqual(
    events
      .filter((event): event is Extract<AgentEvent, { type: "tool_result" }> => event.type === "tool_result")
      .map(event => ({ id: event.toolCall.id, result: event.result }))
      .sort((left, right) => left.id.localeCompare(right.id)),
    [
      { id: "call-left", result: { stdout: "left", stderr: "", exit_code: 0, timed_out: false, truncated: false } },
      { id: "call-right", result: { stdout: "right", stderr: "", exit_code: 0, timed_out: false, truncated: false } },
    ],
  )
})

test("rejects mixed tool batches before bash side effects", async () => {
  const events: AgentEvent[] = []
  const agent = new BrowserBashAgent(async () => ({
    role: "assistant",
    content: "",
    tool_calls: [
      {
        id: "call-create-file",
        type: "function",
        function: { name: "bash", arguments: '{"command":"touch should-not-exist"}' },
      },
      { id: "call-unsupported", type: "function", function: { name: "other", arguments: "{}" } },
    ],
  }), createBrowserSdkCommands(inertSdk))

  await assert.rejects(
    agent.run("conversation-mixed", [{ role: "user", content: "Run the tools" }], event => events.push(event)),
    { message: "Unsupported tool: other" },
  )

  assert.deepEqual(events.map(event => event.type), ["generation_start"])
  assert.notEqual((await agent.executeBash("conversation-mixed", { command: "test -e should-not-exist" })).exit_code, 0)
})

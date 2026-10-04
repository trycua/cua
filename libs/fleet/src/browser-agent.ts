import { Bash, type Command } from "just-bash/browser"
import type { SensitiveOutputBuffer, SensitiveOutput } from "./sensitive-output.js"

export const BASH_TIMEOUT = { min: 250, default: 10_000, max: 60_000 } as const
export const BASH_OUTPUT = { min: 256, default: 20_000, max: 100_000 } as const

export interface BashToolArguments {
  command: string
  timeout_ms?: number
  max_output_chars?: number
}

export interface NormalizedBashArguments {
  command: string
  timeout_ms: number
  max_output_chars: number
}

export interface BashToolResult {
  stdout: string
  stderr: string
  exit_code: number
  timed_out: boolean
  truncated: boolean
}

export interface ToolCall {
  id: string
  type: "function"
  function: { name: string; arguments: string }
}

export interface ClientMessage {
  role: "user" | "tool"
  content: string
  tool_call_id?: string
}

export interface AssistantMessage {
  role: "assistant"
  content: string
  tool_calls: ToolCall[]
}

export type TurnClient = (
  conversationID: string,
  messages: ClientMessage[],
  signal?: AbortSignal,
  onDelta?: (delta: string) => void,
) => Promise<AssistantMessage>

export type AgentEvent =
  | { type: "generation_start" }
  | { type: "assistant_delta"; delta: string }
  | { type: "tool_start"; toolCall: ToolCall; arguments: NormalizedBashArguments }
  | { type: "tool_result"; toolCall: ToolCall; arguments: NormalizedBashArguments; result: BashToolResult; sensitiveOutputs: SensitiveOutput[] }
  | { type: "complete"; message: AssistantMessage }

export function normalizeBashArguments(input: BashToolArguments): NormalizedBashArguments {
  if (!input || typeof input.command !== "string" || input.command.trim() === "") {
    throw new Error("bash.command must be a non-empty string")
  }
  const clamp = (value: number | undefined, bounds: typeof BASH_TIMEOUT | typeof BASH_OUTPUT) =>
    Math.min(bounds.max, Math.max(bounds.min, Number.isFinite(value) ? Math.trunc(value!) : bounds.default))
  return {
    command: input.command,
    timeout_ms: clamp(input.timeout_ms, BASH_TIMEOUT),
    max_output_chars: clamp(input.max_output_chars, BASH_OUTPUT),
  }
}

function abortError(): DOMException {
  return new DOMException("The operation was aborted", "AbortError")
}

export function sanitizeError(error: unknown): string {
  const message = error instanceof Error ? error.message : "Bash execution failed"
  const sanitized = Array.from(message, character => {
    const codePoint = character.codePointAt(0) ?? 0
    return codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f) ? " " : character
  })
    .join("")
    .trim()
  return sanitized || "Bash execution failed"
}

function capOutput(stdout: string, stderr: string, maxOutputChars: number): Pick<BashToolResult, "stdout" | "stderr" | "truncated"> {
  const truncated = stdout.length + stderr.length > maxOutputChars
  const cappedStdout = stdout.slice(0, maxOutputChars)
  const remaining = Math.max(0, maxOutputChars - cappedStdout.length)
  return { stdout: cappedStdout, stderr: stderr.slice(0, remaining), truncated }
}

export class BrowserBashAgent {
  private readonly shells = new Map<string, Bash>()
  private readonly client: TurnClient
  private readonly customCommands: Command[]
  private readonly sensitiveOutputs?: SensitiveOutputBuffer

  constructor(
    client: TurnClient,
    customCommands: Command[] = [],
    sensitiveOutputs?: SensitiveOutputBuffer,
  ) {
    this.client = client
    this.customCommands = customCommands
    this.sensitiveOutputs = sensitiveOutputs
  }

  async executeBash(conversationID: string, input: BashToolArguments, signal?: AbortSignal): Promise<BashToolResult> {
    const arguments_ = normalizeBashArguments(input)
    if (signal?.aborted) {
      throw abortError()
    }

    const timeoutController = new AbortController()
    const linkedController = new AbortController()
    let timedOut = false
    const abortLinked = () => linkedController.abort()
    const abortOnTimeout = () => {
      timedOut = true
      timeoutController.abort()
      linkedController.abort()
    }
    const timeout = setTimeout(abortOnTimeout, arguments_.timeout_ms)
    signal?.addEventListener("abort", abortLinked, { once: true })
    timeoutController.signal.addEventListener("abort", abortLinked, { once: true })

    try {
      const shell = await this.getShell(conversationID)
      const result = await shell.exec(arguments_.command, { signal: linkedController.signal })
      if (signal?.aborted) {
        throw abortError()
      }
      if (timedOut) {
        return this.withOutputLimit({ stdout: result.stdout, stderr: result.stderr, exit_code: 124, timed_out: true }, input, arguments_)
      }
      return this.withOutputLimit(
        { stdout: result.stdout, stderr: result.stderr, exit_code: result.exitCode, timed_out: false },
        input,
        arguments_,
      )
    } catch (error) {
      if (signal?.aborted) {
        throw abortError()
      }
      if (timedOut) {
        return this.withOutputLimit({ stdout: "", stderr: "", exit_code: 124, timed_out: true }, input, arguments_)
      }
      return this.withOutputLimit({ stdout: "", stderr: sanitizeError(error), exit_code: 1, timed_out: false }, input, arguments_)
    } finally {
      clearTimeout(timeout)
      signal?.removeEventListener("abort", abortLinked)
      timeoutController.signal.removeEventListener("abort", abortLinked)
    }
  }

  async run(
    conversationID: string,
    messages: ClientMessage[],
    onEvent: (event: AgentEvent) => void,
    signal?: AbortSignal,
  ): Promise<void> {
    let turnMessages = messages
    for (;;) {
      if (signal?.aborted) {
        throw abortError()
      }
      onEvent({ type: "generation_start" })
      const message = await this.client(conversationID, turnMessages, signal, delta => {
        onEvent({ type: "assistant_delta", delta })
      })
      if (signal?.aborted) {
        throw abortError()
      }
      if (message.tool_calls.length === 0) {
        onEvent({ type: "complete", message })
        return
      }

      const calls = message.tool_calls.map(toolCall => {
        if (toolCall.function.name !== "bash") {
          throw new Error(`Unsupported tool: ${toolCall.function.name}`)
        }
        return {
          toolCall,
          arguments: normalizeBashArguments(JSON.parse(toolCall.function.arguments) as BashToolArguments),
        }
      })
      for (const { toolCall, arguments: arguments_ } of calls) {
        onEvent({ type: "tool_start", toolCall, arguments: arguments_ })
      }
      const results: Array<{ toolCall: ToolCall; result: BashToolResult }> = []
      for (const { toolCall, arguments: arguments_ } of calls) {
        this.sensitiveOutputs?.drain()
        const result = await this.executeBash(conversationID, arguments_, signal)
        const sensitiveOutputs = this.sensitiveOutputs?.drain() ?? []
        onEvent({ type: "tool_result", toolCall, arguments: arguments_, result, sensitiveOutputs })
        results.push({ toolCall, result })
      }
      turnMessages = results.map(({ toolCall, result }) => ({
        role: "tool",
        tool_call_id: toolCall.id,
        content: JSON.stringify(result),
      }))
    }
  }

  private async getShell(conversationID: string): Promise<Bash> {
    let shell = this.shells.get(conversationID)
    if (!shell) {
      shell = new Bash({
        customCommands: this.customCommands,
        executionLimitProfile: "hardened",
      })
      this.shells.set(conversationID, shell)
    }
    return shell
  }

  private withOutputLimit(
    result: Omit<BashToolResult, "truncated">,
    input: BashToolArguments,
    arguments_: NormalizedBashArguments,
  ): BashToolResult {
    const requestedOutputLimit = input.max_output_chars
    const maxOutputChars =
      Number.isFinite(requestedOutputLimit) && requestedOutputLimit! > 0
        ? Math.min(BASH_OUTPUT.max, Math.trunc(requestedOutputLimit!))
        : arguments_.max_output_chars
    return { ...result, ...capOutput(result.stdout, result.stderr, maxOutputChars) }
  }
}

import { getToken } from "../auth/keycloak"
import {
  createLocalVisualPreviewConversation,
  getLocalVisualPreviewConversation,
  isLocalVisualPreview,
  listLocalVisualPreviewConversations,
  setLocalVisualPreviewConversationArchived,
  streamLocalVisualPreviewTurn,
} from "../local-visual-preview"

export type ChatRole = "user" | "assistant" | "tool"

export interface ChatToolFunction {
  name: string
  arguments: string
}

export interface ChatToolCall {
  id: string
  type: string
  function: ChatToolFunction
}

export interface ChatMessage {
  id?: string
  role: ChatRole
  content: string
  tool_call_id?: string
  tool_calls?: ChatToolCall[]
  created_at?: string
}

export interface Conversation {
  id: string
  title: string
  messages: ChatMessage[]
  created_at: string
  updated_at: string
  archived_at?: string
}

export interface ConversationSummary {
  id: string
  title: string
  created_at: string
  updated_at: string
  archived_at?: string
}

export class ChatApiError extends Error {
  constructor(
    message: string,
    readonly status?: number,
  ) {
    super(message)
    this.name = "ChatApiError"
  }
}

async function chatFetch(path: string, init: RequestInit = {}): Promise<Response> {
  const token = await getToken()
  return fetch(path, {
    ...init,
    headers: {
      Authorization: token ? `Bearer ${token}` : "",
      "Content-Type": "application/json",
      ...init.headers,
    },
  })
}

async function chatJson<T>(path: string, init: RequestInit = {}): Promise<T> {
  const response = await chatFetch(path, init)
  if (!response.ok) throw await responseError(response)
  return (await response.json()) as T
}

async function responseError(response: Response): Promise<ChatApiError> {
  let message = `Chat request failed (${response.status})`
  try {
    const body = (await response.json()) as { error?: unknown; message?: unknown }
    if (typeof body.error === "string") message = body.error
    else if (typeof body.message === "string") message = body.message
  } catch {
    // Keep the status-derived message when the backend did not return JSON.
  }
  return new ChatApiError(message, response.status)
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function parseToolCalls(value: unknown): ChatToolCall[] {
  if (!Array.isArray(value)) throw new ChatApiError("Chat stream contained invalid tool calls")

  return value.map(toolCall => {
    if (!isRecord(toolCall) || typeof toolCall.id !== "string" || typeof toolCall.type !== "string") {
      throw new ChatApiError("Chat stream contained an invalid tool call")
    }
    if (
      !isRecord(toolCall.function) ||
      typeof toolCall.function.name !== "string" ||
      typeof toolCall.function.arguments !== "string"
    ) {
      throw new ChatApiError("Chat stream contained an invalid tool function")
    }
    return {
      id: toolCall.id,
      type: toolCall.type,
      function: {
        name: toolCall.function.name,
        arguments: toolCall.function.arguments,
      },
    }
  })
}

function parseAssistantMessage(value: unknown): ChatMessage {
  if (!isRecord(value) || value.role !== "assistant" || typeof value.content !== "string") {
    throw new ChatApiError("Chat stream contained an invalid assistant message")
  }
  if (value.id !== undefined && typeof value.id !== "string") {
    throw new ChatApiError("Chat stream contained an invalid assistant message")
  }
  if (value.tool_call_id !== undefined && typeof value.tool_call_id !== "string") {
    throw new ChatApiError("Chat stream contained an invalid assistant message")
  }
  if (value.created_at !== undefined && typeof value.created_at !== "string") {
    throw new ChatApiError("Chat stream contained an invalid assistant message")
  }

  return {
    ...(typeof value.id === "string" ? { id: value.id } : {}),
    role: "assistant",
    content: value.content,
    ...(typeof value.tool_call_id === "string" ? { tool_call_id: value.tool_call_id } : {}),
    ...(value.tool_calls !== undefined ? { tool_calls: parseToolCalls(value.tool_calls) } : {}),
    ...(typeof value.created_at === "string" ? { created_at: value.created_at } : {}),
  }
}

export async function createConversation(): Promise<Conversation> {
  if (isLocalVisualPreview()) return createLocalVisualPreviewConversation()
  return chatJson<Conversation>("/api/chat/conversations", { method: "POST" })
}

export async function listConversations(
  options: { archived?: boolean } = {},
): Promise<ConversationSummary[]> {
  if (isLocalVisualPreview()) return listLocalVisualPreviewConversations(options)
  const query = options.archived ? "?archived=true" : ""
  return chatJson<ConversationSummary[]>(`/api/chat/conversations${query}`)
}

export async function setConversationArchived(
  id: string,
  archived: boolean,
): Promise<Conversation> {
  if (isLocalVisualPreview()) {
    const conversation = setLocalVisualPreviewConversationArchived(id, archived)
    if (!conversation) throw new ChatApiError("Conversation not found", 404)
    return conversation
  }
  return chatJson<Conversation>(`/api/chat/conversations/${encodeURIComponent(id)}`, {
    method: "PATCH",
    body: JSON.stringify({ archived }),
  })
}

export async function getConversation(id: string): Promise<Conversation> {
  if (isLocalVisualPreview()) {
    const conversation = getLocalVisualPreviewConversation(id)
    if (!conversation) throw new ChatApiError("Conversation not found", 404)
    return conversation
  }
  return chatJson<Conversation>(`/api/chat/conversations/${encodeURIComponent(id)}`)
}

export async function streamTurn(
  conversationId: string,
  messages: ChatMessage[],
  onDelta: (delta: string) => void,
  signal?: AbortSignal,
): Promise<ChatMessage> {
  if (isLocalVisualPreview()) {
    try {
      return await streamLocalVisualPreviewTurn(conversationId, messages, onDelta)
    } catch (error) {
      if (error instanceof Error && "status" in error && typeof error.status === "number") {
        throw new ChatApiError(error.message, error.status)
      }
      throw error
    }
  }
  const response = await chatFetch(
    `/api/chat/conversations/${encodeURIComponent(conversationId)}/turns`,
    { method: "POST", body: JSON.stringify({ messages }), signal },
  )
  if (!response.ok) throw await responseError(response)
  if (!response.body) throw new ChatApiError("Chat response did not include a stream")

  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ""
  let assistant: ChatMessage | undefined

  const applyLine = (line: string) => {
    if (!line.trim()) return

    let event: unknown
    try {
      event = JSON.parse(line)
    } catch {
      throw new ChatApiError("Chat stream contained invalid NDJSON")
    }
    if (!isRecord(event)) throw new ChatApiError("Chat stream contained an invalid event")

    if (event.type === "content_delta") {
      if (typeof event.delta !== "string") {
        throw new ChatApiError("Chat stream contained an invalid content delta")
      }
      onDelta(event.delta)
      return
    }

    if (event.type === "assistant") {
      assistant = parseAssistantMessage(event.message)
      return
    }

    throw new ChatApiError("Chat stream contained an unknown event")
  }

  try {
    while (true) {
      const { done, value } = await reader.read()
      buffer += decoder.decode(value, { stream: !done })
      const lines = buffer.split("\n")
      buffer = lines.pop() ?? ""
      for (const line of lines) applyLine(line)
      if (done) break
    }
    if (buffer.trim()) applyLine(buffer)
  } catch (error) {
    if (error instanceof ChatApiError || (error instanceof DOMException && error.name === "AbortError")) throw error
    throw new ChatApiError(error instanceof Error ? error.message : "Chat stream failed")
  }

  if (!assistant) throw new ChatApiError("Chat stream ended without an assistant message")
  return assistant
}

import type { PoolData, PoolSummary } from "./sdk/models"
import type {
  ChatMessage,
  Conversation,
  ConversationSummary,
} from "./sdk/chat"

export function isLocalVisualPreview(): boolean {
  const localPreview =
    import.meta.env.DEV &&
    import.meta.env.VITE_CUA_LOCAL_VISUAL_PREVIEW === "true"
  const pullRequestPreview =
    import.meta.env.VITE_CUA_REVIEW_VISUAL_PREVIEW === "true" &&
    /^cyclops-cs-pr-\d+\.tail204509\.ts\.net$/.test(
      window.location.hostname,
    )
  if (!localPreview && !pullRequestPreview) return false
  return new URLSearchParams(window.location.search).has("cua-visual-preview")
}

export function localVisualPreviewPath(path: string): string {
  if (!isLocalVisualPreview()) return path
  return `${path}${path.includes("?") ? "&" : "?"}cua-visual-preview`
}

export const localVisualPreviewPools: PoolSummary[] = [
  { name: "prod-web-fleet", namespace: "preview", replicas: 120, availableCount: 118, phase: "Ready" },
  { name: "prod-api-fleet", namespace: "preview", replicas: 80, availableCount: 79, phase: "Ready" },
  { name: "staging-web-fleet", namespace: "preview", replicas: 40, availableCount: 38, phase: "Ready" },
  { name: "staging-api-fleet", namespace: "preview", replicas: 30, availableCount: 28, phase: "Scaling" },
  { name: "dev-tools-fleet", namespace: "preview", replicas: 20, availableCount: 20, phase: "Ready" },
  { name: "eval-runner-fleet", namespace: "preview", replicas: 16, availableCount: 14, phase: "Scaling" },
  { name: "batch-jobs-fleet", namespace: "preview", replicas: 64, availableCount: 60, phase: "Degraded" },
  { name: "canary-fleet", namespace: "preview", replicas: 8, availableCount: 8, phase: "Ready" },
  { name: "infra-maint-fleet", namespace: "preview", replicas: 12, availableCount: 10, phase: "Degraded" },
  { name: "data-pipeline-fleet", namespace: "preview", replicas: 24, availableCount: 22, phase: "Ready" },
  { name: "browser-agent-fleet", namespace: "preview", replicas: 32, availableCount: 31, phase: "Ready" },
  { name: "desktop-agent-fleet", namespace: "preview", replicas: 18, availableCount: 17, phase: "Ready" },
]

export async function listLocalVisualPreviewPools(): Promise<PoolSummary[]> {
  const state = new URLSearchParams(window.location.search).get(
    "cua-preview-state",
  )
  if (state === "loading") {
    await new Promise(resolve => window.setTimeout(resolve, 2_000))
  }
  if (state === "empty") return []
  if (state === "error") throw new Error("Synthetic preview error")
  return localVisualPreviewPools
}

export function getLocalVisualPreviewPool(
  namespace: string,
  name: string,
): PoolData | undefined {
  const summary = localVisualPreviewPools.find(
    pool => pool.namespace === namespace && pool.name === name,
  )
  if (!summary) return undefined

  return {
    ...summary,
    cpu: 4,
    ram: "8Gi",
    ociImage: "ghcr.io/trycua/cua:latest",
    firmware: "efi",
    services: [
      { name: "desktop", targetPort: 6901, protocol: "TCP" },
      { name: "ssh", targetPort: 22, protocol: "TCP" },
    ],
    probes: {},
    autoscaling: {
      minPoolSize: Math.max(1, Math.floor(summary.replicas * 0.25)),
      initialPoolSize: summary.replicas,
      maxPoolSize: Math.ceil(summary.replicas * 1.5),
    },
    totalCount: summary.replicas,
    claimedCount: Math.max(summary.replicas - summary.availableCount, 0),
  }
}

const localVisualPreviewTimestamp = "2026-08-16T20:20:00.000Z"

const localVisualPreviewConversations: Conversation[] = [
  {
    id: "preview-browser-task",
    title: "Inspect the browser fleet",
    created_at: localVisualPreviewTimestamp,
    updated_at: localVisualPreviewTimestamp,
    messages: [
      {
        id: "preview-message-1",
        role: "user",
        content: "Check the browser fleet and summarize its availability.",
        created_at: localVisualPreviewTimestamp,
      },
      {
        id: "preview-message-2",
        role: "assistant",
        content: "",
        created_at: localVisualPreviewTimestamp,
        tool_calls: [
          {
            id: "preview-tool-1",
            type: "function",
            function: {
              name: "bash",
              arguments: JSON.stringify({
                command: "cua pools list --selector fleet=browser",
              }),
            },
          },
        ],
      },
      {
        id: "preview-message-3",
        role: "tool",
        tool_call_id: "preview-tool-1",
        content: JSON.stringify({
          stdout: "browser-agent-fleet  31/32 available  Ready",
          stderr: "",
          exit_code: 0,
          timed_out: false,
          truncated: false,
        }),
        created_at: localVisualPreviewTimestamp,
      },
      {
        id: "preview-message-4",
        role: "assistant",
        content:
          "The browser fleet is healthy: **31 of 32** workspaces are available. One workspace is currently being replenished.",
        created_at: localVisualPreviewTimestamp,
      },
    ],
  },
  {
    id: "preview-pool-task",
    title: "Create a staging pool",
    created_at: "2026-08-15T18:10:00.000Z",
    updated_at: "2026-08-15T18:10:00.000Z",
    messages: [],
  },
]

export async function listLocalVisualPreviewConversations(): Promise<
  ConversationSummary[]
> {
  const state = new URLSearchParams(window.location.search).get(
    "cua-preview-state",
  )
  if (state === "loading") {
    await new Promise(resolve => window.setTimeout(resolve, 2_000))
  }
  if (state === "empty") return []
  return localVisualPreviewConversations.map(({ messages: _messages, ...summary }) => ({
    ...summary,
  }))
}

export function getLocalVisualPreviewConversation(
  id: string,
): Conversation | undefined {
  return localVisualPreviewConversations.find(conversation => conversation.id === id)
}

export function createLocalVisualPreviewConversation(): Conversation {
  const timestamp = new Date().toISOString()
  const conversation: Conversation = {
    id: `preview-conversation-${localVisualPreviewConversations.length + 1}`,
    title: "New conversation",
    created_at: timestamp,
    updated_at: timestamp,
    messages: [],
  }
  localVisualPreviewConversations.unshift(conversation)
  return conversation
}

export async function streamLocalVisualPreviewTurn(
  conversationId: string,
  messages: ChatMessage[],
  onDelta: (delta: string) => void,
): Promise<ChatMessage> {
  const conversation = getLocalVisualPreviewConversation(conversationId)
  if (!conversation) throw new Error("Preview conversation not found")

  const timestamp = new Date().toISOString()
  const content =
    "Preview mode is connected. In a signed-in environment, Cua would run this request against your available tools."
  for (const message of messages) {
    conversation.messages.push({
      ...message,
      id: message.id ?? `preview-message-${conversation.messages.length + 1}`,
      created_at: message.created_at ?? timestamp,
    })
  }
  onDelta(content)
  const assistant: ChatMessage = {
    id: `preview-message-${conversation.messages.length + 1}`,
    role: "assistant",
    content,
    created_at: timestamp,
  }
  conversation.messages.push(assistant)
  conversation.updated_at = timestamp
  return assistant
}

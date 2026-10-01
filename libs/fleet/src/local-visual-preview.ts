import type { PoolData, PoolSummary } from "./fleet/models"
import { healthyPoolDisplayStatus } from "../sdk-bindings/ts-uniffi-browser/ts/index.web"
import type {
  BillingUsage,
  UsagePoint,
  UsageRangeMonths,
} from "./api/billing"
import type {
  ChatMessage,
  Conversation,
  ConversationSummary,
} from "./api/chat"
import { isReviewPreviewEnvironment } from "./preview-environment"

export function isLocalVisualPreview(): boolean {
  const localPreview =
    import.meta.env.DEV &&
    import.meta.env.VITE_CUA_LOCAL_VISUAL_PREVIEW === "true"
  const pullRequestPreview = isReviewPreviewEnvironment()
  if (!localPreview && !pullRequestPreview) return false
  return new URLSearchParams(window.location.search).has("cua-visual-preview")
}

export function localVisualPreviewPath(path: string): string {
  if (!isLocalVisualPreview()) return path
  return `${path}${path.includes("?") ? "&" : "?"}cua-visual-preview`
}

function localVisualPreviewPools(): PoolSummary[] {
  const status = healthyPoolDisplayStatus()
  return [
    { name: "prod-web-fleet", namespace: "preview", replicas: 120, availableCount: 118, status },
    { name: "prod-api-fleet", namespace: "preview", replicas: 80, availableCount: 79, status },
    { name: "staging-web-fleet", namespace: "preview", replicas: 40, availableCount: 38, status },
    { name: "staging-api-fleet", namespace: "preview", replicas: 30, availableCount: 28, status },
    { name: "dev-tools-fleet", namespace: "preview", replicas: 20, availableCount: 20, status },
    { name: "eval-runner-fleet", namespace: "preview", replicas: 16, availableCount: 14, status },
    { name: "batch-jobs-fleet", namespace: "preview", replicas: 64, availableCount: 60, status },
    { name: "canary-fleet", namespace: "preview", replicas: 8, availableCount: 8, status },
    { name: "infra-maint-fleet", namespace: "preview", replicas: 12, availableCount: 10, status },
    { name: "data-pipeline-fleet", namespace: "preview", replicas: 24, availableCount: 22, status },
    { name: "browser-agent-fleet", namespace: "preview", replicas: 32, availableCount: 31, status },
    { name: "desktop-agent-fleet", namespace: "preview", replicas: 18, availableCount: 17, status },
  ]
}

export async function listLocalVisualPreviewPools(): Promise<PoolSummary[]> {
  const state = new URLSearchParams(window.location.search).get(
    "cua-preview-state",
  )
  if (state === "loading") {
    await new Promise(resolve => window.setTimeout(resolve, 2_000))
  }
  if (state === "empty") return []
  if (state === "error") throw new Error("Synthetic preview error")
  return localVisualPreviewPools()
}

export function getLocalVisualPreviewPool(
  namespace: string,
  name: string,
): PoolData | undefined {
  const summary = localVisualPreviewPools().find(
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
  {
    id: "preview-archived-task",
    title: "Archived browser investigation",
    created_at: "2026-08-14T16:00:00.000Z",
    updated_at: "2026-08-14T17:00:00.000Z",
    archived_at: "2026-08-14T17:00:00.000Z",
    messages: [],
  },
]

export async function listLocalVisualPreviewConversations(
  { archived = false }: { archived?: boolean } = {},
): Promise<ConversationSummary[]> {
  const state = new URLSearchParams(window.location.search).get(
    "cua-preview-state",
  )
  if (state === "loading") {
    await new Promise(resolve => window.setTimeout(resolve, 2_000))
  }
  if (state === "empty") return []
  return localVisualPreviewConversations
    .filter(conversation => Boolean(conversation.archived_at) === archived)
    .map(({ messages: _messages, ...summary }) => ({ ...summary }))
}

export function getLocalVisualPreviewConversation(
  id: string,
): Conversation | undefined {
  return localVisualPreviewConversations.find(conversation => conversation.id === id)
}

export function setLocalVisualPreviewConversationArchived(
  id: string,
  archived: boolean,
): Conversation | undefined {
  const conversation = getLocalVisualPreviewConversation(id)
  if (!conversation) return undefined

  const timestamp = new Date().toISOString()
  conversation.updated_at = timestamp
  if (archived) conversation.archived_at = timestamp
  else delete conversation.archived_at
  return conversation
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
  if (conversation.archived_at) {
    const error = new Error("Conversation is archived") as Error & { status: number }
    error.status = 409
    throw error
  }

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


export async function getLocalVisualPreviewBillingUsage(
  months: UsageRangeMonths,
): Promise<BillingUsage> {
  const state = new URLSearchParams(window.location.search).get(
    "cua-preview-state",
  )
  if (state === "error") throw new Error("Preview usage is unavailable")
  if (state === "loading") {
    await new Promise(resolve => window.setTimeout(resolve, 2_000))
  }
  if (state === "empty") {
    return {
      currency: "usd",
      range_start: "2026-02-01T00:00:00Z",
      range_end: "2026-08-22T00:00:00Z",
      current_period_start: null,
      current_period_end: null,
      current_estimate: 0,
      previous_period_amount: 0,
      trend: [],
      breakdown: [],
    }
  }

  const allTrend: UsagePoint[] = [
    { period_start: "2026-03-01T00:00:00Z", period_end: "2026-04-01T00:00:00Z", amount: 18420, estimate: false },
    { period_start: "2026-04-01T00:00:00Z", period_end: "2026-05-01T00:00:00Z", amount: 22780, estimate: false },
    { period_start: "2026-05-01T00:00:00Z", period_end: "2026-06-01T00:00:00Z", amount: 21410, estimate: false },
    { period_start: "2026-06-01T00:00:00Z", period_end: "2026-07-01T00:00:00Z", amount: 28640, estimate: false },
    { period_start: "2026-07-01T00:00:00Z", period_end: "2026-08-01T00:00:00Z", amount: 31920, estimate: false },
    { period_start: "2026-08-01T00:00:00Z", period_end: "2026-09-01T00:00:00Z", amount: 24680, estimate: true },
  ]
  return {
    currency: "usd",
    range_start: "2026-02-22T00:00:00Z",
    range_end: "2026-08-22T00:00:00Z",
    current_period_start: "2026-08-01T00:00:00Z",
    current_period_end: "2026-09-01T00:00:00Z",
    current_estimate: 24680,
    previous_period_amount: 31920,
    trend: allTrend.slice(-months),
    breakdown: [
      { name: "Linux desktop runtime", amount: 13240, quantity: 368, period_start: "2026-08-01T00:00:00Z", period_end: "2026-09-01T00:00:00Z" },
      { name: "Windows desktop runtime", amount: 6860, quantity: 98, period_start: "2026-08-01T00:00:00Z", period_end: "2026-09-01T00:00:00Z" },
      { name: "Android emulator runtime", amount: 3890, quantity: 81, period_start: "2026-08-01T00:00:00Z", period_end: "2026-09-01T00:00:00Z" },
      { name: "Persistent storage", amount: 690, quantity: 230, period_start: "2026-08-01T00:00:00Z", period_end: "2026-09-01T00:00:00Z" },
    ],
  }
}

import { deriveWarmPoolPhase } from "../src/sdk/status.js"
import type {
  Conversation,
  ConversationSummary,
  listConversations,
  setConversationArchived,
} from "../src/sdk/chat.js"

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (actual !== expected) {
    throw new Error(`${message}: expected ${String(expected)}, got ${String(actual)}`)
  }
}

assertEqual(
  deriveWarmPoolPhase(0, { replicas: 0, readyReplicas: 0 }),
  "Ready",
  "a reconciled zero-sized pool is healthy",
)
assertEqual(
  deriveWarmPoolPhase(0, undefined),
  "Provisioning",
  "a pool without observed status is still provisioning",
)
assertEqual(
  deriveWarmPoolPhase(2, { replicas: 2, readyReplicas: 1 }),
  "Provisioning",
  "a partially ready pool is provisioning",
)
assertEqual(
  deriveWarmPoolPhase(2, { replicas: 2, readyReplicas: 2 }),
  "Ready",
  "a fully ready pool is healthy",
)

const archivedAt = "2026-08-16T20:20:00.000Z"
const archivedConversation: ConversationSummary = {
  id: "archived-conversation",
  title: "Archived conversation",
  created_at: "2026-08-15T20:20:00.000Z",
  updated_at: archivedAt,
  archived_at: archivedAt,
}

type Assert<T extends true> = T
type Equal<Actual, Expected> = (
  <Value>() => Value extends Actual ? 1 : 2
) extends (<Value>() => Value extends Expected ? 1 : 2)
  ? (<Value>() => Value extends Expected ? 1 : 2) extends (
      <Value>() => Value extends Actual ? 1 : 2
    )
    ? true
    : false
  : false
type ListConversationsParameters = Assert<
  Equal<Parameters<typeof listConversations>, [options?: { archived?: boolean }]>
>
type ListConversationsReturn = Assert<
  Equal<ReturnType<typeof listConversations>, Promise<ConversationSummary[]>>
>
type SetConversationArchivedParameters = Assert<
  Equal<Parameters<typeof setConversationArchived>, [id: string, archived: boolean]>
>
type SetConversationArchivedReturn = Assert<
  Equal<ReturnType<typeof setConversationArchived>, Promise<Conversation>>
>

assertEqual(
  archivedConversation.archived_at,
  archivedAt,
  "an archived conversation summary exposes its archive timestamp",
)

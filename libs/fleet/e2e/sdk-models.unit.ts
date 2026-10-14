import type {
  Conversation,
  ConversationSummary,
  listConversations,
  setConversationArchived,
} from "../src/api/chat.js"

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (actual !== expected) {
    throw new Error(`${message}: expected ${String(expected)}, got ${String(actual)}`)
  }
}

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

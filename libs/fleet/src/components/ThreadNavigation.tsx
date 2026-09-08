import Button from "@cloudscape-design/components/button"
import ButtonDropdown from "@cloudscape-design/components/button-dropdown"
import type { SideNavigationProps } from "@cloudscape-design/components/side-navigation"
import { displayThreadTitle, useChatThreads } from "../chat/ChatThreadsContext"
import { groupConversations } from "../pages/AgentChat"

export function isThreadNavigationHref(href: string): boolean {
  return href === "#/agent/new" || href.startsWith("#/agent/")
}

export function useThreadNavigation(): {
  items: SideNavigationProps.Item[]
  threadTitles: Array<{ href: string; title: string }>
  generationLocked: boolean
  createThread: () => Promise<void>
  isCreatePending: () => boolean
} {
  const {
    activeThreads,
    activeThreadsError,
    archiveThread,
    createThread,
    generationLocked,
    threadMutationLocked,
    isCreatePending,
    refreshActiveThreads,
  } = useChatThreads()
  const disabled = generationLocked || threadMutationLocked || isCreatePending()
  const groups = groupConversations(new Date(), activeThreads)
  const threadTitles = activeThreads.map(thread => ({
    href: `#/agent/${encodeURIComponent(thread.id)}`,
    title: displayThreadTitle(thread.title),
  }))
  const retryControl = activeThreadsError ? (
    <Button ariaLabel="Retry thread list" onClick={() => void refreshActiveThreads()} variant="inline-link">
      Retry
    </Button>
  ) : undefined

  const items: SideNavigationProps.Item[] = [
    {
      type: "section",
      text: "Threads",
      defaultExpanded: true,
      items: [
        {
          type: "link",
          // Keep this label stable regardless of list-loading state — it
          // used to be a separate icon button with a fixed ariaLabel
          // ("New thread"), independent of the outer link's loading-text
          // swap; the button was folded into this single link, so the
          // text itself now has to stay stable for the same reason (you
          // can create a thread while the list is still loading).
          text: "New thread",
          href: "#/agent/new",
        },
        {
          type: "link",
          text: "Archived threads",
          href: "#/agent/archived",
        },
        ...(activeThreadsError
          ? [{ type: "link" as const, text: "Unable to load threads", href: "#/agent/retry", info: retryControl }]
          : []),
        ...groups.flatMap(group =>
          group.conversations.map(thread => {
            const title = displayThreadTitle(thread.title)
            return {
              type: "link" as const,
              text: title,
              href: `#/agent/${encodeURIComponent(thread.id)}`,
              info: (
                <ButtonDropdown
                  ariaLabel={`Actions for ${title}`}
                  disabled={disabled}
                  expandToViewport
                  items={[{ id: "archive", text: "Archive" }]}
                  onItemClick={({ detail }) => {
                    if (detail.id === "archive") void archiveThread(thread.id)
                  }}
                  variant="icon"
                />
              ),
            }
          }),
        ),
      ],
    },
  ]

  return {
    items,
    threadTitles,
    generationLocked,
    createThread: async () => {
      await createThread()
    },
    isCreatePending,
  }
}

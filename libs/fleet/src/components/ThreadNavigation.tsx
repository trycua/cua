import Button from "@cloudscape-design/components/button"
import ButtonDropdown from "@cloudscape-design/components/button-dropdown"
import type { SideNavigationProps } from "@cloudscape-design/components/side-navigation"
import { useLocation } from "react-router-dom"
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
  const location = useLocation()
  const {
    activeThreads,
    activeThreadsError,
    activeThreadsLoading,
    archiveThread,
    createThread,
    creatingThread,
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
      defaultExpanded: location.pathname.startsWith("/agent"),
      items: [
        {
          type: "link",
          text: activeThreadsLoading ? "Loading threads" : "New thread",
          href: "#/agent/new",
          info: (
            <Button
              ariaLabel="New thread"
              disabled={disabled}
              iconName="add-plus"
              loading={creatingThread}
              loadingText="Creating thread"
              onClick={() => void createThread()}
              variant="inline-icon"
            />
          ),
        },
        ...(activeThreadsError
          ? [{ type: "link" as const, text: "Unable to load threads", href: "#/agent/retry", info: retryControl }]
          : []),
        ...groups.map(group => ({
          type: "section" as const,
          text: group.label,
          defaultExpanded: true,
          items: group.conversations.map(thread => {
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
          }}),
        })),
        {
          type: "link",
          text: "Archived threads",
          href: "#/agent/archived",
        },
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

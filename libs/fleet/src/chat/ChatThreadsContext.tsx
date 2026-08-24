import Button from "@cloudscape-design/components/button"
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { useLocation, useNavigate } from "react-router-dom"
import { useFeatureFlags } from "../components/FeatureFlagContext"
import { useFlash } from "../components/FlashContext"
import { localVisualPreviewPath } from "../local-visual-preview"
import {
  createConversation,
  listConversations,
  setConversationArchived,
  type Conversation,
  type ConversationSummary,
} from "../api/chat"

interface ChatThreadsContextValue {
  activeThreads: ConversationSummary[]
  activeThreadsLoading: boolean
  activeThreadsError?: string
  creatingThread: boolean
  generationLocked: boolean
  threadMutationLocked: boolean
  refreshActiveThreads: () => Promise<ConversationSummary[]>
  createThread: () => Promise<Conversation | undefined>
  archiveThread: (threadId: string) => Promise<boolean>
  restoreThread: (threadId: string) => Promise<Conversation | undefined>
  recoverToActiveThread: () => Promise<void>
  setGenerationLocked: (locked: boolean) => void
  isCreatePending: () => boolean
  isThreadMutationPending: () => boolean
}

interface ProviderLifecycle {
  mounted: boolean
  enabled: boolean
  generation: number
  listSequence: number
}

const unavailable = async () => [] as ConversationSummary[]
const ChatThreadsContext = createContext<ChatThreadsContextValue>({
  activeThreads: [],
  activeThreadsLoading: false,
  creatingThread: false,
  generationLocked: false,
  threadMutationLocked: false,
  refreshActiveThreads: unavailable,
  createThread: async () => undefined,
  archiveThread: async () => false,
  restoreThread: async () => undefined,
  recoverToActiveThread: async () => undefined,
  setGenerationLocked: () => undefined,
  isCreatePending: () => false,
  isThreadMutationPending: () => false,
})

export function displayThreadTitle(title: string): string {
  return title.trim() || "New conversation"
}

function sortActiveThreads(threads: ConversationSummary[]): ConversationSummary[] {
  return [...threads].sort(
    (left, right) => Date.parse(right.updated_at) - Date.parse(left.updated_at),
  )
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback
}

function UndoThreadAction({ threadId }: { threadId: string }) {
  const { generationLocked, isThreadMutationPending, restoreThread } = useChatThreads()
  const disabled = generationLocked || isThreadMutationPending()
  return (
    <Button disabled={disabled} onClick={() => void restoreThread(threadId)}>
      Undo
    </Button>
  )
}

export function ChatThreadsProvider({ children }: { children: ReactNode }) {
  const { chat, resolved } = useFeatureFlags()
  const { push } = useFlash()
  const location = useLocation()
  const navigate = useNavigate()
  const enabled = resolved && chat
  const [activeThreads, setActiveThreads] = useState<ConversationSummary[]>([])
  const [activeThreadsLoading, setActiveThreadsLoading] = useState(true)
  const [activeThreadsLoaded, setActiveThreadsLoaded] = useState(false)
  const [activeThreadsLoadVersion, setActiveThreadsLoadVersion] = useState(0)
  const [activeThreadsError, setActiveThreadsError] = useState<string>()
  const [creatingThread, setCreatingThread] = useState(false)
  const [generationLocked, setGenerationLockedState] = useState(false)
  const [threadMutationLocked, setThreadMutationLocked] = useState(false)
  const activeThreadsRef = useRef<ConversationSummary[]>([])
  const nextThreadMutationTokenRef = useRef(0)
  const activeThreadMutationTokenRef = useRef<number>()
  const generationLockedRef = useRef(false)
  const lifecycleRef = useRef<ProviderLifecycle>({
    mounted: false,
    enabled: false,
    generation: 0,
    listSequence: 0,
  })
  const fallbackStartedForLoad = useRef<number>()
  const pathnameRef = useRef(location.pathname)
  pathnameRef.current = location.pathname

  const threadPath = useCallback((threadId: string) => (
    localVisualPreviewPath(`/agent/${encodeURIComponent(threadId)}`)
  ), [])

  const isCurrentGeneration = useCallback((generation: number) => {
    const lifecycle = lifecycleRef.current
    return lifecycle.mounted && lifecycle.enabled && lifecycle.generation === generation
  }, [])

  const setCurrentThreads = useCallback((next: ConversationSummary[] | ((current: ConversationSummary[]) => ConversationSummary[])) => {
    const resolvedThreads = typeof next === "function"
      ? next(activeThreadsRef.current)
      : next
    activeThreadsRef.current = resolvedThreads
    setActiveThreads(resolvedThreads)
    return resolvedThreads
  }, [])

  const invalidateListRequests = useCallback(() => {
    lifecycleRef.current.listSequence += 1
    setActiveThreadsLoading(false)
  }, [])

  const refreshActiveThreads = useCallback(async () => {
    const lifecycle = lifecycleRef.current
    if (!lifecycle.mounted || !lifecycle.enabled) return []
    const generation = lifecycle.generation
    const listSequence = ++lifecycle.listSequence
    setActiveThreadsLoading(true)
    setActiveThreadsLoaded(false)
    setActiveThreadsError(undefined)

    try {
      const threads = sortActiveThreads(await listConversations())
      if (!isCurrentGeneration(generation) || lifecycleRef.current.listSequence !== listSequence) return []
      setCurrentThreads(threads)
      setActiveThreadsLoaded(true)
      setActiveThreadsLoadVersion(current => current + 1)
      return threads
    } catch (error) {
      if (!isCurrentGeneration(generation) || lifecycleRef.current.listSequence !== listSequence) return []
      setActiveThreadsError(errorMessage(error, "Unable to load threads"))
      throw error
    } finally {
      if (isCurrentGeneration(generation) && lifecycleRef.current.listSequence === listSequence) {
        setActiveThreadsLoading(false)
      }
    }
  }, [isCurrentGeneration, setCurrentThreads])

  useEffect(() => {
    const lifecycle = lifecycleRef.current
    lifecycle.mounted = true
    return () => {
      lifecycle.mounted = false
      lifecycle.enabled = false
      lifecycle.generation += 1
      lifecycle.listSequence += 1
      activeThreadMutationTokenRef.current = undefined
    }
  }, [])

  useEffect(() => {
    const lifecycle = lifecycleRef.current
    lifecycle.generation += 1
    lifecycle.listSequence += 1
    lifecycle.enabled = enabled
    fallbackStartedForLoad.current = undefined

    if (!enabled) {
      activeThreadMutationTokenRef.current = undefined
      generationLockedRef.current = false
      setCreatingThread(false)
      setGenerationLockedState(false)
      setThreadMutationLocked(false)
      setCurrentThreads([])
      setActiveThreadsError(undefined)
      setActiveThreadsLoaded(false)
      setActiveThreadsLoading(false)
      return
    }

    void refreshActiveThreads().catch(() => undefined)
  }, [enabled, refreshActiveThreads, setCurrentThreads])

  const isThreadMutationPending = useCallback(() => (
    activeThreadMutationTokenRef.current !== undefined
  ), [])

  const ownsThreadMutation = useCallback((token: number) => (
    activeThreadMutationTokenRef.current === token
  ), [])

  const beginThreadMutation = useCallback((generation: number): number | undefined => {
    if (
      !isCurrentGeneration(generation) ||
      generationLockedRef.current ||
      activeThreadMutationTokenRef.current !== undefined
    )
      return undefined
    const token = ++nextThreadMutationTokenRef.current
    activeThreadMutationTokenRef.current = token
    setThreadMutationLocked(true)
    return token
  }, [isCurrentGeneration])

  const finishThreadMutation = useCallback((token: number) => {
    if (!ownsThreadMutation(token)) return
    activeThreadMutationTokenRef.current = undefined
    if (lifecycleRef.current.mounted) setThreadMutationLocked(false)
  }, [ownsThreadMutation])

  const setGenerationLocked = useCallback((locked: boolean) => {
    if (locked && isThreadMutationPending()) return
    generationLockedRef.current = locked
    if (lifecycleRef.current.mounted) setGenerationLockedState(locked)
  }, [isThreadMutationPending])

  const isCreatePending = isThreadMutationPending

  const createThreadWithMutation = useCallback(async (
    generation: number,
    mutationToken: number,
    navigationPath: string,
  ) => {
    invalidateListRequests()
    setCreatingThread(true)

    const ownsCreate = () => (
      isCurrentGeneration(generation) && ownsThreadMutation(mutationToken)
    )

    try {
      const conversation = await createConversation()
      if (!ownsCreate()) return undefined
      const summary: ConversationSummary = {
        id: conversation.id,
        title: conversation.title,
        created_at: conversation.created_at,
        updated_at: conversation.updated_at,
        ...(conversation.archived_at ? { archived_at: conversation.archived_at } : {}),
      }
      setCurrentThreads(current => sortActiveThreads([summary, ...current]))
      setActiveThreadsLoaded(true)
      if (pathnameRef.current === navigationPath) navigate(threadPath(conversation.id))
      return conversation
    } catch (error) {
      if (ownsCreate()) {
        push({
          type: "error",
          header: "Unable to create thread",
          content: errorMessage(error, "Please try again."),
        })
      }
      return undefined
    } finally {
      if (ownsCreate()) setCreatingThread(false)
    }
  }, [
    invalidateListRequests,
    isCurrentGeneration,
    navigate,
    ownsThreadMutation,
    push,
    setCurrentThreads,
    threadPath,
  ])

  const createThread = useCallback(async () => {
    const generation = lifecycleRef.current.generation
    const mutationToken = beginThreadMutation(generation)
    if (mutationToken === undefined) return undefined

    try {
      return await createThreadWithMutation(generation, mutationToken, pathnameRef.current)
    } finally {
      finishThreadMutation(mutationToken)
    }
  }, [beginThreadMutation, createThreadWithMutation, finishThreadMutation])

  useEffect(() => {
    if (!enabled || !activeThreadsLoaded || activeThreadsError || activeThreadsLoading) return
    if (location.pathname !== "/agent") return
    if (activeThreads.length > 0) {
      navigate(threadPath(activeThreads[0].id), { replace: true })
      return
    }
    if (fallbackStartedForLoad.current === activeThreadsLoadVersion) return
    fallbackStartedForLoad.current = activeThreadsLoadVersion
    void createThread()
  }, [
    activeThreads,
    activeThreadsError,
    activeThreadsLoaded,
    activeThreadsLoading,
    activeThreadsLoadVersion,
    createThread,
    enabled,
    location.pathname,
    navigate,
    threadPath,
  ])

  const restoreThread = useCallback(async (threadId: string) => {
    const generation = lifecycleRef.current.generation
    const mutationToken = beginThreadMutation(generation)
    if (mutationToken === undefined) return undefined
    invalidateListRequests()

    const ownsRestore = () => (
      isCurrentGeneration(generation) && ownsThreadMutation(mutationToken)
    )

    let conversation: Conversation
    try {
      conversation = await setConversationArchived(threadId, false)
    } catch (error) {
      if (ownsRestore()) {
        push({
          type: "error",
          header: "Unable to restore thread",
          content: errorMessage(error, "Please try again."),
        })
      }
      return undefined
    }

    try {
      if (!ownsRestore()) return undefined
      const summary: ConversationSummary = {
        id: conversation.id,
        title: conversation.title,
        created_at: conversation.created_at,
        updated_at: conversation.updated_at,
      }
      setCurrentThreads(current => sortActiveThreads([
        summary,
        ...current.filter(thread => thread.id !== threadId),
      ]))
      setActiveThreadsLoaded(true)
      await refreshActiveThreads().catch(() => undefined)
      if (!ownsRestore()) return undefined
      return conversation
    } finally {
      finishThreadMutation(mutationToken)
    }
  }, [
    beginThreadMutation,
    finishThreadMutation,
    invalidateListRequests,
    isCurrentGeneration,
    ownsThreadMutation,
    push,
    refreshActiveThreads,
    setCurrentThreads,
  ])

  const archiveThread = useCallback(async (threadId: string) => {
    const generation = lifecycleRef.current.generation
    const mutationToken = beginThreadMutation(generation)
    if (mutationToken === undefined) return false
    invalidateListRequests()

    const ownsArchive = () => (
      isCurrentGeneration(generation) && ownsThreadMutation(mutationToken)
    )

    try {
      await setConversationArchived(threadId, true)
      if (!ownsArchive()) return false
      const remaining = setCurrentThreads(current => current.filter(thread => thread.id !== threadId))
      push({
        type: "success",
        header: "Thread archived",
        action: <UndoThreadAction threadId={threadId} />,
      })

      if (pathnameRef.current !== `/agent/${encodeURIComponent(threadId)}`) return true
      if (remaining.length > 0) {
        navigate(threadPath(remaining[0].id), { replace: true })
        return true
      }
      const archivedPath = "/agent/archived"
      navigate(localVisualPreviewPath(archivedPath), { replace: true })
      await createThreadWithMutation(generation, mutationToken, archivedPath)
      return true
    } catch (error) {
      if (ownsArchive()) {
        push({
          type: "error",
          header: "Unable to archive thread",
          content: errorMessage(error, "Please try again."),
        })
      }
      return false
    } finally {
      finishThreadMutation(mutationToken)
    }
  }, [
    beginThreadMutation,
    createThreadWithMutation,
    finishThreadMutation,
    invalidateListRequests,
    isCurrentGeneration,
    navigate,
    ownsThreadMutation,
    push,
    setCurrentThreads,
    threadPath,
  ])

  const recoverToActiveThread = useCallback(async () => {
    const generation = lifecycleRef.current.generation
    if (
      !isCurrentGeneration(generation) ||
      generationLockedRef.current ||
      isThreadMutationPending()
    )
      return

    try {
      const threads = await refreshActiveThreads()
      if (
        !isCurrentGeneration(generation) ||
        generationLockedRef.current ||
        isThreadMutationPending()
      )
        return
      if (threads.length > 0) {
        navigate(threadPath(threads[0].id), { replace: true })
        return
      }
      await createThread()
    } catch {
      // The shared thread list retains and announces its load failure.
    }
  }, [createThread, isCurrentGeneration, isThreadMutationPending, navigate, refreshActiveThreads, threadPath])

  const value = useMemo<ChatThreadsContextValue>(() => ({
    activeThreads,
    activeThreadsLoading,
    activeThreadsError,
    creatingThread,
    generationLocked,
    threadMutationLocked,
    refreshActiveThreads,
    createThread,
    archiveThread,
    restoreThread,
    recoverToActiveThread,
    setGenerationLocked,
    isCreatePending,
    isThreadMutationPending,
  }), [
    activeThreads,
    activeThreadsError,
    activeThreadsLoading,
    archiveThread,
    createThread,
    creatingThread,
    generationLocked,
    threadMutationLocked,
    refreshActiveThreads,
    restoreThread,
    recoverToActiveThread,
    setGenerationLocked,
    isCreatePending,
    isThreadMutationPending,
  ])

  return (
    <ChatThreadsContext.Provider value={value}>
      {children}
    </ChatThreadsContext.Provider>
  )
}

export function useChatThreads(): ChatThreadsContextValue {
  return useContext(ChatThreadsContext)
}

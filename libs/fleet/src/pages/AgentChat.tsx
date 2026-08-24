import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import { createPortal } from "react-dom"
import Avatar from "@cloudscape-design/chat-components/avatar"
import ChatBubble from "@cloudscape-design/chat-components/chat-bubble"
import Alert from "@cloudscape-design/components/alert"
import Box from "@cloudscape-design/components/box"
import Button, { type ButtonProps } from "@cloudscape-design/components/button"
import Drawer from "@cloudscape-design/components/drawer"
import ExpandableSection from "@cloudscape-design/components/expandable-section"
import Header from "@cloudscape-design/components/header"
import List from "@cloudscape-design/components/list"
import LiveRegion from "@cloudscape-design/components/live-region"
import PromptInput from "@cloudscape-design/components/prompt-input"
import SpaceBetween from "@cloudscape-design/components/space-between"
import StatusIndicator from "@cloudscape-design/components/status-indicator"
import { MarkdownMessage } from "../components/MarkdownMessage"
import { PageShell } from "../components/PageShell"
import { createClaim, deleteClaim, getClaim, listClaims } from "../sdk/claims"
import { createPool, deletePool, getPool, listNamespaces, listPools, updatePoolServices } from "../sdk/pools"
import { createUserKey, deleteUserKey, listUserKeys } from "../sdk/userKeys"
import {
  createConversation,
  getConversation,
  listConversations,
  streamTurn,
  type ChatMessage,
  type ChatToolCall,
  type Conversation,
  type ConversationSummary,
} from "../sdk/chat"
import {
  BrowserBashAgent,
  type BashToolResult,
  type ClientMessage,
  type NormalizedBashArguments,
  type ToolCall,
} from "../browser-agent"
import { createBrowserMcpCommands } from "../browser-mcp-commands"
import { createBrowserSdkCommands, type BrowserSdk } from "../browser-sdk-commands"
import "./AgentChat.css"

export interface ConversationGroup {
  label: string
  conversations: ConversationSummary[]
}

interface CommandStep {
  id: string
  arguments: NormalizedBashArguments
  result?: BashToolResult
  running?: boolean
}

type TranscriptItem = { type: "message"; message: ChatMessage } | { type: "command"; command: CommandStep }

export function groupConversations(now: Date, summaries: ConversationSummary[]): ConversationGroup[] {
  const startOfToday = new Date(now)
  startOfToday.setHours(0, 0, 0, 0)
  const groups = new Map<string, ConversationSummary[]>()

  for (const summary of summaries) {
    const updatedAt = new Date(summary.updated_at)
    const startOfDate = new Date(updatedAt)
    startOfDate.setHours(0, 0, 0, 0)
    const daysAgo = Math.round((startOfToday.getTime() - startOfDate.getTime()) / 86_400_000)
    const label =
      daysAgo === 0
        ? "Today"
        : daysAgo === 1
          ? "Yesterday"
          : daysAgo >= 2 && daysAgo <= 7
            ? "Past 7 days"
            : updatedAt.toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" })
    const group = groups.get(label) ?? []
    group.push(summary)
    groups.set(label, group)
  }

  return [...groups].map(([label, conversations]) => ({ label, conversations }))
}

const MOBILE_HISTORY_QUERY = "(max-width: 700px)"
const GENERATION_LOADING_DELAY_MS = 300
const browserSdk: BrowserSdk = {
  listNamespaces,
  listPools,
  getPool,
  createPool,
  updatePoolServices,
  deletePool,
  listClaims,
  createClaim,
  getClaim,
  deleteClaim,
  listUserKeys,
  createUserKey,
  deleteUserKey,
}

function useMobileHistory(): boolean {
  const [mobile, setMobile] = useState(() => window.matchMedia(MOBILE_HISTORY_QUERY).matches)

  useEffect(() => {
    const mediaQuery = window.matchMedia(MOBILE_HISTORY_QUERY)
    const update = () => setMobile(mediaQuery.matches)
    update()
    mediaQuery.addEventListener("change", update)
    return () => mediaQuery.removeEventListener("change", update)
  }, [])

  return mobile
}

function Skeleton({ kind }: { kind: "conversation" | "message" }) {
  return <div aria-hidden="true" className={`agent-chat-skeleton agent-chat-skeleton--${kind}`} data-testid={`${kind}-skeleton`} />
}

function formatTime(timestamp?: string): { full: string; local: string } {
  if (!timestamp) return { full: "Unknown time", local: "Unknown time" }
  const date = new Date(timestamp)
  return { full: date.toLocaleString(), local: date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }) }
}

function parseStoredResult(content: string): BashToolResult | undefined {
  try {
    const value = JSON.parse(content) as Partial<BashToolResult>
    if (
      typeof value.stdout !== "string" ||
      typeof value.stderr !== "string" ||
      typeof value.exit_code !== "number" ||
      typeof value.timed_out !== "boolean" ||
      typeof value.truncated !== "boolean"
    ) {
      return undefined
    }
    return value as BashToolResult
  } catch {
    return undefined
  }
}

function parseStoredCommand(toolCall: ChatToolCall, messages: ChatMessage[], index: number): CommandStep | undefined {
  if (toolCall.function.name !== "bash") return undefined
  try {
    const value = JSON.parse(toolCall.function.arguments) as NormalizedBashArguments
    if (typeof value.command !== "string" || !value.command.trim()) return undefined
    const toolMessage = messages.slice(index + 1).find(message => message.role === "tool" && message.tool_call_id === toolCall.id)
    return {
      id: toolCall.id,
      arguments: {
        command: value.command,
        timeout_ms: typeof value.timeout_ms === "number" ? value.timeout_ms : 10_000,
        max_output_chars: typeof value.max_output_chars === "number" ? value.max_output_chars : 20_000,
      },
      result: toolMessage ? parseStoredResult(toolMessage.content) : undefined,
    }
  } catch {
    return undefined
  }
}

function reconstructTranscript(messages: ChatMessage[]): TranscriptItem[] {
  return messages.flatMap((message, index) => {
    if (message.role === "tool") return []
    const items: TranscriptItem[] = []
    if (!(message.role === "assistant" && message.tool_calls?.length && !message.content.trim())) {
      items.push({ type: "message", message })
    }
    if (message.role === "assistant") {
      for (const toolCall of message.tool_calls ?? []) {
        const command = parseStoredCommand(toolCall, messages, index)
        if (command) items.push({ type: "command", command })
      }
    }
    return items
  })
}

function ConversationHistory({ conversations, loading, onSelect, disabled = false, showHeading = true }: { conversations: ConversationSummary[]; loading: boolean; onSelect: (id: string) => void; disabled?: boolean; showHeading?: boolean }) {
  const groups = useMemo(() => groupConversations(new Date(), conversations), [conversations])
  return (
    <SpaceBetween size="m">
      {showHeading && <Header variant="h2" counter={loading ? undefined : `(${conversations.length})`}>Conversations</Header>}
      {loading ? <SpaceBetween size="s"><Skeleton kind="conversation" /><Skeleton kind="conversation" /><Skeleton kind="conversation" /></SpaceBetween> : groups.length === 0 ? <Box color="text-body-secondary">No conversations yet.</Box> : (
        <SpaceBetween size="l">
          {groups.map(group => (
            <section aria-label={group.label} key={group.label}>
              <Box fontWeight="bold" padding={{ bottom: "xs" }}>{group.label}</Box>
              <List ariaLabel={`${group.label} conversations`} items={group.conversations} renderItem={conversation => {
                const timestamp = formatTime(conversation.updated_at)
                return {
                  id: conversation.id,
                  content: <Button disabled={disabled} disabledReason="Wait for the current turn to finish." onClick={() => onSelect(conversation.id)} variant="inline-link">{conversation.title || "Untitled conversation"}</Button>,
                  secondaryContent: <span className="agent-chat-timestamp" title={new Date(conversation.updated_at).toLocaleString()}>{timestamp.local}</span>,
                }
              }} />
            </section>
          ))}
        </SpaceBetween>
      )}
    </SpaceBetween>
  )
}

function MessageBubble({ message, position, loading = false }: { message: ChatMessage; position: number; loading?: boolean }) {
  const incoming = message.role !== "user"
  const author = incoming ? "Assistant" : "You"
  const time = formatTime(message.created_at)
  return (
      <ChatBubble ariaLabel={`${author} at ${time.full}, message ${position}`} avatar={incoming ? <Avatar ariaLabel="Assistant avatar" color="gen-ai" iconName="gen-ai" loading={loading} /> : <Avatar ariaLabel="Your avatar" initials="Y" />} type={incoming ? "incoming" : "outgoing"}>
      <SpaceBetween size="xxs">
        {loading && !message.content ? <div>Generating a response</div> : <MarkdownMessage content={message.content} />}
        {!loading && <span className="agent-chat-timestamp" title={time.full}>{author} - {time.local}</span>}
      </SpaceBetween>
    </ChatBubble>
  )
}

function CommandStatus({ command }: { command: CommandStep }) {
  const { result } = command
  const running = command.running ?? !result
  const statusText = running ? "Running command" : result?.timed_out ? "Command timed out" : result?.exit_code === 0 ? "Command completed" : "Command failed"
  const statusType = running ? "in-progress" : result?.timed_out ? "warning" : result?.exit_code === 0 ? "success" : "error"
  return (
    <div className="agent-chat-command">
      <StatusIndicator type={statusType}>{statusText}</StatusIndicator>
      <ExpandableSection headerAriaLabel="Command details" headerText="Command details" variant="inline">
        <SpaceBetween size="xs">
          <Box variant="code">{command.arguments.command}</Box>
          {result && <>
            {result.stdout && <Box variant="code">{result.stdout}</Box>}
            {result.stderr && <Box variant="code">{result.stderr}</Box>}
            {result.truncated && <Box color="text-status-warning">Output was truncated</Box>}
          </>}
        </SpaceBetween>
      </ExpandableSection>
    </div>
  )
}

function isAbort(error: unknown): boolean {
  return error instanceof DOMException && error.name === "AbortError"
}

export function AgentChat() {
  const [conversations, setConversations] = useState<ConversationSummary[]>([])
  const [listLoading, setListLoading] = useState(true)
  const [selectedId, setSelectedId] = useState<string>()
  const [selectedConversation, setSelectedConversation] = useState<Conversation>()
  const [conversationLoading, setConversationLoading] = useState(false)
  const [historyOpen, setHistoryOpen] = useState(false)
  const [draft, setDraft] = useState("")
  const [liveItems, setLiveItems] = useState<TranscriptItem[]>([])
  const [liveAssistant, setLiveAssistant] = useState<ChatMessage>()
  const [assistantLoading, setAssistantLoading] = useState(false)
  const [running, setRunning] = useState(false)
  const [abortable, setAbortable] = useState(false)
  const [turnError, setTurnError] = useState<string>()
  const [refreshError, setRefreshError] = useState<string>()
  const [initialCreatePrompt, setInitialCreatePrompt] = useState<string>()
  const mobileHistory = useMobileHistory()
  const pageRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<ButtonProps.Ref>(null)
  const closeButtonRef = useRef<ButtonProps.Ref>(null)
  const dialogRef = useRef<HTMLDivElement>(null)
  const transcriptRef = useRef<HTMLElement>(null)
  const wasHistoryOpenRef = useRef(false)
  const agentRef = useRef<BrowserBashAgent>()
  const abortControllerRef = useRef<AbortController>()
  const loadingTimerRef = useRef<number>()
  const ignoreNextConversationLoadRef = useRef(false)
  const nextRunIdRef = useRef(0)
  const activeRunIdRef = useRef<number>()
  const activeRunConversationIdRef = useRef<string>()
  const selectedConversationIdRef = useRef<string>()
  const loadingRunIdRef = useRef<number>()

  useLayoutEffect(() => {
    const element = pageRef.current
    if (!element) return

    const measure = () => {
      const box = element.getBoundingClientRect()
      const top = Math.max(0, Math.round(box.top))
      element.style.setProperty(
        "--cua-chat-viewport-offset",
        `${top + 16}px`,
      )
    }
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(document.documentElement)
    window.addEventListener("resize", measure)
    return () => {
      observer.disconnect()
      window.removeEventListener("resize", measure)
    }
  }, [])

  if (!agentRef.current) {
    agentRef.current = new BrowserBashAgent(async (conversationID, messages, signal, onDelta) => {
      const message = await streamTurn(conversationID, messages, onDelta ?? (() => undefined), signal)
      return {
        role: "assistant",
        content: message.content,
        tool_calls: (message.tool_calls ?? []).filter((toolCall): toolCall is ToolCall => toolCall.type === "function"),
      }
    }, [...createBrowserSdkCommands(browserSdk), ...createBrowserMcpCommands()])
  }

  const ownsRun = (runId: number, conversationID: string) =>
    activeRunIdRef.current === runId &&
    activeRunConversationIdRef.current === conversationID &&
    selectedConversationIdRef.current === conversationID

  const ownsUnboundRun = (runId: number) =>
    activeRunIdRef.current === runId && activeRunConversationIdRef.current === undefined

  const ownsLifecycle = (runId: number, conversationID?: string) =>
    conversationID === undefined ? ownsUnboundRun(runId) : ownsRun(runId, conversationID)

  const refresh = async (conversationID: string, runId?: number) => {
    const [conversation, summaries] = await Promise.all([getConversation(conversationID), listConversations()])
    if (runId !== undefined && !ownsRun(runId, conversationID)) return false
    setSelectedConversation(conversation)
    setConversations(summaries)
    setLiveItems([])
    setLiveAssistant(undefined)
    setRefreshError(undefined)
    return true
  }

  const recoverRefresh = async (conversationID: string) => {
    if (activeRunIdRef.current !== undefined || selectedConversationIdRef.current !== conversationID) return
    const runId = ++nextRunIdRef.current
    activeRunIdRef.current = runId
    activeRunConversationIdRef.current = conversationID
    setRunning(true)
    setRefreshError(undefined)
    try {
      await refresh(conversationID, runId)
    } catch (error) {
      if (ownsRun(runId, conversationID)) setRefreshError(error instanceof Error ? error.message : "Unable to refresh conversation")
    } finally {
      finishRun(runId, conversationID)
    }
  }

  useEffect(() => {
    let active = true
    void listConversations().then(items => { if (active) setConversations(items) }).catch(() => { if (active) setConversations([]) }).finally(() => { if (active) setListLoading(false) })
    return () => { active = false }
  }, [])

  useEffect(() => {
    selectedConversationIdRef.current = selectedId
  }, [selectedId])

  useEffect(() => {
    if (!selectedId) { setSelectedConversation(undefined); return }
    let active = true
    if (ignoreNextConversationLoadRef.current) {
      ignoreNextConversationLoadRef.current = false
      return
    }
    setConversationLoading(true)
    setSelectedConversation(undefined)
    setLiveItems([])
    setLiveAssistant(undefined)
    void getConversation(selectedId).then(conversation => { if (active && selectedConversationIdRef.current === selectedId) setSelectedConversation(conversation) }).catch(() => { if (active && selectedConversationIdRef.current === selectedId) setSelectedConversation(undefined) }).finally(() => { if (active && selectedConversationIdRef.current === selectedId) setConversationLoading(false) })
    return () => { active = false }
  }, [selectedId])

  useEffect(() => () => {
    abortControllerRef.current?.abort()
    if (loadingTimerRef.current) window.clearTimeout(loadingTimerRef.current)
  }, [])

  useEffect(() => {
    const appRoot = document.getElementById("root")
    if (appRoot) appRoot.inert = historyOpen
    if (historyOpen) {
      wasHistoryOpenRef.current = true
      closeButtonRef.current?.focus()
      return () => { if (appRoot) appRoot.inert = false }
    }
    if (wasHistoryOpenRef.current) { wasHistoryOpenRef.current = false; triggerRef.current?.focus() }
  }, [historyOpen])

  useEffect(() => { if (!mobileHistory) setHistoryOpen(false) }, [mobileHistory])

  useEffect(() => {
    const transcriptElement = transcriptRef.current
    const hasTranscriptUpdate = assistantLoading || Boolean(liveAssistant?.content) || liveItems.length > 0 || Boolean(selectedConversation)
    if (!transcriptElement || conversationLoading || !hasTranscriptUpdate) return
    transcriptElement.scrollTop = transcriptElement.scrollHeight
  }, [assistantLoading, conversationLoading, liveAssistant?.content, liveItems.length, selectedConversation])

  const clearLoading = (runId?: number, conversationID?: string) => {
    if (runId !== undefined && !ownsLifecycle(runId, conversationID)) return
    if (loadingTimerRef.current) window.clearTimeout(loadingTimerRef.current)
    loadingTimerRef.current = undefined
    loadingRunIdRef.current = undefined
    setAssistantLoading(false)
  }

  const finishAbortable = (runId: number, conversationID: string, controller: AbortController) => {
    if (!ownsRun(runId, conversationID)) return
    if (abortControllerRef.current === controller) abortControllerRef.current = undefined
    setAbortable(false)
  }

  const finishRun = (runId: number, conversationID?: string) => {
    if (!ownsLifecycle(runId, conversationID)) return
    clearLoading(runId, conversationID)
    abortControllerRef.current = undefined
    activeRunIdRef.current = undefined
    activeRunConversationIdRef.current = undefined
    setAbortable(false)
    setRunning(false)
  }

  const runAgent = async (runId: number, conversationID: string, messages: ClientMessage[]) => {
    const controller = new AbortController()
    let completedCommand = false
    abortControllerRef.current = controller
    setAbortable(true)
    try {
      await agentRef.current!.run(conversationID, messages, event => {
        if (!ownsRun(runId, conversationID)) return
        if (event.type === "generation_start") {
          clearLoading(runId, conversationID)
          loadingRunIdRef.current = runId
          loadingTimerRef.current = window.setTimeout(() => {
            if (ownsRun(runId, conversationID) && loadingRunIdRef.current === runId) setAssistantLoading(true)
          }, GENERATION_LOADING_DELAY_MS)
        } else if (event.type === "assistant_delta") {
          clearLoading(runId, conversationID)
          setLiveAssistant(current => ({ role: "assistant", content: `${current?.content ?? ""}${event.delta}`, created_at: new Date().toISOString() }))
        } else if (event.type === "tool_start") {
          clearLoading(runId, conversationID)
          setLiveItems(current => [...current, { type: "command", command: { id: event.toolCall.id, arguments: event.arguments, running: true } }])
        } else if (event.type === "tool_result") {
          setLiveItems(current => current.map(item => item.type === "command" && item.command.id === event.toolCall.id ? { type: "command", command: { ...item.command, result: event.result, running: true } } : item))
          completedCommand = true
          window.setTimeout(() => {
            if (!ownsRun(runId, conversationID)) return
            setLiveItems(current => current.map(item => item.type === "command" && item.command.id === event.toolCall.id ? { type: "command", command: { ...item.command, running: false } } : item))
          }, 350)
        } else if (event.type === "complete") {
          clearLoading(runId, conversationID)
          setLiveAssistant(current => ({ role: "assistant", content: current?.content || event.message.content, created_at: new Date().toISOString() }))
        }
      }, controller.signal)
    } catch (error) {
      if (ownsRun(runId, conversationID) && !isAbort(error)) setTurnError(error instanceof Error ? error.message : "Unable to generate a response")
      finishAbortable(runId, conversationID, controller)
      finishRun(runId, conversationID)
      return
    }

    finishAbortable(runId, conversationID, controller)
    if (completedCommand) await new Promise<void>(resolve => window.setTimeout(resolve, 360))

    try {
      await refresh(conversationID, runId)
    } catch (error) {
      if (ownsRun(runId, conversationID)) setRefreshError(error instanceof Error ? error.message : "Unable to refresh conversation")
    } finally {
      finishRun(runId, conversationID)
    }
  }

  const submitPrompt = async (prompt: string, initialConversationID = selectedId) => {
    if (activeRunIdRef.current !== undefined) return
    const runId = ++nextRunIdRef.current
    activeRunIdRef.current = runId
    activeRunConversationIdRef.current = initialConversationID
    setRunning(true)
    setTurnError(undefined)
    setRefreshError(undefined)
    setInitialCreatePrompt(undefined)
    let conversationID = initialConversationID
    try {
      if (!conversationID) {
        const conversation = await createConversation()
        if (!ownsUnboundRun(runId)) return
        conversationID = conversation.id
        activeRunConversationIdRef.current = conversation.id
        selectedConversationIdRef.current = conversation.id
        ignoreNextConversationLoadRef.current = true
        setSelectedId(conversation.id)
        setSelectedConversation(conversation)
      }
      setDraft("")
      setLiveItems([{ type: "message", message: { role: "user", content: prompt, created_at: new Date().toISOString() } }])
      await runAgent(runId, conversationID, [{ role: "user", content: prompt }])
    } catch (error) {
      if (!conversationID && ownsUnboundRun(runId)) {
        setInitialCreatePrompt(prompt)
        setTurnError(error instanceof Error ? error.message : "Unable to create conversation")
        finishRun(runId)
      } else if (conversationID && ownsRun(runId, conversationID)) {
        setTurnError(error instanceof Error ? error.message : "Unable to create conversation")
        finishRun(runId, conversationID)
      }
    }
  }

  const submit = async () => {
    const prompt = draft.trim()
    if (!prompt) return
    await submitPrompt(prompt)
  }

  const retryTurn = () => {
    if (!selectedId || activeRunIdRef.current !== undefined) return
    const runId = ++nextRunIdRef.current
    activeRunIdRef.current = runId
    activeRunConversationIdRef.current = selectedId
    setRunning(true)
    setTurnError(undefined)
    setRefreshError(undefined)
    setLiveAssistant(undefined)
    void runAgent(runId, selectedId, [])
  }

  const retryInitialCreate = () => {
    if (!initialCreatePrompt) return
    void submitPrompt(initialCreatePrompt, undefined)
  }

  const stop = () => {
    if (!abortable) return
    abortControllerRef.current?.abort()
    clearLoading(activeRunIdRef.current, activeRunConversationIdRef.current)
    setLiveAssistant(undefined)
  }

  const closeHistory = () => setHistoryOpen(false)
  const trapDialogFocus = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") { event.preventDefault(); closeHistory(); return }
    if (event.key !== "Tab") return
    const focusable = dialogRef.current?.querySelectorAll<HTMLElement>('a[href], button:not([disabled]):not([tabindex="-1"]), input:not([disabled]):not([tabindex="-1"]), select:not([disabled]):not([tabindex="-1"]), textarea:not([disabled]):not([tabindex="-1"]), [tabindex]:not([tabindex="-1"])')
    if (!focusable?.length) { event.preventDefault(); return }
    const first = focusable[0]
    const last = focusable[focusable.length - 1]
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus() }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus() }
  }

  const selectConversation = (conversationID: string) => {
    if (activeRunIdRef.current !== undefined) return false
    selectedConversationIdRef.current = conversationID
    setSelectedId(conversationID)
    return true
  }

  const persistedItems = selectedConversation ? reconstructTranscript(selectedConversation.messages) : []
  const transcript = [...persistedItems, ...liveItems]
  const latestMessage = [...transcript].reverse().find((item): item is Extract<TranscriptItem, { type: "message" }> => item.type === "message")?.message
  const effectiveMessage = liveAssistant ?? latestMessage
  const latestAnnouncement = abortable
    ? "Generating response"
    : effectiveMessage?.content
      ? `Latest ${effectiveMessage.role === "user" ? "user" : "assistant"} message available.`
      : ""
  const drawerHistory = <ConversationHistory conversations={conversations} disabled={running} loading={listLoading} onSelect={id => { if (selectConversation(id)) closeHistory() }} showHeading={false} />
  const firstConversation = !listLoading && conversations.length === 0

  return (
    <>
      <PageShell
        eyebrow="Agent"
        title="Chat"
        description={
          mobileHistory
            ? undefined
            : "Run fleet and browser tasks with the Cua agent."
        }
      >
        <div className="agent-chat-page" ref={pageRef}>
      <aside className="agent-chat-history"><ConversationHistory conversations={conversations} disabled={running} loading={listLoading} onSelect={selectConversation} /></aside>
      <div className="agent-chat-main">
        <div className="agent-chat-mobile-history"><Button ariaLabel="View conversations" iconName="menu" onClick={() => setHistoryOpen(true)} ref={triggerRef}>Conversations</Button></div>
        <section aria-label="Chat" className="agent-chat-transcript" ref={transcriptRef}>
          {conversationLoading ? <SpaceBetween size="m"><Skeleton kind="message" /><Skeleton kind="message" /></SpaceBetween> : transcript.length || liveAssistant || assistantLoading ? (
            <SpaceBetween size="m">
              {transcript.map((item, index) => item.type === "message" ? <MessageBubble key={item.message.id ?? `message-${index}`} message={item.message} position={index + 1} /> : <CommandStatus command={item.command} key={item.command.id} />)}
              {liveAssistant && <MessageBubble loading={abortable} message={liveAssistant} position={transcript.length + 1} />}
              {!liveAssistant && assistantLoading && <MessageBubble loading message={{ role: "assistant", content: "", created_at: new Date().toISOString() }} position={transcript.length + 1} />}
            </SpaceBetween>
          ) : <div className="agent-chat-empty"><Box variant="h2">{firstConversation ? "Start a conversation" : "Select a conversation"}</Box><Box color="text-body-secondary">{firstConversation ? "Ask a question below to create your first conversation." : "Choose a previous conversation to view its transcript."}</Box></div>}
          {turnError && <div role="alert"><Alert action={<Button onClick={initialCreatePrompt ? retryInitialCreate : retryTurn}>Retry</Button>} type="error">{turnError}</Alert></div>}
          {refreshError && <div role="alert"><Alert action={<Button disabled={running} onClick={() => { if (selectedId) void recoverRefresh(selectedId) }}>Refresh conversation</Button>} type="error">{refreshError}</Alert></div>}
          <div className="agent-chat-announcements">
            <LiveRegion>{conversationLoading ? "Loading conversation" : selectedConversation ? "Conversation loaded" : "No conversation selected"}</LiveRegion>
            <LiveRegion>{latestAnnouncement}</LiveRegion>
          </div>
        </section>
        <div className="agent-chat-composer">
          <div className="agent-chat-composer-surface">
            <div className="agent-chat-prompt"><PromptInput actionButtonAriaLabel="Send message" actionButtonIconName="send" disableActionButton={running || !draft.trim()} maxRows={6} minRows={2} onAction={() => void submit()} onChange={({ detail }) => setDraft(detail.value)} placeholder="Ask a question" value={draft} /></div>
            {abortable && <Button ariaLabel="Stop generating" onClick={stop}>Stop</Button>}
          </div>
        </div>
      </div>
        </div>
      </PageShell>
      {mobileHistory && historyOpen && createPortal(
        <div aria-label="Conversations" aria-modal="true" className="agent-chat-mobile-overlay" onKeyDown={trapDialogFocus} ref={dialogRef} role="dialog">
          <div aria-hidden="true" className="agent-chat-mobile-backdrop" data-testid="conversation-history-backdrop" onClick={closeHistory} />
          <div className="agent-chat-mobile-drawer"><Drawer header={<h2>Conversations</h2>} headerActions={<Button ariaLabel="Close conversations" iconName="close" onClick={closeHistory} ref={closeButtonRef} variant="icon" />}>{drawerHistory}</Drawer></div>
        </div>, document.body,
      )}
    </>
  )
}

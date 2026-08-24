import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useLocation, useNavigate, useParams } from "react-router-dom";
import Avatar from "@cloudscape-design/chat-components/avatar";
import ChatBubble from "@cloudscape-design/chat-components/chat-bubble";
import Alert from "@cloudscape-design/components/alert";
import Box from "@cloudscape-design/components/box";
import Button from "@cloudscape-design/components/button";
import Container from "@cloudscape-design/components/container";
import CopyToClipboard from "@cloudscape-design/components/copy-to-clipboard";
import ExpandableSection from "@cloudscape-design/components/expandable-section";
import FormField from "@cloudscape-design/components/form-field";
import Header from "@cloudscape-design/components/header";
import PromptInput from "@cloudscape-design/components/prompt-input";
import SpaceBetween from "@cloudscape-design/components/space-between";
import StatusIndicator from "@cloudscape-design/components/status-indicator";
import { MarkdownMessage } from "../components/MarkdownMessage";
import { PageShell } from "../components/PageShell";
import { createClaim, deleteClaim, getClaim, listClaims } from "../fleet/claims";
import {
  createPool,
  deletePool,
  getPool,
  listNamespaces,
  listPools,
  updatePoolServices,
} from "../fleet/pools";
import { createUserKey, deleteUserKey, listUserKeys } from "../fleet/userKeys";
import {
  ChatApiError,
  getConversation,
  streamTurn,
  type ChatMessage,
  type ChatToolCall,
  type Conversation,
  type ConversationSummary,
} from "../api/chat";
import {
  BrowserBashAgent,
  type BashToolResult,
  type ClientMessage,
  type NormalizedBashArguments,
  type ToolCall,
} from "../browser-agent";
import { createBrowserMcpCommands } from "../browser-mcp-commands";
import { SensitiveOutputBuffer, type SensitiveOutput, type UserApiKeyCredentials } from "../sensitive-output";
import { useChatThreads } from "../chat/ChatThreadsContext";
import { localVisualPreviewPath } from "../local-visual-preview";
import {
  createBrowserSdkCommands,
  type BrowserSdk,
} from "../browser-sdk-commands";
import "./AgentChat.css";

export interface ConversationGroup {
  label: string;
  conversations: ConversationSummary[];
}

interface CommandStep {
  id: string;
  arguments: NormalizedBashArguments;
  result?: BashToolResult;
  sensitiveOutputs?: SensitiveOutput[];
  running?: boolean;
}

type TranscriptItem =
  | { type: "message"; message: ChatMessage }
  | { type: "command"; command: CommandStep };

export function groupConversations(
  now: Date,
  summaries: ConversationSummary[],
): ConversationGroup[] {
  const startOfToday = new Date(now);
  startOfToday.setHours(0, 0, 0, 0);
  const groups = new Map<string, ConversationSummary[]>();

  for (const summary of summaries) {
    const updatedAt = new Date(summary.updated_at);
    const startOfDate = new Date(updatedAt);
    startOfDate.setHours(0, 0, 0, 0);
    const daysAgo = Math.round(
      (startOfToday.getTime() - startOfDate.getTime()) / 86_400_000,
    );
    const label =
      daysAgo === 0
        ? "Today"
        : daysAgo === 1
          ? "Yesterday"
          : daysAgo >= 2 && daysAgo <= 7
            ? "Past 7 days"
            : updatedAt.toLocaleDateString(undefined, {
                year: "numeric",
                month: "long",
                day: "numeric",
              });
    const group = groups.get(label) ?? [];
    group.push(summary);
    groups.set(label, group);
  }

  return [...groups].map(([label, conversations]) => ({
    label,
    conversations,
  }));
}

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
};

function Skeleton({ kind }: { kind: "conversation" | "message" }) {
  return (
    <div
      aria-hidden="true"
      className={`agent-chat-skeleton agent-chat-skeleton--${kind}`}
      data-testid={`${kind}-skeleton`}
    />
  );
}

function formatTime(timestamp?: string): { full: string; local: string } {
  if (!timestamp) return { full: "Unknown time", local: "Unknown time" };
  const date = new Date(timestamp);
  return {
    full: date.toLocaleString(),
    local: date.toLocaleTimeString(undefined, {
      hour: "numeric",
      minute: "2-digit",
    }),
  };
}

function parseStoredResult(content: string): BashToolResult | undefined {
  try {
    const value = JSON.parse(content) as Partial<BashToolResult>;
    if (
      typeof value.stdout !== "string" ||
      typeof value.stderr !== "string" ||
      typeof value.exit_code !== "number" ||
      typeof value.timed_out !== "boolean" ||
      typeof value.truncated !== "boolean"
    ) {
      return undefined;
    }
    return value as BashToolResult;
  } catch {
    return undefined;
  }
}

function parseStoredCommand(
  toolCall: ChatToolCall,
  messages: ChatMessage[],
  index: number,
  sensitiveOutputsByToolCall: Record<string, SensitiveOutput[]>,
): CommandStep | undefined {
  if (toolCall.function.name !== "bash") return undefined;
  try {
    const value = JSON.parse(
      toolCall.function.arguments,
    ) as NormalizedBashArguments;
    if (typeof value.command !== "string" || !value.command.trim())
      return undefined;
    const toolMessage = messages
      .slice(index + 1)
      .find(
        (message) =>
          message.role === "tool" && message.tool_call_id === toolCall.id,
      );
    return {
      id: toolCall.id,
      arguments: {
        command: value.command,
        timeout_ms:
          typeof value.timeout_ms === "number" ? value.timeout_ms : 10_000,
        max_output_chars:
          typeof value.max_output_chars === "number"
            ? value.max_output_chars
            : 20_000,
      },
      result: toolMessage ? parseStoredResult(toolMessage.content) : undefined,
      sensitiveOutputs: sensitiveOutputsByToolCall[toolCall.id],
    };
  } catch {
    return undefined;
  }
}

function reconstructTranscript(
  messages: ChatMessage[],
  sensitiveOutputsByToolCall: Record<string, SensitiveOutput[]>,
): TranscriptItem[] {
  return messages.flatMap((message, index) => {
    if (message.role === "tool") return [];
    const items: TranscriptItem[] = [];
    if (!(
      message.role === "assistant" &&
      message.tool_calls?.length &&
      !message.content.trim()
    )) {
      items.push({ type: "message", message });
    }
    if (message.role === "assistant") {
      for (const toolCall of message.tool_calls ?? []) {
        const command = parseStoredCommand(
          toolCall,
          messages,
          index,
          sensitiveOutputsByToolCall,
        );
        if (command) items.push({ type: "command", command });
      }
    }
    return items;
  });
}

function MessageBubble({
  message,
  position,
  loading = false,
}: {
  message: ChatMessage;
  position: number;
  loading?: boolean;
}) {
  const incoming = message.role !== "user";
  const author = incoming ? "Assistant" : "You";
  const time = formatTime(message.created_at);
  return (
    <ChatBubble
      ariaLabel={`${author} at ${time.full}, message ${position}`}
      avatar={
        incoming ? (
          <Avatar
            ariaLabel="Assistant avatar"
            color="gen-ai"
            iconName="gen-ai"
            loading={loading}
          />
        ) : (
          <Avatar ariaLabel="Your avatar" initials="Y" />
        )
      }
      type={incoming ? "incoming" : "outgoing"}
    >
      <SpaceBetween size="xxs">
        {loading && !message.content ? (
          <div aria-hidden="true" className="agent-chat-thinking">
            <span>Thinking...</span>
            <span className="agent-chat-thinking-dots">
              <span />
              <span />
              <span />
            </span>
          </div>
        ) : (
          <MarkdownMessage content={message.content} />
        )}
        {!loading && (
          <span className="agent-chat-timestamp" title={time.full}>
            {author} - {time.local}
          </span>
        )}
      </SpaceBetween>
    </ChatBubble>
  );
}

function UserApiKeyCredentialCard({
  credentials,
}: {
  credentials: UserApiKeyCredentials;
}) {
  return (
    <Container header={<Header variant="h3">API key created</Header>}>
      <SpaceBetween size="m">
        <Alert type="warning">
          These credentials are held only in this page's memory. Copy them
          before reloading or closing the page.
        </Alert>
        <FormField label="Client ID">
          <CopyToClipboard
            variant="inline"
            textToCopy={credentials.clientId}
            textToDisplay={<code>{credentials.clientId}</code>}
            copyButtonAriaLabel="Copy client ID"
            copySuccessText="Client ID copied"
            copyErrorText="Failed to copy"
          />
        </FormField>
        <FormField label="Client Secret">
          <CopyToClipboard
            variant="inline"
            textToCopy={credentials.clientSecret}
            textToDisplay={<code>{credentials.clientSecret}</code>}
            copyButtonAriaLabel="Copy client secret"
            copySuccessText="Client secret copied"
            copyErrorText="Failed to copy"
          />
        </FormField>
        <FormField label="Token URL">
          <CopyToClipboard
            variant="inline"
            textToCopy={credentials.tokenUrl}
            textToDisplay={<code>{credentials.tokenUrl}</code>}
            copyButtonAriaLabel="Copy token URL"
            copySuccessText="Token URL copied"
            copyErrorText="Failed to copy"
          />
        </FormField>
        <FormField label="Name">
          <Box>{credentials.name}</Box>
        </FormField>
        <FormField label="Scopes">
          <Box>{credentials.scope.join(", ") || "Unrestricted"}</Box>
        </FormField>
      </SpaceBetween>
    </Container>
  );
}

function CommandStatus({ command }: { command: CommandStep }) {
  const { result } = command;
  const running = command.running ?? !result;
  const statusText = running
    ? "Running command"
    : result?.timed_out
      ? "Command timed out"
      : result?.exit_code === 0
        ? "Command completed"
        : "Command failed";
  const statusType = running
    ? "in-progress"
    : result?.timed_out
      ? "warning"
      : result?.exit_code === 0
        ? "success"
        : "error";
  return (
    <div className="agent-chat-command">
      <StatusIndicator type={statusType}>{statusText}</StatusIndicator>
      <ExpandableSection
        headerAriaLabel="Command details"
        headerText="Command details"
        variant="inline"
      >
        <SpaceBetween size="xs">
          <Box variant="code">{command.arguments.command}</Box>
          {result && (
            <>
              {result.stdout && <Box variant="code">{result.stdout}</Box>}
              {result.stderr && <Box variant="code">{result.stderr}</Box>}
              {result.truncated && (
                <Box color="text-status-warning">Output was truncated</Box>
              )}
            </>
          )}
        </SpaceBetween>
      </ExpandableSection>
      {command.sensitiveOutputs?.map((output, index) =>
        output.kind === "user_api_key" ? (
          <div className="agent-chat-credential" key={index}>
            <UserApiKeyCredentialCard credentials={output.value} />
          </div>
        ) : null,
      )}
    </div>
  );
}

function isAbort(error: unknown): boolean {
  return error instanceof DOMException && error.name === "AbortError";
}

export function AgentChat() {
  const { threadId } = useParams<{ threadId: string }>();
  const location = useLocation();
  const navigate = useNavigate();
  const {
    generationLocked,
    isThreadMutationPending,
    recoverToActiveThread,
    refreshActiveThreads,
    restoreThread,
    setGenerationLocked,
    threadMutationLocked,
  } = useChatThreads();
  const [conversation, setConversation] = useState<Conversation>();
  const [conversationLoading, setConversationLoading] = useState(
    Boolean(threadId),
  );
  const [conversationError, setConversationError] = useState<string>();
  const [conversationErrorStatus, setConversationErrorStatus] = useState<number>();
  const [draft, setDraft] = useState("");
  const [liveItems, setLiveItems] = useState<TranscriptItem[]>([]);
  const [liveAssistant, setLiveAssistant] = useState<ChatMessage>();
  const [sensitiveOutputsByToolCall, setSensitiveOutputsByToolCall] = useState<Record<string, SensitiveOutput[]>>({});
  const [assistantLoading, setAssistantLoading] = useState(false);
  const [running, setRunning] = useState(false);
  const [abortable, setAbortable] = useState(false);
  const [turnError, setTurnError] = useState<string>();
  const [refreshError, setRefreshError] = useState<string>();
  const [latestAnnouncement, setLatestAnnouncement] = useState("");
  const [restorePending, setRestorePending] = useState(false);
  const [archivedTurnNotice, setArchivedTurnNotice] = useState<string>();
  const pageRef = useRef<HTMLDivElement>(null);
  const transcriptRef = useRef<HTMLElement>(null);
  const agentRef = useRef<BrowserBashAgent>();
  const sensitiveOutputBufferRef = useRef<SensitiveOutputBuffer>();
  const abortControllerRef = useRef<AbortController>();
  const loadSequenceRef = useRef(0);
  const nextRunIdRef = useRef(0);
  const activeRunIdRef = useRef<number>();
  const activeRunConversationIdRef = useRef<string>();
  const currentThreadIdRef = useRef<string>();
  const restoreTokenRef = useRef(0);
  const routePath = location.pathname;
  const pathnameRef = useRef(routePath);
  pathnameRef.current = routePath;

  useLayoutEffect(() => {
    const element = pageRef.current;
    if (!element) return;

    const measure = () => {
      const box = element.getBoundingClientRect();
      const top = Math.max(0, Math.round(box.top));
      element.style.setProperty(
        "--cua-chat-viewport-offset",
        `${String(top + 16)}px`,
      );
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(document.documentElement);
    window.addEventListener("resize", measure);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, []);

  if (!sensitiveOutputBufferRef.current) {
    sensitiveOutputBufferRef.current = new SensitiveOutputBuffer();
  }

  if (!agentRef.current) {
    const sensitiveOutputs = sensitiveOutputBufferRef.current;
    agentRef.current = new BrowserBashAgent(
      async (conversationID, messages, signal, onDelta) => {
        const message = await streamTurn(
          conversationID,
          messages,
          onDelta ?? (() => undefined),
          signal,
        );
        return {
          role: "assistant",
          content: message.content,
          tool_calls: (message.tool_calls ?? []).filter(
            (toolCall): toolCall is ToolCall => toolCall.type === "function",
          ),
        };
      },
      [
        ...createBrowserSdkCommands(browserSdk, sensitiveOutputs),
        ...createBrowserMcpCommands(),
      ],
      sensitiveOutputs,
    );
  }

  const ownsRun = (runId: number, conversationID: string) =>
    activeRunIdRef.current === runId &&
    activeRunConversationIdRef.current === conversationID &&
    currentThreadIdRef.current === conversationID;

  const clearLoading = (runId?: number, conversationID?: string) => {
    if (
      runId !== undefined &&
      conversationID !== undefined &&
      !ownsRun(runId, conversationID)
    )
      return;
    setAssistantLoading(false);
  };

  const finishRun = (runId: number, conversationID: string) => {
    if (!ownsRun(runId, conversationID)) return;
    clearLoading(runId, conversationID);
    abortControllerRef.current = undefined;
    activeRunIdRef.current = undefined;
    activeRunConversationIdRef.current = undefined;
    setAbortable(false);
    setRunning(false);
    setGenerationLocked(false);
    window.setTimeout(() =>
      pageRef.current?.querySelector<HTMLTextAreaElement>("textarea")?.focus(),
    );
  };

  const refresh = async (conversationID: string, runId?: number) => {
    const refreshed = await getConversation(conversationID);
    if (runId !== undefined && !ownsRun(runId, conversationID)) return false;
    if (runId === undefined && currentThreadIdRef.current !== conversationID)
      return false;
    setConversation(refreshed);
    setLiveItems([]);
    setLiveAssistant(undefined);
    setRefreshError(undefined);
    await refreshActiveThreads();
    return true;
  };

  useEffect(() => {
    if (pathnameRef.current !== routePath) return;

    const invalidateRouteRun = () => {
      abortControllerRef.current?.abort();
      abortControllerRef.current = undefined;
      activeRunIdRef.current = undefined;
      activeRunConversationIdRef.current = undefined;
      setAssistantLoading(false);
      setAbortable(false);
      setRunning(false);
      setGenerationLocked(false);
    };

    restoreTokenRef.current += 1;
    setRestorePending(false);
    setArchivedTurnNotice(undefined);
    currentThreadIdRef.current = threadId;
    const loadSequence = ++loadSequenceRef.current;
    if (
      activeRunConversationIdRef.current &&
      activeRunConversationIdRef.current !== threadId
    )
      invalidateRouteRun();
    setConversation(undefined);
    setLiveItems([]);
    setLiveAssistant(undefined);
    setConversationError(undefined);
    setConversationErrorStatus(undefined);
    setTurnError(undefined);
    setRefreshError(undefined);
    setConversationLoading(Boolean(threadId));

    if (!threadId)
      return () => {
        loadSequenceRef.current += 1;
      };

    void getConversation(threadId)
      .then((loaded) => {
        if (
          loadSequenceRef.current !== loadSequence ||
          currentThreadIdRef.current !== threadId
        )
          return;
        setConversation(loaded);
      })
      .catch((error) => {
        if (
          loadSequenceRef.current !== loadSequence ||
          currentThreadIdRef.current !== threadId
        )
          return;
        setConversationError(
          error instanceof Error
            ? error.message
            : "Unable to load conversation",
        );
        setConversationErrorStatus(
          error instanceof ChatApiError ? error.status : undefined,
        );
      })
      .finally(() => {
        if (
          loadSequenceRef.current === loadSequence &&
          currentThreadIdRef.current === threadId
        )
          setConversationLoading(false);
      });

    return () => {
      loadSequenceRef.current += 1;
      if (activeRunConversationIdRef.current === threadId) invalidateRouteRun();
    };
  }, [routePath, threadId, setGenerationLocked]);

  useEffect(
    () => () => {
      currentThreadIdRef.current = undefined;
      abortControllerRef.current?.abort();
      abortControllerRef.current = undefined;
      activeRunIdRef.current = undefined;
      activeRunConversationIdRef.current = undefined;
      setAssistantLoading(false);
      setGenerationLocked(false);
    },
    [setGenerationLocked],
  );

  useEffect(() => {
    const transcriptElement = transcriptRef.current;
    const hasTranscriptUpdate =
      assistantLoading ||
      Boolean(liveAssistant?.content) ||
      liveItems.length > 0 ||
      Boolean(conversation);
    if (!transcriptElement || conversationLoading || !hasTranscriptUpdate)
      return;
    transcriptElement.scrollTop = transcriptElement.scrollHeight;
  }, [
    assistantLoading,
    conversation,
    conversationLoading,
    liveAssistant?.content,
    liveItems.length,
  ]);

  const runAgent = async (
    runId: number,
    conversationID: string,
    messages: ClientMessage[],
  ) => {
    const controller = new AbortController();
    let completedCommand = false;
    abortControllerRef.current = controller;
    setAbortable(true);
    try {
      await agentRef.current!.run(
        conversationID,
        messages,
        (event) => {
          if (!ownsRun(runId, conversationID)) return;
          if (event.type === "assistant_delta") {
            clearLoading(runId, conversationID);
            setLiveAssistant((current) => ({
              role: "assistant",
              content: (current?.content ?? "") + event.delta,
              created_at: new Date().toISOString(),
            }));
          } else if (event.type === "tool_start") {
            clearLoading(runId, conversationID);
            setLiveItems((current) => [
              ...current,
              {
                type: "command",
                command: {
                  id: event.toolCall.id,
                  arguments: event.arguments,
                  running: true,
                },
              },
            ]);
          } else if (event.type === "tool_result") {
            if (event.sensitiveOutputs.length > 0) {
              setSensitiveOutputsByToolCall((current) => ({
                ...current,
                [event.toolCall.id]: event.sensitiveOutputs,
              }));
            }
            setLiveItems((current) =>
              current.map((item) =>
                item.type === "command" && item.command.id === event.toolCall.id
                  ? {
                      type: "command",
                      command: {
                        ...item.command,
                        result: event.result,
                        sensitiveOutputs: event.sensitiveOutputs,
                        running: true,
                      },
                    }
                  : item,
              ),
            );
            completedCommand = true;
            window.setTimeout(() => {
              if (!ownsRun(runId, conversationID)) return;
              setLiveItems((current) =>
                current.map((item) =>
                  item.type === "command" &&
                  item.command.id === event.toolCall.id
                    ? {
                        type: "command",
                        command: { ...item.command, running: false },
                      }
                    : item,
                ),
              );
            }, 350);
          } else if (event.type === "complete") {
            setLatestAnnouncement("Latest assistant message available.");
            clearLoading(runId, conversationID);
            setLiveAssistant((current) => ({
              role: "assistant",
              content: current?.content || event.message.content,
              created_at: new Date().toISOString(),
            }));
          }
        },
        controller.signal,
      );
    } catch (error) {
      if (!ownsRun(runId, conversationID)) return;
      if (error instanceof ChatApiError && error.status === 409) {
        setTurnError(undefined);
        setConversation((current) =>
          current?.id === conversationID
            ? {
                ...current,
                archived_at: current.archived_at ?? new Date().toISOString(),
              }
            : current,
        );
        setArchivedTurnNotice(
          "This thread was archived while generating and is now read-only.",
        );
        try {
          await refresh(conversationID, runId);
        } catch (refreshFailure) {
          if (ownsRun(runId, conversationID))
            setRefreshError(
              refreshFailure instanceof Error
                ? refreshFailure.message
                : "Unable to refresh conversation",
            );
        }
      } else if (!isAbort(error)) {
        setTurnError(
          error instanceof Error
            ? error.message
            : "Unable to generate a response",
        );
      }
      finishRun(runId, conversationID);
      return;
    }

    if (
      ownsRun(runId, conversationID) &&
      abortControllerRef.current === controller
    ) {
      abortControllerRef.current = undefined;
      setAbortable(false);
    }
    if (completedCommand)
      await new Promise<void>((resolve) => window.setTimeout(resolve, 360));
    try {
      await refresh(conversationID, runId);
    } catch (error) {
      if (ownsRun(runId, conversationID))
        setRefreshError(
          error instanceof Error
            ? error.message
            : "Unable to refresh conversation",
        );
    } finally {
      finishRun(runId, conversationID);
    }
  };

  const submit = () => {
    const prompt = draft.trim();
    if (
      !prompt ||
      !threadId ||
      !conversation ||
      conversationLoading ||
      activeRunIdRef.current !== undefined ||
      isThreadMutationPending() ||
      conversation.archived_at
    )
      return;
    const runId = ++nextRunIdRef.current;
    activeRunIdRef.current = runId;
    activeRunConversationIdRef.current = threadId;
    setGenerationLocked(true);
    setRunning(true);
    setAssistantLoading(true);
    setLatestAnnouncement("");
    setTurnError(undefined);
    setRefreshError(undefined);
    setDraft("");
    setLiveItems([
      {
        type: "message",
        message: {
          role: "user",
          content: prompt,
          created_at: new Date().toISOString(),
        },
      },
    ]);
    void runAgent(runId, threadId, [{ role: "user", content: prompt }]);
  };

  const retryTurn = () => {
    if (
      !threadId ||
      activeRunIdRef.current !== undefined ||
      isThreadMutationPending() ||
      conversation?.archived_at
    )
      return;
    const runId = ++nextRunIdRef.current;
    activeRunIdRef.current = runId;
    activeRunConversationIdRef.current = threadId;
    setGenerationLocked(true);
    setRunning(true);
    setAssistantLoading(true);
    setLatestAnnouncement("");
    setTurnError(undefined);
    setRefreshError(undefined);
    setLiveAssistant(undefined);
    void runAgent(runId, threadId, []);
  };

  const recoverRefresh = () => {
    if (
      !threadId ||
      activeRunIdRef.current !== undefined ||
      isThreadMutationPending()
    )
      return;
    const runId = ++nextRunIdRef.current;
    activeRunIdRef.current = runId;
    activeRunConversationIdRef.current = threadId;
    setGenerationLocked(true);
    setRunning(true);
    setRefreshError(undefined);
    void refresh(threadId, runId)
      .catch((error) => {
        if (ownsRun(runId, threadId))
          setRefreshError(
            error instanceof Error
              ? error.message
              : "Unable to refresh conversation",
          );
      })
      .finally(() => finishRun(runId, threadId));
  };

  const stop = () => {
    if (!abortable) return;
    abortControllerRef.current?.abort();
    clearLoading(activeRunIdRef.current, activeRunConversationIdRef.current);
    setLiveAssistant(undefined);
  };

  const restore = () => {
    if (
      !conversation ||
      conversation.archived_at === undefined ||
      restorePending
    )
      return;
    const token = ++restoreTokenRef.current;
    const archivedPath = `/agent/${encodeURIComponent(conversation.id)}`;
    setRestorePending(true);
    void restoreThread(conversation.id)
      .then((restored) => {
        if (
          !restored ||
          token !== restoreTokenRef.current ||
          currentThreadIdRef.current !== conversation.id ||
          pathnameRef.current !== archivedPath
        )
          return;
        setConversation(restored);
        setArchivedTurnNotice(undefined);
        navigate(localVisualPreviewPath(archivedPath), { replace: true });
      })
      .finally(() => {
        if (token === restoreTokenRef.current) setRestorePending(false);
      });
  };

  const archived = Boolean(conversation?.archived_at);
  const canRecoverLoad = conversationErrorStatus === 401 || conversationErrorStatus === 404;
  const transcript = conversation
    ? reconstructTranscript(conversation.messages, sensitiveOutputsByToolCall)
    : [];
  const visibleItems = [...transcript, ...liveItems];
  const showPendingAssistant =
    assistantLoading &&
    !liveAssistant &&
    activeRunConversationIdRef.current === threadId &&
    currentThreadIdRef.current === threadId;

  return (
    <PageShell
      className="agent-chat-shell"
      eyebrow="Agent"
      title="Chat"
      description={
        archived
          ? "This thread is archived and read-only."
          : window.matchMedia("(min-width: 701px)").matches
            ? "Run fleet and browser tasks with the Cua agent."
            : undefined
      }
      primaryAction={
        archived ? (
          <Button
            disabled={restorePending || generationLocked || threadMutationLocked}
            loading={restorePending}
            onClick={restore}
          >
            Restore thread
          </Button>
        ) : undefined
      }
    >
      <div className="agent-chat-page" ref={pageRef}>
        <div className="agent-chat-main">
          <div aria-live="polite" className="agent-chat-announcements">
            {latestAnnouncement ||
              (running
                ? "Generating response"
                : conversationLoading
                  ? "Loading conversation"
                  : conversation
                    ? "Conversation loaded"
                    : "No conversation selected")}
          </div>
          <section
            aria-label="Chat"
            className="agent-chat-transcript"
            ref={transcriptRef}
          >
            <SpaceBetween size="m">
              {archived && (
                <Alert type="info">
                  {archivedTurnNotice ??
                    "This thread is archived and read-only."}
                </Alert>
              )}
              {conversationError && (
                <Alert
                  action={
                    canRecoverLoad ? (
                      <Button onClick={() => void recoverToActiveThread()}>
                        Open newest thread
                      </Button>
                    ) : undefined
                  }
                  type="error"
                >
                  {conversationError}
                </Alert>
              )}
              {conversationLoading ? (
                <>
                  <Skeleton kind="message" />
                  <Skeleton kind="message" />
                </>
              ) : !conversation ? (
                <div className="agent-chat-empty">
                  <Header variant="h2">Start a conversation</Header>
                  <Box color="text-body-secondary">
                    Choose a thread from navigation or create a new thread to
                    begin.
                  </Box>
                </div>
              ) : visibleItems.length === 0 && !liveAssistant && !showPendingAssistant ? (
                <div className="agent-chat-empty">
                  <Header variant="h2">
                    {archived
                      ? "No messages in this archived thread"
                      : "Start the conversation"}
                  </Header>
                  <Box color="text-body-secondary">
                    {archived
                      ? "Restore this thread to continue it."
                      : "Ask the agent to inspect or change your Cua environment."}
                  </Box>
                </div>
              ) : (
                visibleItems.map((item, index) =>
                  item.type === "message" ? (
                    <MessageBubble
                      key={`message-${index}`}
                      message={item.message}
                      position={index + 1}
                    />
                  ) : (
                    <CommandStatus
                      command={item.command}
                      key={item.command.id}
                    />
                  ),
                )
              )}
              {showPendingAssistant && (
                <MessageBubble
                  loading
                  message={{
                    role: "assistant",
                    content: "",
                    created_at: new Date().toISOString(),
                  }}
                  position={visibleItems.length + 1}
                />
              )}
              {liveAssistant && (
                <MessageBubble
                  loading
                  message={liveAssistant}
                  position={visibleItems.length + 1}
                />
              )}
              {turnError && (
                <div role="alert">
                  <Alert
                    action={<Button onClick={retryTurn}>Retry</Button>}
                    type="error"
                  >
                    {turnError}
                  </Alert>
                </div>
              )}
              {refreshError && (
                <div role="alert">
                  <Alert
                    action={
                      <Button onClick={recoverRefresh}>
                        Refresh conversation
                      </Button>
                    }
                    type="error"
                  >
                    {refreshError}
                  </Alert>
                </div>
              )}
            </SpaceBetween>
          </section>
          {!archived && (
            <div className="agent-chat-composer">
              <div className="agent-chat-composer-surface">
                <PromptInput
                  className="agent-chat-prompt"
                  ariaLabel="Ask a question"
                  disabled={!threadId || !conversation || conversationLoading || threadMutationLocked}
                  onChange={({ detail }) => setDraft(detail.value)}
                  onKeyDown={(event) => {
                    if (
                      event.detail.key === "Enter" &&
                      !event.detail.shiftKey
                    ) {
                      event.preventDefault();
                      submit();
                    }
                  }}
                  placeholder="Ask a question"
                  value={draft}
                />
                {running && abortable ? (
                  <Button
                    ariaLabel="Stop generating"
                    iconName="status-negative"
                    onClick={stop}
                  />
                ) : (
                  <Button
                    ariaLabel="Send message"
                    disabled={
                      !draft.trim() ||
                      !threadId ||
                      !conversation ||
                      conversationLoading ||
                      threadMutationLocked ||
                      running
                    }
                    iconName="send"
                    onClick={submit}
                  />
                )}
              </div>
            </div>
          )}
        </div>
      </div>
    </PageShell>
  );
}

import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import Alert from "@cloudscape-design/components/alert";
import Box from "@cloudscape-design/components/box";
import Button from "@cloudscape-design/components/button";
import Header from "@cloudscape-design/components/header";
import SpaceBetween from "@cloudscape-design/components/space-between";
import { displayThreadTitle, useChatThreads } from "../chat/ChatThreadsContext";
import { localVisualPreviewPath } from "../local-visual-preview";
import { listConversations, type ConversationSummary } from "../api/chat";
import { PageShell } from "../components/PageShell";
import "./AgentChat.css";

function formatArchivedAt(timestamp?: string): string {
  return timestamp
    ? new Date(timestamp).toLocaleString()
    : "Unknown archive time";
}

export function ArchivedThreads() {
  const navigate = useNavigate();
  const { generationLocked, restoreThread, threadMutationLocked } = useChatThreads();
  const [threads, setThreads] = useState<ConversationSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const [restoringId, setRestoringId] = useState<string>();
  const requestIdRef = useRef(0);

  const load = useCallback(() => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(undefined);
    void listConversations({ archived: true })
      .then((items) => {
        if (requestId !== requestIdRef.current) return;
        setThreads(items);
      })
      .catch((loadError) => {
        if (requestId !== requestIdRef.current) return;
        setError(
          loadError instanceof Error
            ? loadError.message
            : "Unable to load archived threads",
        );
      })
      .finally(() => {
        if (requestId === requestIdRef.current) setLoading(false);
      });
  }, []);

  useEffect(() => {
    load();
    return () => {
      requestIdRef.current += 1;
    };
  }, [load]);

  const restore = (thread: ConversationSummary) => {
    if (generationLocked || threadMutationLocked || restoringId) return;
    setRestoringId(thread.id);
    void restoreThread(thread.id)
      .then((restored) => {
        if (!restored) return;
        setThreads((current) =>
          current.filter((item) => item.id !== thread.id),
        );
      })
      .finally(() =>
        setRestoringId((current) =>
          current === thread.id ? undefined : current,
        ),
      );
  };

  return (
    <PageShell
      eyebrow="Agent"
      title="Archived threads"
      description="Read or restore conversations that are no longer active."
    >
      <SpaceBetween size="l">
        {error && (
          <Alert action={<Button onClick={load}>Retry</Button>} type="error">
            {error}
          </Alert>
        )}
        {loading ? (
          <SpaceBetween size="s">
            <div
              className="agent-chat-skeleton agent-chat-skeleton--message"
              data-testid="archived-thread-skeleton"
            />
            <div
              className="agent-chat-skeleton agent-chat-skeleton--message"
              data-testid="archived-thread-skeleton"
            />
          </SpaceBetween>
        ) : threads.length === 0 ? (
          <Box color="text-body-secondary">No archived threads.</Box>
        ) : (
          <SpaceBetween size="m">
            {threads.map((thread) => (
              <section key={thread.id}>
                <SpaceBetween direction="horizontal" size="s">
                  <Header variant="h2">
                    <a
                      href={`#${localVisualPreviewPath(`/agent/${encodeURIComponent(thread.id)}`)}`}
                      title={displayThreadTitle(thread.title)}
                      onClick={(event) => {
                        event.preventDefault();
                        navigate(
                          localVisualPreviewPath(
                            `/agent/${encodeURIComponent(thread.id)}`,
                          ),
                        );
                      }}
                    >
                      {displayThreadTitle(thread.title)}
                    </a>
                  </Header>
                  <Button
                    ariaLabel={`Restore ${displayThreadTitle(thread.title)}`}
                    disabled={
                      generationLocked || threadMutationLocked || Boolean(restoringId)
                    }
                    loading={restoringId === thread.id}
                    onClick={() => restore(thread)}
                  >
                    Restore
                  </Button>
                </SpaceBetween>
                <Box color="text-body-secondary" fontSize="body-s">
                  Archived {formatArchivedAt(thread.archived_at)}
                </Box>
              </section>
            ))}
          </SpaceBetween>
        )}
      </SpaceBetween>
    </PageShell>
  );
}

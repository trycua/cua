import { Loader2, Play, X } from 'lucide-react';
import { memo, useCallback, useEffect, useRef, useState } from 'react';
import TrajectoryViewer from '../TrajectoryViewer';
import { usePlaygroundTelemetry } from '../../telemetry';
import { inferRuns } from '../../utils/trajectory';
import type { AgentMessage } from '../../types';

// Memoized wrapper to prevent TrajectoryViewer from re-rendering due to parent state changes
const MemoizedTrajectoryViewer = memo(TrajectoryViewer);

interface ReplayTrajectoryModalProps {
  isOpen: boolean;
  onClose: () => void;
  sessionId: number;
  chatTitle?: string;
  /** Optional: directly pass messages to avoid refetching */
  messages?: AgentMessage[];
  /** Optional: model ID for display */
  modelId?: string;
}

interface RunSummary {
  index: number;
  userPromptPreview: string;
  screenshotCount: number;
  actionCount: number;
  isEmpty: boolean;
  messageCount: number;
}

interface ReplayTurn {
  turnName: string;
  screenshotSignedUrl: string | null;
  agentResponseJson: string;
}

interface ReplayResponse {
  trajectoryFolder: string;
  turns: ReplayTurn[];
}

export default function ReplayTrajectoryModal({
  isOpen,
  onClose,
  sessionId,
  chatTitle,
  messages: propMessages,
  modelId,
}: ReplayTrajectoryModalProps) {
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [selectedRunIndex, setSelectedRunIndex] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [trajectoryFiles, setTrajectoryFiles] = useState<File[] | null>(null);
  const [generatingTrajectory, setGeneratingTrajectory] = useState(false);
  const [trajectoryKey, setTrajectoryKey] = useState(0);
  const [title, setTitle] = useState(chatTitle || 'Chat Replay');
  const { trackTrajectoryReplayed } = usePlaygroundTelemetry();

  // Track if we've already initialized to prevent re-runs
  const hasInitializedRef = useRef(false);
  // Track the current loaded run to prevent re-generation
  const loadedRunIndexRef = useRef<number | null>(null);

  // Fetch run information - only run once when modal opens
  useEffect(() => {
    if (!isOpen || hasInitializedRef.current) return;

    const fetchRuns = async () => {
      setLoading(true);
      hasInitializedRef.current = true;

      try {
        // If messages are passed directly, infer runs locally
        if (propMessages && propMessages.length > 0) {
          const inferredRuns = inferRuns(propMessages);
          setRuns(
            inferredRuns.map((run) => ({
              index: run.index,
              userPromptPreview: run.userPromptPreview,
              screenshotCount: run.screenshotCount,
              actionCount: run.actionCount,
              isEmpty: run.isEmpty,
              messageCount: run.messages.length,
            }))
          );
          // Auto-select first run with content
          const firstNonEmpty = inferredRuns.find((r) => !r.isEmpty);
          if (firstNonEmpty) {
            setSelectedRunIndex(firstNonEmpty.index);
          } else if (inferredRuns.length > 0) {
            setSelectedRunIndex(inferredRuns[0].index);
          }
        } else {
          // Fetch from API
          const response = await fetch(`/api/chat/${sessionId}/export`);
          if (!response.ok) {
            throw new Error('Failed to fetch run information');
          }

          const data = await response.json();
          setRuns(data.runs || []);
          setTitle(data.chatTitle || 'Chat Replay');

          // Auto-select first run with content
          const firstNonEmpty = data.runs?.find(
            (r: RunSummary) => !r.isEmpty && r.screenshotCount > 0
          );
          if (firstNonEmpty) {
            setSelectedRunIndex(firstNonEmpty.index);
          } else if (data.runs?.length > 0) {
            setSelectedRunIndex(data.runs[0].index);
          }
        }
      } catch (error) {
        console.error('Failed to fetch runs:', error);
      } finally {
        setLoading(false);
      }
    };

    fetchRuns();
  }, [isOpen, sessionId, propMessages]);

  // Reset initialization when modal closes
  useEffect(() => {
    if (!isOpen) {
      hasInitializedRef.current = false;
      loadedRunIndexRef.current = null;
    }
  }, [isOpen]);

  // Generate trajectory files for the selected run
  const generateTrajectoryForRun = useCallback(
    async (runIndex: number) => {
      // Prevent re-generating if already loaded for this run
      if (loadedRunIndexRef.current === runIndex) return;

      setGeneratingTrajectory(true);
      setTrajectoryFiles(null);

      try {
        const response = await fetch(`/api/chat/${sessionId}/export`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ runIndices: [runIndex], format: 'replay' }),
        });

        if (!response.ok) {
          throw new Error('Failed to generate trajectory');
        }

        const data = await response.json();
        // withErrorHandling wraps in { result, _meta }
        const replay: ReplayResponse = data.result ?? data;

        // Fetch all screenshots in parallel from signed URLs
        const screenshotResults = await Promise.allSettled(
          replay.turns.map(async (turn) => {
            if (!turn.screenshotSignedUrl) return null;
            const imgResponse = await fetch(turn.screenshotSignedUrl);
            if (!imgResponse.ok) return null;
            return imgResponse.blob();
          })
        );

        // Build File[] array with proper webkitRelativePath
        const files: File[] = [];
        for (let i = 0; i < replay.turns.length; i++) {
          const turn = replay.turns[i];
          const turnPath = `${replay.trajectoryFolder}/${turn.turnName}`;

          // Add screenshot file if fetched successfully
          const screenshotResult = screenshotResults[i];
          if (screenshotResult.status === 'fulfilled' && screenshotResult.value) {
            const screenshotFile = new File([screenshotResult.value], 'screenshot.png', {
              type: 'image/png',
            });
            Object.defineProperty(screenshotFile, 'webkitRelativePath', {
              value: `${turnPath}/screenshot.png`,
              writable: false,
            });
            files.push(screenshotFile);
          }

          // Add agent_response.json file
          const jsonFile = new File(
            [turn.agentResponseJson],
            `${turn.turnName}_agent_response.json`,
            { type: 'application/json' }
          );
          Object.defineProperty(jsonFile, 'webkitRelativePath', {
            value: `${turnPath}/${turn.turnName}_agent_response.json`,
            writable: false,
          });
          files.push(jsonFile);
        }

        loadedRunIndexRef.current = runIndex;
        setTrajectoryFiles(files);
        setTrajectoryKey((prev) => prev + 1);
        trackTrajectoryReplayed({ runIndex });
      } catch (error) {
        console.error('Failed to generate trajectory:', error);
      } finally {
        setGeneratingTrajectory(false);
      }
    },
    [sessionId, trackTrajectoryReplayed]
  );

  // Load trajectory when run is selected
  useEffect(() => {
    if (selectedRunIndex !== null && !loading && loadedRunIndexRef.current !== selectedRunIndex) {
      generateTrajectoryForRun(selectedRunIndex);
    }
  }, [selectedRunIndex, loading, generateTrajectoryForRun]);

  if (!isOpen) return null;

  const selectedRun = runs.find((r) => r.index === selectedRunIndex);

  // Use compact size for loading, empty, or no-screenshot states
  const hasScreenshots = selectedRun && selectedRun.screenshotCount > 0 && trajectoryFiles;
  const isCompactView = loading || runs.length === 0 || generatingTrajectory || !hasScreenshots;

  const modalSizeClass = isCompactView ? 'w-full max-w-md' : 'h-[90vh] w-[95vw] max-w-7xl';

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div
        className={`relative flex flex-col overflow-hidden rounded-lg bg-white shadow-xl dark:bg-neutral-900 ${modalSizeClass}`}
      >
        {/* Header */}
        <div className="flex items-center justify-between border-neutral-200 border-b px-4 py-3 dark:border-neutral-700">
          <div className="flex items-center gap-3">
            <Play className="h-5 w-5 text-neutral-600 dark:text-neutral-400" />
            <h2 className="font-semibold text-lg text-neutral-900 dark:text-white">
              Replay: {title}
            </h2>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded-full p-1.5 text-neutral-500 transition-colors hover:bg-neutral-100 hover:text-neutral-700 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-200"
          >
            <X className="h-5 w-5" />
          </button>
        </div>

        {/* Run selector tabs */}
        {runs.length > 1 && (
          <div className="flex gap-1 overflow-x-auto border-neutral-200 border-b bg-neutral-50 px-4 py-2 dark:border-neutral-700 dark:bg-neutral-800/50">
            {runs.map((run) => (
              <button
                key={run.index}
                type="button"
                onClick={() => setSelectedRunIndex(run.index)}
                disabled={generatingTrajectory}
                className={`flex-shrink-0 rounded-md px-3 py-1.5 text-sm transition-colors ${
                  selectedRunIndex === run.index
                    ? 'bg-blue-500 text-white'
                    : 'bg-white text-neutral-700 hover:bg-neutral-100 dark:bg-neutral-700 dark:text-neutral-300 dark:hover:bg-neutral-600'
                } ${run.isEmpty || run.screenshotCount === 0 ? 'opacity-50' : ''}`}
                title={run.userPromptPreview}
              >
                <span className="max-w-[150px] truncate">
                  Run {run.index + 1}: {run.userPromptPreview || 'Empty'}
                </span>
              </button>
            ))}
          </div>
        )}

        {/* Main content */}
        <div className={isCompactView ? 'p-6' : 'flex-1 overflow-hidden'}>
          {loading ? (
            <div className="flex items-center justify-center py-8">
              <div className="flex flex-col items-center gap-3">
                <Loader2 className="h-8 w-8 animate-spin text-blue-500" />
                <p className="text-neutral-600 text-sm dark:text-neutral-400">
                  Loading trajectory data...
                </p>
              </div>
            </div>
          ) : runs.length === 0 ? (
            <div className="flex items-center justify-center py-8">
              <p className="text-neutral-500 dark:text-neutral-400">
                No trajectory runs found in this chat.
              </p>
            </div>
          ) : generatingTrajectory ? (
            <div className="flex items-center justify-center py-8">
              <div className="flex flex-col items-center gap-3">
                <Loader2 className="h-8 w-8 animate-spin text-blue-500" />
                <p className="text-neutral-600 text-sm dark:text-neutral-400">
                  Preparing trajectory for replay...
                </p>
              </div>
            </div>
          ) : selectedRun && selectedRun.screenshotCount === 0 ? (
            <div className="flex items-center justify-center py-8">
              <div className="text-center">
                <p className="text-neutral-500 dark:text-neutral-400">
                  This run has no screenshots to replay.
                </p>
                <p className="mt-1 text-neutral-400 text-sm dark:text-neutral-500">
                  {selectedRun.actionCount} action
                  {selectedRun.actionCount !== 1 ? 's' : ''} recorded
                </p>
              </div>
            </div>
          ) : trajectoryFiles ? (
            <MemoizedTrajectoryViewer
              key={trajectoryKey}
              files={trajectoryFiles}
              showToolbar={true}
              scroll={true}
              scenarioName={selectedRun?.userPromptPreview || 'Replay'}
              operatingSystem="linux"
              autoLoop={false}
              userPrompt={selectedRun?.userPromptPreview}
              modelOverride={modelId}
            />
          ) : null}
        </div>

        {/* Footer with run info - only show when viewing actual trajectory */}
        {selectedRun && !loading && !isCompactView && (
          <div className="flex items-center justify-between border-neutral-200 border-t bg-neutral-50 px-4 py-2 text-xs dark:border-neutral-700 dark:bg-neutral-800/50">
            <span className="text-neutral-500 dark:text-neutral-400">
              {selectedRun.screenshotCount} screenshot
              {selectedRun.screenshotCount !== 1 ? 's' : ''} &middot; {selectedRun.actionCount}{' '}
              action
              {selectedRun.actionCount !== 1 ? 's' : ''}
            </span>
            <span className="text-neutral-400 dark:text-neutral-500">
              Run {(selectedRun.index || 0) + 1} of {runs.length}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}

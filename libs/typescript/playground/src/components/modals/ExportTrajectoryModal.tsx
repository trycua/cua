import { Download, ImageIcon, Loader2, MousePointerClick } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import { Modal } from '../ui/Modal';
import { Button } from '../ui/button';
import { usePlaygroundTelemetry } from '../../telemetry';

interface RunSummary {
  index: number;
  userPromptPreview: string;
  userPrompt: string;
  screenshotCount: number;
  actionCount: number;
  isEmpty: boolean;
  messageCount: number;
}

interface ExportTrajectoryModalProps {
  isOpen: boolean;
  onClose: () => void;
  sessionId: number;
}

export default function ExportTrajectoryModal({
  isOpen,
  onClose,
  sessionId,
}: ExportTrajectoryModalProps) {
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [chatTitle, setChatTitle] = useState<string>('');
  const [selectedIndices, setSelectedIndices] = useState<Set<number>>(new Set());
  const [loading, setLoading] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { trackTrajectoryExported } = usePlaygroundTelemetry();

  // Fetch run information when modal opens
  useEffect(() => {
    if (!isOpen || !sessionId) return;

    const fetchRuns = async () => {
      setLoading(true);
      setError(null);

      try {
        const response = await fetch(`/api/chat/${sessionId}/export`);
        const data = await response.json();

        if (!response.ok || !data.success) {
          throw new Error(data.error || 'Failed to fetch trajectory runs');
        }

        setRuns(data.runs);
        setChatTitle(data.chatTitle);
        // Select all by default
        setSelectedIndices(new Set(data.runs.map((r: RunSummary) => r.index)));
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Failed to load runs');
      } finally {
        setLoading(false);
      }
    };

    fetchRuns();
  }, [isOpen, sessionId]);

  const toggleRun = useCallback((index: number) => {
    setSelectedIndices((prev) => {
      const next = new Set(prev);
      if (next.has(index)) {
        next.delete(index);
      } else {
        next.add(index);
      }
      return next;
    });
  }, []);

  const selectAll = useCallback(() => {
    setSelectedIndices(new Set(runs.map((r) => r.index)));
  }, [runs]);

  const deselectAll = useCallback(() => {
    setSelectedIndices(new Set());
  }, []);

  const handleExport = async () => {
    if (selectedIndices.size === 0) return;

    setExporting(true);
    setError(null);

    try {
      const response = await fetch(`/api/chat/${sessionId}/export`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          runIndices: Array.from(selectedIndices).sort(),
        }),
      });

      if (!response.ok) {
        const data = await response.json().catch(() => ({}));
        throw new Error(data.error || 'Export failed');
      }

      // Download the ZIP file
      const blob = await response.blob();
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download =
        response.headers.get('Content-Disposition')?.split('filename="')[1]?.replace('"', '') ||
        'trajectory_export.zip';
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);

      trackTrajectoryExported({ runCount: selectedIndices.size });
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Export failed');
    } finally {
      setExporting(false);
    }
  };

  return (
    <Modal isOpen={isOpen} onClose={onClose} contentClassName="max-w-lg">
      <div className="flex flex-col">
        {/* Header */}
        <div className="mb-4">
          <h2 className="font-bold text-neutral-900 text-xl dark:text-white">Export Trajectory</h2>
          {chatTitle && (
            <p className="mt-1 text-neutral-600 text-sm dark:text-neutral-400">{chatTitle}</p>
          )}
        </div>

        {/* Content */}
        {loading ? (
          <div className="flex items-center justify-center py-8">
            <Loader2 className="h-6 w-6 animate-spin text-neutral-500" />
            <span className="ml-2 text-neutral-500">Loading runs...</span>
          </div>
        ) : error ? (
          <div className="rounded-md bg-red-50 p-4 text-red-600 text-sm dark:bg-red-900/20 dark:text-red-400">
            {error}
          </div>
        ) : runs.length === 0 ? (
          <div className="py-8 text-center text-neutral-500">
            No runs found in this chat session.
          </div>
        ) : (
          <>
            <p className="mb-3 text-neutral-600 text-sm dark:text-neutral-400">
              Select runs to export:
            </p>

            {/* Run list */}
            <div className="max-h-64 space-y-2 overflow-y-auto rounded-md border border-neutral-200 p-2 dark:border-neutral-700">
              {runs.map((run) => (
                <label
                  key={run.index}
                  className="flex cursor-pointer items-start gap-3 rounded-md p-2 transition-colors hover:bg-neutral-50 dark:hover:bg-neutral-700/50"
                >
                  <input
                    type="checkbox"
                    checked={selectedIndices.has(run.index)}
                    onChange={() => toggleRun(run.index)}
                    className="mt-1 h-4 w-4 rounded border-neutral-300 text-blue-600 focus:ring-blue-500 dark:border-neutral-600"
                  />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="font-medium text-neutral-900 text-sm dark:text-white">
                        Run {run.index + 1}
                      </span>
                      {run.isEmpty && (
                        <span className="rounded bg-neutral-100 px-1.5 py-0.5 text-neutral-500 text-xs dark:bg-neutral-700 dark:text-neutral-400">
                          Empty
                        </span>
                      )}
                    </div>
                    <p className="mt-0.5 truncate text-neutral-600 text-sm dark:text-neutral-400">
                      {run.userPromptPreview || '[No prompt]'}
                    </p>
                    <div className="mt-1 flex items-center gap-3 text-neutral-500 text-xs dark:text-neutral-500">
                      <span className="flex items-center gap-1">
                        <MousePointerClick className="h-3 w-3" />
                        {run.actionCount} actions
                      </span>
                      <span className="flex items-center gap-1">
                        <ImageIcon className="h-3 w-3" />
                        {run.screenshotCount} screenshots
                      </span>
                    </div>
                  </div>
                </label>
              ))}
            </div>

            {/* Selection controls */}
            <div className="mt-3 flex gap-2">
              <button
                type="button"
                onClick={selectAll}
                className="text-blue-600 text-sm hover:underline dark:text-blue-400"
              >
                Select All
              </button>
              <span className="text-neutral-300 dark:text-neutral-600">|</span>
              <button
                type="button"
                onClick={deselectAll}
                className="text-blue-600 text-sm hover:underline dark:text-blue-400"
              >
                Deselect All
              </button>
              <span className="ml-auto text-neutral-500 text-sm">
                {selectedIndices.size} of {runs.length} selected
              </span>
            </div>
          </>
        )}

        {/* Footer */}
        <div className="mt-6 flex justify-end gap-3">
          <Button variant="outline" onClick={onClose} disabled={exporting}>
            Cancel
          </Button>
          <Button
            onClick={handleExport}
            disabled={loading || exporting || selectedIndices.size === 0}
          >
            {exporting ? (
              <>
                <Loader2 className="h-4 w-4 animate-spin" />
                Exporting...
              </>
            ) : (
              <>
                <Download className="h-4 w-4" />
                Export ZIP
              </>
            )}
          </Button>
        </div>
      </div>
    </Modal>
  );
}

// Step 2b: Local Docker configuration form
// Shows docker command, name/host fields, test connection, and add button.

import { Check, Copy } from 'lucide-react';
import type { SandboxPreset } from '../../constants/sandboxPresets';
import { TestConnectionButton, type TestStatus } from './TestConnectionButton';

export interface LocalDockerFormProps {
  preset: SandboxPreset;
  name: string;
  onNameChange: (v: string) => void;
  host: string;
  onHostChange: (v: string) => void;
  onCopyCommand: (cmd: string) => void;
  copiedCommand: boolean;
  testStatus: TestStatus;
  onTestConnection: () => void;
  onAdd: () => void;
  isLoading: boolean;
  error: string | null;
}

export function LocalDockerForm({
  preset,
  name,
  onNameChange,
  host,
  onHostChange,
  onCopyCommand,
  copiedCommand,
  testStatus,
  onTestConnection,
  onAdd,
  isLoading,
  error,
}: LocalDockerFormProps) {
  return (
    <div className="flex flex-col gap-4">
      {/* Docker command */}
      <div>
        <label className="mb-1.5 block text-sm font-medium text-neutral-700 dark:text-neutral-300">
          1. Run this Docker command
        </label>
        <div className="group relative">
          <pre className="overflow-x-auto rounded-md bg-neutral-900 p-3 text-xs text-green-400">
            <code>{preset.dockerCommand}</code>
          </pre>
          <button
            type="button"
            onClick={() => onCopyCommand(preset.dockerCommand)}
            className="absolute right-2 top-2 rounded-md bg-neutral-700 p-1.5 text-neutral-300 opacity-0 transition-opacity hover:bg-neutral-600 group-hover:opacity-100"
            title="Copy command"
          >
            {copiedCommand ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
          </button>
        </div>
        <p className="mt-1 text-xs text-neutral-500 dark:text-neutral-400">
          Prerequisites: {preset.prerequisites}
        </p>
      </div>

      {/* Name */}
      <div>
        <label
          htmlFor="local-name"
          className="mb-1.5 block text-sm font-medium text-neutral-700 dark:text-neutral-300"
        >
          2. Configure connection
        </label>
        <div className="flex flex-col gap-3">
          <div>
            <label
              htmlFor="local-sandbox-name"
              className="mb-1 block text-xs text-neutral-500 dark:text-neutral-400"
            >
              Name
            </label>
            <input
              id="local-sandbox-name"
              type="text"
              value={name}
              onChange={(e) => onNameChange(e.target.value)}
              className="w-full rounded-md border border-neutral-300 px-3 py-2 text-sm dark:border-neutral-600 dark:bg-neutral-700 dark:text-white"
              disabled={isLoading}
            />
          </div>
          <div>
            <label
              htmlFor="local-sandbox-host"
              className="mb-1 block text-xs text-neutral-500 dark:text-neutral-400"
            >
              Host
            </label>
            <input
              id="local-sandbox-host"
              type="text"
              value={host}
              onChange={(e) => onHostChange(e.target.value)}
              placeholder="localhost"
              className="w-full rounded-md border border-neutral-300 px-3 py-2 text-sm dark:border-neutral-600 dark:bg-neutral-700 dark:text-white"
              disabled={isLoading}
            />
          </div>
        </div>
        <p className="mt-2 text-xs text-neutral-500 dark:text-neutral-400">
          Agent URL:{' '}
          <code className="text-neutral-700 dark:text-neutral-300">
            http://{host}:{preset.apiPort}
          </code>
          {' | '}
          VNC URL:{' '}
          <code className="text-neutral-700 dark:text-neutral-300">
            http://{host}:{preset.vncPort}
            {preset.vncPath}
          </code>
        </p>
      </div>

      {error && <p className="text-sm text-red-600 dark:text-red-400">{error}</p>}

      {/* Actions */}
      <div className="flex items-center justify-end gap-3 border-t border-neutral-200 pt-4 dark:border-neutral-700">
        <TestConnectionButton status={testStatus} onClick={onTestConnection} />
        <button
          type="button"
          onClick={onAdd}
          disabled={isLoading}
          className="rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700 disabled:opacity-50"
        >
          {isLoading ? 'Adding...' : 'Add'}
        </button>
      </div>
    </div>
  );
}

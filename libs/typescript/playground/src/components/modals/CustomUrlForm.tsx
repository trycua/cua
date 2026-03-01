// Step 2c: Custom URL form
// Simple form with name, agent URL, VNC URL fields, test connection, and add button.

import { TestConnectionButton, type TestStatus } from './TestConnectionButton';

export interface CustomUrlFormProps {
  name: string;
  onNameChange: (v: string) => void;
  agentUrl: string;
  onAgentUrlChange: (v: string) => void;
  vncUrl: string;
  onVncUrlChange: (v: string) => void;
  testStatus: TestStatus;
  onTestConnection: () => void;
  onAdd: () => void;
  isLoading: boolean;
  error: string | null;
}

export function CustomUrlForm({
  name,
  onNameChange,
  agentUrl,
  onAgentUrlChange,
  vncUrl,
  onVncUrlChange,
  testStatus,
  onTestConnection,
  onAdd,
  isLoading,
  error,
}: CustomUrlFormProps) {
  return (
    <div className="flex flex-col gap-4">
      <div>
        <label
          htmlFor="custom-name"
          className="mb-1 block text-sm font-medium text-neutral-700 dark:text-neutral-300"
        >
          Name
        </label>
        <input
          id="custom-name"
          type="text"
          value={name}
          onChange={(e) => onNameChange(e.target.value)}
          placeholder="My Computer"
          className="w-full rounded-md border border-neutral-300 px-3 py-2 text-sm dark:border-neutral-600 dark:bg-neutral-700 dark:text-white"
          disabled={isLoading}
        />
      </div>

      <div>
        <label
          htmlFor="custom-agent-url"
          className="mb-1 block text-sm font-medium text-neutral-700 dark:text-neutral-300"
        >
          Agent URL
        </label>
        <input
          id="custom-agent-url"
          type="text"
          value={agentUrl}
          onChange={(e) => onAgentUrlChange(e.target.value)}
          placeholder="http://localhost:8000"
          className="w-full rounded-md border border-neutral-300 px-3 py-2 text-sm dark:border-neutral-600 dark:bg-neutral-700 dark:text-white"
          disabled={isLoading}
        />
      </div>

      <div>
        <label
          htmlFor="custom-vnc-url"
          className="mb-1 block text-sm font-medium text-neutral-700 dark:text-neutral-300"
        >
          VNC URL
        </label>
        <input
          id="custom-vnc-url"
          type="text"
          value={vncUrl}
          onChange={(e) => onVncUrlChange(e.target.value)}
          placeholder="http://localhost:6901/vnc.html"
          className="w-full rounded-md border border-neutral-300 px-3 py-2 text-sm dark:border-neutral-600 dark:bg-neutral-700 dark:text-white"
          disabled={isLoading}
        />
      </div>

      {error && <p className="text-sm text-red-600 dark:text-red-400">{error}</p>}

      <div className="flex items-center justify-end gap-3 border-t border-neutral-200 pt-4 dark:border-neutral-700">
        <TestConnectionButton status={testStatus} onClick={onTestConnection} disabled={!agentUrl} />
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

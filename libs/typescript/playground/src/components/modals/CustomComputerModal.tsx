// Add Sandbox wizard modal
// Supports three paths: Local Docker, Cloud Sandbox, Custom URL

import { useState, useCallback } from 'react';
import {
  ArrowLeft,
  Check,
  ChevronRight,
  Copy,
  Globe,
  Loader2,
  Monitor,
  Server,
} from 'lucide-react';
import { usePlayground } from '../../hooks/usePlayground';
import type { ComputerInfo } from '../../adapters/types';
import { cn } from '../../utils/cn';
import { Modal } from '../ui/Modal';
import { SANDBOX_PRESETS, MACOS_PRESET, type SandboxPreset } from '../../constants/sandboxPresets';

type SandboxSource = 'local' | 'cloud' | 'custom';
type WizardStep = 'source' | 'configure';

export interface AddSandboxModalProps {
  isOpen: boolean;
  onClose: () => void;
  onAdd?: (computer: ComputerInfo) => void;
  /** Render prop for cloud VM creation wizard. Injected by cloud website. */
  renderCloudCreate?: (props: { onCreated: () => void; onCancel: () => void }) => React.ReactNode;
  className?: string;
}

// Backward-compatible alias
export type CustomComputerModalProps = AddSandboxModalProps;

export function AddSandboxModal({
  isOpen,
  onClose,
  onAdd,
  renderCloudCreate,
  className,
}: AddSandboxModalProps) {
  const { adapters, dispatch } = usePlayground();

  const [step, setStep] = useState<WizardStep>('source');
  const [source, setSource] = useState<SandboxSource | null>(null);
  const [selectedPreset, setSelectedPreset] = useState<SandboxPreset | null>(null);

  // Local Docker form state
  const [localName, setLocalName] = useState('');
  const [localHost, setLocalHost] = useState('localhost');

  // Custom URL form state
  const [customName, setCustomName] = useState('');
  const [customAgentUrl, setCustomAgentUrl] = useState('');
  const [customVncUrl, setCustomVncUrl] = useState('');

  // Shared state
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [testStatus, setTestStatus] = useState<'idle' | 'testing' | 'success' | 'error'>('idle');
  const [copiedCommand, setCopiedCommand] = useState(false);

  const resetState = useCallback(() => {
    setStep('source');
    setSource(null);
    setSelectedPreset(null);
    setLocalName('');
    setLocalHost('localhost');
    setCustomName('');
    setCustomAgentUrl('');
    setCustomVncUrl('');
    setError(null);
    setTestStatus('idle');
    setIsLoading(false);
    setCopiedCommand(false);
  }, []);

  const handleClose = useCallback(() => {
    resetState();
    onClose();
  }, [onClose, resetState]);

  const handleSelectSource = useCallback((s: SandboxSource) => {
    setSource(s);
    setStep('configure');
    setError(null);
    setTestStatus('idle');
  }, []);

  const handleBack = useCallback(() => {
    if (step === 'configure' && source === 'local' && selectedPreset) {
      setSelectedPreset(null);
      setError(null);
      setTestStatus('idle');
      return;
    }
    setStep('source');
    setSource(null);
    setSelectedPreset(null);
    setError(null);
    setTestStatus('idle');
  }, [step, source, selectedPreset]);

  const handleSelectPreset = useCallback((preset: SandboxPreset) => {
    setSelectedPreset(preset);
    setLocalName(preset.name);
    setError(null);
    setTestStatus('idle');
  }, []);

  const handleCopyCommand = useCallback((command: string) => {
    navigator.clipboard.writeText(command).then(() => {
      setCopiedCommand(true);
      setTimeout(() => setCopiedCommand(false), 2000);
    });
  }, []);

  const buildAgentUrl = useCallback((preset: SandboxPreset, host: string) => {
    return `http://${host}:${preset.apiPort}`;
  }, []);

  const buildVncUrl = useCallback((preset: SandboxPreset, host: string) => {
    const base = `http://${host}:${preset.vncPort}`;
    return preset.vncPath ? `${base}${preset.vncPath}` : base;
  }, []);

  const handleTestConnection = useCallback(async (agentUrl: string) => {
    setTestStatus('testing');
    setError(null);
    try {
      const response = await fetch(`${agentUrl}/health`, {
        signal: AbortSignal.timeout(5000),
      });
      if (response.ok) {
        setTestStatus('success');
      } else {
        setTestStatus('error');
        setError(`Health check returned ${response.status}`);
      }
    } catch {
      setTestStatus('error');
      setError('Could not connect. Make sure the container is running.');
    }
  }, []);

  const handleAddComputer = useCallback(
    async (name: string, agentUrl: string, vncUrl: string) => {
      setError(null);

      if (!name.trim()) {
        setError('Please enter a name');
        return;
      }

      try {
        new URL(agentUrl);
      } catch {
        setError('Invalid agent URL');
        return;
      }

      setIsLoading(true);

      try {
        if (adapters.computer.addCustomComputer) {
          const newComputer = await adapters.computer.addCustomComputer({
            name: name.trim(),
            vncUrl,
            agentUrl,
            status: 'running',
            isCustom: true,
          });

          const computers = await adapters.computer.listComputers();
          dispatch({ type: 'SET_COMPUTERS', payload: computers });

          onAdd?.(newComputer);
          handleClose();
        } else {
          setError('Adding custom computers is not supported');
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Failed to add computer');
      } finally {
        setIsLoading(false);
      }
    },
    [adapters, dispatch, onAdd, handleClose]
  );

  const handleCloudCreated = useCallback(async () => {
    try {
      const computers = await adapters.computer.listComputers();
      dispatch({ type: 'SET_COMPUTERS', payload: computers });
    } catch (err) {
      console.error('Failed to refresh computers after cloud creation:', err);
    }
    handleClose();
  }, [adapters, dispatch, handleClose]);

  const showBackButton = step === 'configure';

  const title = (() => {
    if (step === 'source') return 'Add Sandbox';
    if (source === 'local' && !selectedPreset) return 'Choose Sandbox Type';
    if (source === 'local' && selectedPreset) return selectedPreset.name;
    if (source === 'cloud') return 'Create Cloud Sandbox';
    if (source === 'custom') return 'Custom URL';
    return 'Add Sandbox';
  })();

  return (
    <Modal
      isOpen={isOpen}
      onClose={handleClose}
      showCloseButton={!showBackButton}
      contentClassName={cn(
        'max-w-lg p-0',
        step === 'configure' && source === 'cloud' && 'max-w-2xl',
        className
      )}
    >
      {/* Header */}
      <div className="flex items-center gap-3 border-b border-neutral-200 px-6 py-4 dark:border-neutral-700">
        {showBackButton && (
          <button
            type="button"
            onClick={handleBack}
            className="rounded-md p-1 hover:bg-neutral-100 dark:hover:bg-neutral-700"
          >
            <ArrowLeft className="h-4 w-4 text-neutral-600 dark:text-neutral-400" />
          </button>
        )}
        <h2 className="flex-1 text-lg font-semibold text-neutral-900 dark:text-white">{title}</h2>
      </div>

      {/* Content */}
      <div className="overflow-y-auto px-6 py-4" style={{ maxHeight: '70vh' }}>
        {step === 'source' && (
          <SourceStep onSelect={handleSelectSource} hasCloud={!!renderCloudCreate} />
        )}

        {step === 'configure' && source === 'local' && !selectedPreset && (
          <PresetGrid onSelect={handleSelectPreset} />
        )}

        {step === 'configure' && source === 'local' && selectedPreset && (
          <LocalDockerForm
            preset={selectedPreset}
            name={localName}
            onNameChange={setLocalName}
            host={localHost}
            onHostChange={setLocalHost}
            onCopyCommand={handleCopyCommand}
            copiedCommand={copiedCommand}
            testStatus={testStatus}
            onTestConnection={() => handleTestConnection(buildAgentUrl(selectedPreset, localHost))}
            onAdd={() =>
              handleAddComputer(
                localName,
                buildAgentUrl(selectedPreset, localHost),
                buildVncUrl(selectedPreset, localHost)
              )
            }
            isLoading={isLoading}
            error={error}
          />
        )}

        {step === 'configure' && source === 'cloud' && renderCloudCreate && (
          <div>{renderCloudCreate({ onCreated: handleCloudCreated, onCancel: handleClose })}</div>
        )}

        {step === 'configure' && source === 'custom' && (
          <CustomUrlForm
            name={customName}
            onNameChange={setCustomName}
            agentUrl={customAgentUrl}
            onAgentUrlChange={setCustomAgentUrl}
            vncUrl={customVncUrl}
            onVncUrlChange={setCustomVncUrl}
            testStatus={testStatus}
            onTestConnection={() => handleTestConnection(customAgentUrl)}
            onAdd={() => handleAddComputer(customName, customAgentUrl, customVncUrl)}
            isLoading={isLoading}
            error={error}
          />
        )}
      </div>
    </Modal>
  );
}

// Backward-compatible export
export const CustomComputerModal = AddSandboxModal;

// ---------------------------------------------------------------------------
// Step 1: Source selection
// ---------------------------------------------------------------------------

function SourceStep({
  onSelect,
  hasCloud,
}: {
  onSelect: (source: SandboxSource) => void;
  hasCloud: boolean;
}) {
  const cards: Array<{
    source: SandboxSource;
    icon: React.ReactNode;
    title: string;
    description: string;
    hidden?: boolean;
  }> = [
    {
      source: 'local',
      icon: <Monitor className="h-6 w-6" />,
      title: 'Local (Docker)',
      description: 'Run a sandbox on your machine using Docker',
    },
    {
      source: 'cloud',
      icon: <Server className="h-6 w-6" />,
      title: 'Cloud Sandbox',
      description: 'Provision a cloud VM',
      hidden: !hasCloud,
    },
    {
      source: 'custom',
      icon: <Globe className="h-6 w-6" />,
      title: 'Custom URL',
      description: 'Connect to any computer-server endpoint',
    },
  ];

  return (
    <div className="flex flex-col gap-3">
      {cards
        .filter((c) => !c.hidden)
        .map((card) => (
          <button
            key={card.source}
            type="button"
            onClick={() => onSelect(card.source)}
            className="flex items-center gap-4 rounded-lg border border-neutral-200 p-4 text-left transition-colors hover:border-blue-400 hover:bg-blue-50/50 dark:border-neutral-600 dark:hover:border-blue-500 dark:hover:bg-blue-950/20"
          >
            <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-neutral-100 text-neutral-600 dark:bg-neutral-700 dark:text-neutral-300">
              {card.icon}
            </div>
            <div className="flex-1">
              <p className="font-medium text-neutral-900 dark:text-white">{card.title}</p>
              <p className="text-sm text-neutral-500 dark:text-neutral-400">{card.description}</p>
            </div>
            <ChevronRight className="h-5 w-5 text-neutral-400" />
          </button>
        ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Step 2a: Preset grid for local Docker
// ---------------------------------------------------------------------------

const ICON_MAP: Record<SandboxPreset['icon'], string> = {
  linux: '\u{1F427}',
  windows: '\u{1FA9F}',
  android: '\u{1F4F1}',
  macos: '\u{1F34E}',
};

function PresetGrid({ onSelect }: { onSelect: (preset: SandboxPreset) => void }) {
  return (
    <div className="flex flex-col gap-3">
      {SANDBOX_PRESETS.map((preset) => (
        <button
          key={preset.id}
          type="button"
          onClick={() => onSelect(preset)}
          className="flex items-center gap-4 rounded-lg border border-neutral-200 p-4 text-left transition-colors hover:border-blue-400 hover:bg-blue-50/50 dark:border-neutral-600 dark:hover:border-blue-500 dark:hover:bg-blue-950/20"
        >
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-neutral-100 text-xl dark:bg-neutral-700">
            {ICON_MAP[preset.icon]}
          </div>
          <div className="flex-1">
            <p className="font-medium text-neutral-900 dark:text-white">{preset.name}</p>
            <p className="text-sm text-neutral-500 dark:text-neutral-400">{preset.description}</p>
          </div>
          <ChevronRight className="h-5 w-5 text-neutral-400" />
        </button>
      ))}

      {/* macOS informational card */}
      <div className="flex items-center gap-4 rounded-lg border border-dashed border-neutral-300 p-4 dark:border-neutral-600">
        <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-neutral-100 text-xl dark:bg-neutral-700">
          {ICON_MAP.macos}
        </div>
        <div className="flex-1">
          <p className="font-medium text-neutral-900 dark:text-white">{MACOS_PRESET.name}</p>
          <p className="text-sm text-neutral-500 dark:text-neutral-400">{MACOS_PRESET.note}</p>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Step 2b: Local Docker configuration form
// ---------------------------------------------------------------------------

function LocalDockerForm({
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
}: {
  preset: SandboxPreset;
  name: string;
  onNameChange: (v: string) => void;
  host: string;
  onHostChange: (v: string) => void;
  onCopyCommand: (cmd: string) => void;
  copiedCommand: boolean;
  testStatus: 'idle' | 'testing' | 'success' | 'error';
  onTestConnection: () => void;
  onAdd: () => void;
  isLoading: boolean;
  error: string | null;
}) {
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

// ---------------------------------------------------------------------------
// Step 2c: Custom URL form
// ---------------------------------------------------------------------------

function CustomUrlForm({
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
}: {
  name: string;
  onNameChange: (v: string) => void;
  agentUrl: string;
  onAgentUrlChange: (v: string) => void;
  vncUrl: string;
  onVncUrlChange: (v: string) => void;
  testStatus: 'idle' | 'testing' | 'success' | 'error';
  onTestConnection: () => void;
  onAdd: () => void;
  isLoading: boolean;
  error: string | null;
}) {
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

// ---------------------------------------------------------------------------
// Test connection button
// ---------------------------------------------------------------------------

function TestConnectionButton({
  status,
  onClick,
  disabled,
}: {
  status: 'idle' | 'testing' | 'success' | 'error';
  onClick: () => void;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled || status === 'testing'}
      className={cn(
        'flex items-center gap-2 rounded-md border px-4 py-2 text-sm font-medium transition-colors disabled:opacity-50',
        status === 'success'
          ? 'border-green-300 text-green-700 dark:border-green-700 dark:text-green-400'
          : status === 'error'
            ? 'border-red-300 text-red-700 dark:border-red-700 dark:text-red-400'
            : 'border-neutral-300 text-neutral-700 hover:bg-neutral-50 dark:border-neutral-600 dark:text-neutral-300 dark:hover:bg-neutral-700'
      )}
    >
      {status === 'testing' && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
      {status === 'success' && <Check className="h-3.5 w-3.5" />}
      {status === 'testing' ? 'Testing...' : status === 'success' ? 'Connected' : 'Test Connection'}
    </button>
  );
}

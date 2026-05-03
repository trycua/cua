// Add Sandbox wizard modal
// Supports three paths: Local Docker, Cloud Sandbox, Custom URL
//
// Sub-components live in sibling files:
//   SourcePicker.tsx  - Step 1: choose source
//   PresetGrid.tsx    - Step 2a: choose Docker preset
//   LocalDockerForm.tsx - Step 2b: configure local Docker
//   CustomUrlForm.tsx - Step 2c: configure custom URL

import { useState, useCallback } from 'react';
import { ArrowLeft } from 'lucide-react';
import { usePlayground } from '../../hooks/usePlayground';
import type { ComputerInfo } from '../../adapters/types';
import { cn } from '../../utils/cn';
import { Modal } from '../ui/Modal';
import type { SandboxPreset } from '../../constants/sandboxPresets';

import { SourcePicker, type SandboxSource } from './SourcePicker';
import { PresetGrid } from './PresetGrid';
import { LocalDockerForm } from './LocalDockerForm';
import { CustomUrlForm } from './CustomUrlForm';

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
          <SourcePicker onSelect={handleSelectSource} hasCloud={!!renderCloudCreate} />
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

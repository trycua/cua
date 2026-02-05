// Custom computer modal component
// Adapted from cloud/src/website/app/components/playground/modals/CustomComputerModal.tsx

import { useState } from 'react';
import { X } from 'lucide-react';
import { usePlayground } from '../../hooks/usePlayground';
import type { ComputerInfo } from '../../adapters/types';
import { cn } from '../../utils/cn';

interface CustomComputerModalProps {
  /** Whether the modal is open */
  isOpen: boolean;
  /** Callback to close the modal */
  onClose: () => void;
  /** Optional callback when a computer is added */
  onAdd?: (computer: ComputerInfo) => void;
  /** Optional class name for the modal content */
  className?: string;
}

export function CustomComputerModal({
  isOpen,
  onClose,
  onAdd,
  className,
}: CustomComputerModalProps) {
  const { adapters, dispatch } = usePlayground();
  const [name, setName] = useState('');
  const [url, setUrl] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);

    if (!name.trim() || !url.trim()) {
      setError('Please fill in all fields');
      return;
    }

    // Validate URL format
    try {
      new URL(url);
    } catch {
      setError('Please enter a valid URL');
      return;
    }

    setIsLoading(true);

    try {
      // Add computer via adapter
      if (adapters.computer.addCustomComputer) {
        const newComputer = await adapters.computer.addCustomComputer({
          name: name.trim(),
          vncUrl: `${url.trim()}/vnc`,
          agentUrl: url.trim(),
          status: 'running',
          isCustom: true,
        });

        // Refresh computer list
        const computers = await adapters.computer.listComputers();
        dispatch({ type: 'SET_COMPUTERS', payload: computers });

        onAdd?.(newComputer);
        onClose();
        setName('');
        setUrl('');
      } else {
        setError('Adding custom computers is not supported');
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to add computer');
    } finally {
      setIsLoading(false);
    }
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div
        className={cn(
          'relative w-full max-w-md rounded-lg bg-white p-6 shadow-xl dark:bg-neutral-800',
          className
        )}
      >
        {/* Close button */}
        <button
          type="button"
          onClick={onClose}
          className="absolute right-4 top-4 rounded-md p-1 hover:bg-neutral-100 dark:hover:bg-neutral-700"
        >
          <X className="h-5 w-5" />
        </button>

        {/* Header */}
        <h2 className="text-center text-xl font-semibold text-neutral-900 dark:text-white">
          Add a new Computer
        </h2>

        {/* Form */}
        <form onSubmit={handleSubmit} className="mt-6 flex flex-col gap-4">
          <div>
            <label
              htmlFor="computer-name"
              className="block text-sm font-medium text-neutral-700 dark:text-neutral-300"
            >
              Name
            </label>
            <input
              id="computer-name"
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="My Computer"
              className="mt-1 w-full rounded-md border border-neutral-300 px-3 py-2 text-sm dark:border-neutral-600 dark:bg-neutral-700"
              disabled={isLoading}
            />
          </div>

          <div>
            <label
              htmlFor="computer-url"
              className="block text-sm font-medium text-neutral-700 dark:text-neutral-300"
            >
              URL
            </label>
            <input
              id="computer-url"
              type="text"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="http://localhost:8443"
              className="mt-1 w-full rounded-md border border-neutral-300 px-3 py-2 text-sm dark:border-neutral-600 dark:bg-neutral-700"
              disabled={isLoading}
            />
          </div>

          {error && <p className="text-sm text-red-600 dark:text-red-400">{error}</p>}

          <div className="mt-4 flex justify-end gap-3">
            <button
              type="button"
              onClick={onClose}
              disabled={isLoading}
              className="rounded-md border px-4 py-2 hover:bg-neutral-50 disabled:opacity-50 dark:hover:bg-neutral-700"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={isLoading}
              className="rounded-md bg-blue-600 px-4 py-2 text-white hover:bg-blue-700 disabled:opacity-50"
            >
              {isLoading ? 'Adding...' : 'Add'}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}

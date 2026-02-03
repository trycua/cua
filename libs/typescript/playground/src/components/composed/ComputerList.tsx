// Computer list component
// Adapted from cloud/src/website/app/components/playground/panels/ComputerList.tsx

import { useState } from 'react';
import { AnimatePresence, motion } from 'framer-motion';
import { Server, Globe, RefreshCcw, EllipsisVertical, Trash, Pen, CirclePlus } from 'lucide-react';
import { usePlayground } from '../../hooks/usePlayground';
import type { ComputerInfo } from '../../adapters/types';
import { cn } from '../../utils/cn';

interface ComputerListProps {
  /** Optional class name for styling */
  className?: string;
  /** Optional callback when a computer is added */
  onAddComputer?: () => void;
  /** Optional callback when a computer is deleted */
  onDeleteComputer?: (computer: ComputerInfo) => void;
  /** Optional callback when a computer is renamed */
  onRenameComputer?: (computer: ComputerInfo, newName: string) => void;
}

export function ComputerList({
  className,
  onAddComputer,
  onDeleteComputer,
  onRenameComputer,
}: ComputerListProps) {
  const { state, dispatch, adapters } = usePlayground();
  const [menuOpenId, setMenuOpenId] = useState<string | null>(null);
  const [showDeleteModal, setShowDeleteModal] = useState(false);
  const [showRenameModal, setShowRenameModal] = useState(false);
  const [selectedComputer, setSelectedComputer] = useState<ComputerInfo | null>(null);
  const [newName, setNewName] = useState('');

  const handleDelete = async (computer: ComputerInfo) => {
    setSelectedComputer(computer);
    setShowDeleteModal(true);
  };

  const handleRename = (computer: ComputerInfo) => {
    setSelectedComputer(computer);
    setNewName(computer.name);
    setShowRenameModal(true);
  };

  const confirmDelete = async () => {
    if (!selectedComputer) return;

    // Remove from adapter if it supports it
    if (selectedComputer.isCustom && adapters.computer.removeCustomComputer) {
      try {
        await adapters.computer.removeCustomComputer(selectedComputer.id);
      } catch (error) {
        console.error('Failed to remove computer:', error);
      }
    }

    // Refresh computer list
    const computers = await adapters.computer.listComputers();
    dispatch({ type: 'SET_COMPUTERS', payload: computers });

    onDeleteComputer?.(selectedComputer);
    setShowDeleteModal(false);
    setSelectedComputer(null);
  };

  const confirmRename = async () => {
    if (!selectedComputer || !newName.trim()) return;

    // Note: Renaming is handled by the parent - adapters don't support rename directly
    onRenameComputer?.(selectedComputer, newName.trim());
    setShowRenameModal(false);
    setSelectedComputer(null);
    setNewName('');
  };

  const getStatusColor = (status: string) => {
    switch (status) {
      case 'running':
        return 'bg-green-100 text-green-800 dark:bg-green-900 dark:text-green-200';
      case 'stopped':
        return 'bg-neutral-100 text-neutral-800 dark:bg-neutral-600 dark:text-neutral-200';
      case 'starting':
        return 'bg-blue-100 text-blue-800 dark:bg-blue-900 dark:text-blue-200';
      case 'error':
        return 'bg-red-100 text-red-800 dark:bg-red-900 dark:text-red-200';
      default:
        return 'bg-gray-100 text-gray-800 dark:bg-gray-900 dark:text-gray-200';
    }
  };

  return (
    <div className={cn('flex h-full flex-col', className)}>
      {/* Header */}
      <div className="flex items-center justify-between border-b px-4 py-3">
        <h2 className="font-semibold">Computers</h2>
        {onAddComputer && (
          <button
            type="button"
            onClick={onAddComputer}
            className="rounded-md border p-2 hover:bg-neutral-50 dark:hover:bg-neutral-800"
            title="Add Computer"
          >
            <CirclePlus className="h-4 w-4" />
          </button>
        )}
      </div>

      {/* Computer List */}
      <ul className="flex-1 overflow-auto p-2">
        <AnimatePresence>
          {state.computers.map((computer) => (
            <motion.li
              key={computer.id}
              layout
              initial={{ opacity: 0, y: -20 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, x: -100 }}
              transition={{ duration: 0.2, ease: 'easeOut' }}
              className="relative mb-3 rounded-lg border bg-white p-3 dark:bg-neutral-800"
            >
              {/* Type Badge & Menu */}
              <div className="absolute right-3 top-3 flex gap-1">
                <div className="flex items-center gap-1 rounded bg-neutral-100 px-2 py-1 text-xs font-medium text-neutral-600 dark:bg-neutral-700 dark:text-neutral-300">
                  {computer.isCustom ? (
                    <Globe className="h-3 w-3" />
                  ) : (
                    <Server className="h-3 w-3" />
                  )}
                  {computer.isCustom ? 'Custom' : 'VM'}
                </div>

                {computer.isCustom && (
                  <div className="relative">
                    <button
                      type="button"
                      onClick={() => setMenuOpenId(menuOpenId === computer.id ? null : computer.id)}
                      className="rounded-md p-1 hover:bg-neutral-200 dark:hover:bg-neutral-600"
                    >
                      <EllipsisVertical className="h-4 w-4" />
                    </button>

                    {menuOpenId === computer.id && (
                      <div className="absolute right-0 z-10 mt-1 w-32 rounded-md border bg-white shadow-lg dark:bg-neutral-800">
                        <button
                          type="button"
                          onClick={() => {
                            handleRename(computer);
                            setMenuOpenId(null);
                          }}
                          className="flex w-full items-center gap-2 px-3 py-2 text-sm hover:bg-neutral-50 dark:hover:bg-neutral-700"
                        >
                          <Pen className="h-4 w-4" /> Rename
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            handleDelete(computer);
                            setMenuOpenId(null);
                          }}
                          className="flex w-full items-center gap-2 px-3 py-2 text-sm text-red-600 hover:bg-neutral-50 dark:text-red-400 dark:hover:bg-neutral-700"
                        >
                          <Trash className="h-4 w-4" /> Delete
                        </button>
                      </div>
                    )}
                  </div>
                )}
              </div>

              {/* Computer Info */}
              <div className="flex flex-col gap-2">
                <h3 className="pr-20 text-lg font-semibold text-neutral-900 dark:text-neutral-100">
                  {computer.name}
                </h3>

                <div className="flex flex-wrap gap-2 text-sm text-neutral-600 dark:text-neutral-400">
                  <div className="flex items-center gap-1">
                    <Globe className="h-4 w-4" />
                    <span className="truncate">{computer.agentUrl}</span>
                  </div>
                </div>

                {/* Status */}
                <div className="flex items-center gap-2">
                  <span
                    className={cn(
                      'flex items-center justify-center gap-2 rounded-full px-2 py-1 text-xs font-medium',
                      getStatusColor(computer.status)
                    )}
                  >
                    {computer.status === 'starting' && (
                      <RefreshCcw className="h-3 w-3 animate-spin" />
                    )}
                    {computer.status}
                  </span>
                </div>
              </div>
            </motion.li>
          ))}
        </AnimatePresence>

        {state.computers.length === 0 && (
          <div className="flex flex-col items-center justify-center py-12 text-center">
            <div className="mb-4 rounded-full bg-neutral-100 p-6 dark:bg-neutral-800">
              <Server className="h-8 w-8 text-neutral-400" />
            </div>
            <h3 className="mb-2 text-lg font-medium text-neutral-900 dark:text-neutral-100">
              No computers yet
            </h3>
            <p className="max-w-sm text-neutral-500 dark:text-neutral-400">
              Add a computer to get started.
            </p>
          </div>
        )}
      </ul>

      {/* Delete Modal */}
      {showDeleteModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="w-full max-w-md rounded-lg bg-white p-6 shadow-xl dark:bg-neutral-800">
            <h3 className="text-lg font-semibold text-neutral-900 dark:text-neutral-100">
              Delete Computer
            </h3>
            <p className="mt-2 text-sm text-neutral-600 dark:text-neutral-400">
              Are you sure you want to delete "{selectedComputer?.name}"?
            </p>
            <div className="mt-4 flex justify-end gap-3">
              <button
                type="button"
                onClick={() => {
                  setShowDeleteModal(false);
                  setSelectedComputer(null);
                }}
                className="rounded-md border px-4 py-2 hover:bg-neutral-50 dark:hover:bg-neutral-700"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={confirmDelete}
                className="rounded-md bg-red-600 px-4 py-2 text-white hover:bg-red-700"
              >
                Delete
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Rename Modal */}
      {showRenameModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="w-full max-w-md rounded-lg bg-white p-6 shadow-xl dark:bg-neutral-800">
            <h3 className="text-lg font-semibold text-neutral-900 dark:text-neutral-100">
              Rename Computer
            </h3>
            <p className="mt-2 text-sm text-neutral-600 dark:text-neutral-400">
              Enter a new name for "{selectedComputer?.name}".
            </p>
            <input
              type="text"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              className="mt-3 w-full rounded-md border border-neutral-300 px-3 py-2 text-sm dark:border-neutral-600 dark:bg-neutral-700"
              autoFocus
            />
            <div className="mt-4 flex justify-end gap-3">
              <button
                type="button"
                onClick={() => {
                  setShowRenameModal(false);
                  setSelectedComputer(null);
                  setNewName('');
                }}
                className="rounded-md border px-4 py-2 hover:bg-neutral-50 dark:hover:bg-neutral-700"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={confirmRename}
                disabled={!newName.trim()}
                className="rounded-md bg-blue-600 px-4 py-2 text-white hover:bg-blue-700 disabled:opacity-50"
              >
                Save
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

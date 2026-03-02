// Settings modal component
// Adapted from cloud/src/website/app/components/playground/modals/SettingsModal.tsx

import { X } from 'lucide-react';
import { cn } from '../../utils/cn';

interface SettingsModalProps {
  /** Whether the modal is open */
  isOpen: boolean;
  /** Callback to close the modal */
  onClose: () => void;
  /** Optional class name for the modal content */
  className?: string;
  /** Optional children to render in the modal */
  children?: React.ReactNode;
}

export function SettingsModal({ isOpen, onClose, className, children }: SettingsModalProps) {
  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div
        className={cn(
          'relative w-full max-w-3xl rounded-lg bg-white p-6 shadow-xl dark:bg-neutral-800',
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

        {/* Content */}
        {children || (
          <div className="flex flex-col">
            <div className="mb-4">
              <h2 className="text-xl font-bold text-neutral-900 dark:text-white">
                Playground Settings
              </h2>
              <p className="mt-2 text-sm text-neutral-600 dark:text-neutral-400">
                Configure your playground settings here.
              </p>
            </div>

            <div className="py-8 text-center text-neutral-500">Settings panel coming soon...</div>
          </div>
        )}
      </div>
    </div>
  );
}

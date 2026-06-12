// Shared "Test Connection" button used by LocalDockerForm and CustomUrlForm

import { Check, Loader2 } from 'lucide-react';
import { cn } from '../../utils/cn';

export type TestStatus = 'idle' | 'testing' | 'success' | 'error';

export interface TestConnectionButtonProps {
  status: TestStatus;
  onClick: () => void;
  disabled?: boolean;
}

export function TestConnectionButton({ status, onClick, disabled }: TestConnectionButtonProps) {
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

import { X } from 'lucide-react';
import { motion } from 'motion/react';
import type { VM } from '../../types';
import { isVM } from '../../types';

/**
 * Version info for a VM, used to determine if warning banners should show
 */
export interface VMVersionInfo {
  isOutdated?: boolean;
  unreachable?: boolean;
}

interface VMStatusBannerProps {
  /** Callback when restart button is clicked */
  onRestartVM: (vm: VM) => void;
  /** Callback when start button is clicked */
  onStartVM: (vm: VM) => void;
  /** Callback when action button is clicked (for no-sandbox scenarios) */
  onAction?: (action: string) => void;
  /** Whether the user has an organization set up */
  hasOrg: boolean;
  /** Whether the user has a workspace set up */
  hasWorkspace: boolean;
  /** Whether the user has credits available */
  hasCredits: boolean;
  /** Organization slug for building links */
  orgSlug?: string;
  /** List of available computers/sandboxes */
  computers: Array<VM | { id: string; url: string; name: string }>;
  /** Currently selected computer */
  computer?: VM | { id: string; url: string; name: string } | null;
  /** VM version info map (vmId -> VMVersionInfo) */
  vmVersionInfo?: Map<string, VMVersionInfo>;
  /** Currently dismissed VM warning ID */
  dismissedVmWarning?: string | null;
  /** Callback when warning is dismissed */
  onDismissWarning?: (vmId: string) => void;
}

/**
 * Displays status banners for VM-related issues:
 * - No sandboxes available (with setup guidance)
 * - Sandbox offline
 * - Sandbox outdated or unreachable
 *
 * Adapted from cloud/src/website/app/components/playground/VMStatusBanner.tsx
 * Uses props instead of context for better reusability across different applications.
 */
export function VMStatusBanner({
  onRestartVM,
  onStartVM,
  onAction,
  hasOrg,
  hasWorkspace,
  hasCredits,
  orgSlug,
  computers,
  computer,
  vmVersionInfo,
  dismissedVmWarning,
  onDismissWarning,
}: VMStatusBannerProps) {
  // No sandboxes banner - show different messages based on setup state
  if (computers.length === 0) {
    // Determine message and action based on setup state
    let title = 'No Sandboxes Available';
    let message = 'Create your first sandbox to start using the playground.';
    let buttonText = 'Create Sandbox';
    let actionType = 'create-sandbox';

    if (!hasOrg) {
      title = 'Set Up Your Organization';
      message = 'Create an organization to start using sandboxes.';
      buttonText = 'Get Started';
      actionType = 'get-started';
    } else if (!hasWorkspace) {
      title = 'Create a Workspace';
      message = 'Create a workspace to manage your sandboxes.';
      buttonText = 'Create Workspace';
      actionType = 'create-workspace';
    } else if (!hasCredits) {
      message = 'Purchase credits to create a sandbox.';
      buttonText = 'Purchase Credits';
      actionType = 'purchase-credits';
    }

    return (
      <motion.div
        initial={{ opacity: 0, y: 10 }}
        animate={{ opacity: 1, y: 0 }}
        exit={{ opacity: 0, y: 10 }}
        className="mb-4 flex items-center justify-between gap-3 rounded-lg border border-blue-200 bg-blue-50 p-3 dark:border-blue-900/50 dark:bg-blue-950/30"
      >
        <div className="flex-1">
          <p className="font-medium text-blue-900 text-sm dark:text-blue-200">{title}</p>
          <p className="mt-0.5 text-blue-700 text-xs dark:text-blue-300">{message}</p>
        </div>
        <button
          type="button"
          onClick={() => onAction?.(actionType)}
          className="rounded-md bg-blue-600 px-3 py-1.5 font-medium text-white text-xs transition-colors hover:bg-blue-700 dark:bg-blue-700 dark:hover:bg-blue-600"
        >
          {buttonText}
        </button>
      </motion.div>
    );
  }

  // No computer selected or not a VM
  if (!computer || !isVM(computer)) {
    return null;
  }

  const vm = computer as VM;

  // Offline banner
  if (vm.status !== 'running' && vm.status !== 'restarting' && vm.status !== 'starting') {
    return (
      <motion.div
        initial={{ opacity: 0, y: 10 }}
        animate={{ opacity: 1, y: 0 }}
        exit={{ opacity: 0, y: 10 }}
        className="mb-4 flex items-center justify-between gap-3 rounded-lg border border-red-200 bg-red-50 p-3 dark:border-red-900/50 dark:bg-red-950/30"
      >
        <div className="flex-1">
          <p className="font-medium text-red-900 text-sm dark:text-red-200">Sandbox Offline</p>
          <p className="mt-0.5 text-red-700 text-xs dark:text-red-300">
            This sandbox is not running. Start it to begin using it.
          </p>
        </div>
        <button
          type="button"
          onClick={(e) => {
            e.preventDefault();
            onStartVM(vm);
          }}
          className="rounded-md bg-red-600 px-3 py-1.5 font-medium text-white text-xs transition-colors hover:bg-red-700 dark:bg-red-700 dark:hover:bg-red-600"
        >
          Start
        </button>
      </motion.div>
    );
  }

  // Outdated/Unreachable banner
  const versionInfo = vmVersionInfo?.get(vm.vmId);
  const shouldShowWarning =
    (versionInfo?.isOutdated || versionInfo?.unreachable) && dismissedVmWarning !== vm.vmId;

  if (!shouldShowWarning) {
    return null;
  }

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: 10 }}
      className="mb-4 flex items-center justify-between gap-3 rounded-lg border border-orange-200 bg-orange-50 p-3 dark:border-orange-900/50 dark:bg-orange-950/30"
    >
      <div className="flex-1">
        <p className="font-medium text-orange-900 text-sm dark:text-orange-200">
          {versionInfo?.unreachable ? 'Sandbox Not Responding' : 'Sandbox Outdated'}
        </p>
        <p className="mt-0.5 text-orange-700 text-xs dark:text-orange-300">
          {versionInfo?.unreachable
            ? 'The sandbox is slow to respond. Try waiting a moment or restarting it.'
            : 'The computer-server is out of date. Restart the sandbox to update.'}
        </p>
      </div>
      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={(e) => {
            e.preventDefault();
            onRestartVM(vm);
          }}
          disabled={vm.status === 'restarting'}
          className="rounded-md bg-orange-600 px-3 py-1.5 font-medium text-white text-xs transition-colors hover:bg-orange-700 disabled:cursor-not-allowed disabled:opacity-50 dark:bg-orange-700 dark:hover:bg-orange-600"
        >
          {vm.status === 'restarting' ? 'Restarting...' : 'Restart'}
        </button>
        {onDismissWarning && (
          <button
            type="button"
            onClick={() => onDismissWarning(vm.vmId)}
            className="flex h-6 w-6 items-center justify-center rounded text-orange-600 transition-colors hover:bg-orange-100 dark:text-orange-400 dark:hover:bg-orange-900/50"
          >
            <X className="h-4 w-4" />
          </button>
        )}
      </div>
    </motion.div>
  );
}

export type { VMStatusBannerProps };

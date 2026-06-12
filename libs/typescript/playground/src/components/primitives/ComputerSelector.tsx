// Computer selector dropdown component
// Extracted from ChatInput.tsx

import { Loader2 } from 'lucide-react';
import { useMemo } from 'react';
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from '../ui/select';
import type { Computer, VM } from '../../types';
import { isVM, getComputerId, getComputerName, getComputerStatus } from '../../types';

/**
 * Groups computers by workspace name, sorted alphabetically with "Custom" at the end.
 */
export function groupComputersByWorkspace(
  computers: Computer[]
): Array<{ workspaceName: string; computers: Computer[] }> {
  const groups = new Map<string, Computer[]>();

  for (const computer of computers) {
    const wsName = isVM(computer) ? 'Sandboxes' : 'Custom';
    if (!groups.has(wsName)) {
      groups.set(wsName, []);
    }
    groups.get(wsName)!.push(computer);
  }

  // Sort alphabetically, with "Custom" at the end
  return Array.from(groups.entries())
    .sort(([a], [b]) => {
      if (a === 'Custom') return 1;
      if (b === 'Custom') return -1;
      return a.localeCompare(b);
    })
    .map(([workspaceName, computers]) => ({ workspaceName, computers }));
}

export interface ComputerSelectorProps {
  computers: Computer[];
  selectedComputer?: Computer;
  onComputerChange: (computerId: string) => void;
  disabled?: boolean;
  vmVersionInfo?: Map<string, { unreachable?: boolean; isOutdated?: boolean }>;
  hasOrg?: boolean;
  hasWorkspace?: boolean;
  onAddComputer?: () => void;
  /** When true, renders the mobile variant (simpler items, z-index on content) */
  mobile?: boolean;
}

export function ComputerSelector({
  computers,
  selectedComputer,
  onComputerChange,
  disabled = false,
  vmVersionInfo = new Map(),
  hasOrg = true,
  hasWorkspace = true,
  onAddComputer,
  mobile = false,
}: ComputerSelectorProps) {
  const groupedComputers = useMemo(() => groupComputersByWorkspace(computers), [computers]);

  const handleChange = (computerId: string) => {
    if (computerId === '__add_sandbox__') {
      onAddComputer?.();
      return;
    }
    onComputerChange(computerId);
  };

  const value =
    selectedComputer &&
    computers.find((c: Computer) => c.name === selectedComputer.name) !== undefined
      ? getComputerId(selectedComputer)
      : '';

  if (mobile) {
    return (
      <Select value={value} onValueChange={handleChange} disabled={disabled}>
        <SelectTrigger
          minimal
          className="flex-1 border-0 bg-neutral-100 text-neutral-600 disabled:cursor-not-allowed disabled:opacity-50 dark:bg-neutral-800 dark:text-neutral-400"
        >
          <SelectValue placeholder="Sandbox" />
        </SelectTrigger>
        <SelectContent className="z-[100] rounded-lg bg-white dark:bg-card">
          {groupedComputers.map((group) => (
            <SelectGroup key={group.workspaceName}>
              <SelectLabel>{group.workspaceName}</SelectLabel>
              {group.computers.map((c: Computer) => (
                <SelectItem
                  key={getComputerId(c)}
                  value={getComputerId(c)}
                  className="hover:bg-neutral-200 dark:hover:bg-neutral-700"
                >
                  {getComputerName(c)}
                </SelectItem>
              ))}
            </SelectGroup>
          ))}
          {onAddComputer && (
            <>
              <SelectSeparator />
              <SelectItem
                value="__add_sandbox__"
                className="text-blue-600 hover:bg-neutral-200 dark:text-blue-400 dark:hover:bg-neutral-700"
              >
                + Add Sandbox...
              </SelectItem>
            </>
          )}
        </SelectContent>
      </Select>
    );
  }

  // Desktop variant with status indicators
  return (
    <Select value={value} onValueChange={handleChange} disabled={disabled}>
      <SelectTrigger
        minimal
        className="border-0 bg-neutral-100 text-neutral-400 hover:bg-white/60 disabled:cursor-not-allowed disabled:opacity-50 dark:bg-neutral-800 dark:text-neutral-400 dark:hover:bg-neutral-700/60 [&>svg]:rotate-180"
      >
        <SelectValue placeholder="Sandbox" />
      </SelectTrigger>
      <SelectContent className="rounded-lg bg-white dark:bg-card">
        {computers.length === 0 ? (
          <div className="p-2 text-center">
            <p className="mb-2 text-muted-foreground text-xs">
              {!hasOrg
                ? 'Set up an organization first'
                : !hasWorkspace
                  ? 'Create a workspace first'
                  : 'No sandboxes available'}
            </p>
          </div>
        ) : (
          <>
            {groupedComputers.map((group) => (
              <SelectGroup key={group.workspaceName}>
                <SelectLabel>{group.workspaceName}</SelectLabel>
                {group.computers.map((c) => {
                  const computerStatus = getComputerStatus(c);
                  const isRunning = computerStatus === 'running';
                  const versionInfo = isVM(c) ? vmVersionInfo.get((c as VM).vmId) : undefined;
                  const isUnreachable = isVM(c) ? versionInfo?.unreachable : false;
                  return (
                    <SelectItem
                      key={getComputerId(c)}
                      value={getComputerId(c)}
                      className="hover:bg-neutral-200 dark:hover:bg-neutral-700"
                    >
                      <div className="flex items-center gap-2">
                        {computerStatus === 'restarting' || computerStatus === 'starting' ? (
                          <Loader2 className="h-3 w-3 animate-spin text-neutral-500 dark:text-neutral-400" />
                        ) : (
                          <div
                            className={`h-2 w-2 rounded-full ${
                              isUnreachable
                                ? 'bg-orange-500'
                                : isRunning
                                  ? 'bg-green-500'
                                  : 'bg-red-500'
                            }`}
                          />
                        )}
                        <span>{getComputerName(c)}</span>
                        {computerStatus === 'starting' ? (
                          <span className="text-neutral-500 text-xs dark:text-neutral-400">
                            Starting
                          </span>
                        ) : computerStatus === 'restarting' ? (
                          <span className="text-neutral-500 text-xs dark:text-neutral-400">
                            Restarting
                          </span>
                        ) : isUnreachable ? (
                          <span className="text-orange-600 text-xs dark:text-orange-400">
                            Not responding
                          </span>
                        ) : (
                          versionInfo?.isOutdated && (
                            <span className="text-orange-600 text-xs dark:text-orange-400">
                              Outdated
                            </span>
                          )
                        )}
                      </div>
                    </SelectItem>
                  );
                })}
              </SelectGroup>
            ))}
          </>
        )}
        {onAddComputer && (
          <>
            <SelectSeparator />
            <SelectItem
              value="__add_sandbox__"
              className="text-blue-600 hover:bg-neutral-200 dark:text-blue-400 dark:hover:bg-neutral-700"
            >
              + Add Sandbox...
            </SelectItem>
          </>
        )}
      </SelectContent>
    </Select>
  );
}

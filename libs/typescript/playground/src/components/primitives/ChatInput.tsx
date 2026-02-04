// Chat input component
// Based on cloud/src/website/app/components/playground/ChatInput.tsx from main

import { Loader2, Monitor, Send, StopCircle } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from '../ui/select';
import { Tooltip, TooltipContent, TooltipTrigger } from '../ui/tooltip';
import type { Computer, Model, ModelProvider, VM } from '../../types';
import { isVM, getComputerId, getComputerName } from '../../types';

/**
 * Groups computers by workspace name, sorted alphabetically with "Custom" at the end.
 */
function groupComputersByWorkspace(
  computers: Computer[]
): Array<{ workspaceName: string; computers: Computer[] }> {
  const groups = new Map<string, Computer[]>();

  for (const computer of computers) {
    const wsName = isVM(computer) ? ((computer as VM).workspaceName ?? 'Custom') : 'Custom';
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

const GHOST_PROMPTS = [
  'Describe my screen',
  'Type ls in terminal',
  'Go to google.com',
  'Make a new folder',
  'Delete my newest download',
];

function useGhostTyping(isActive: boolean) {
  const [ghostText, setGhostText] = useState('');
  const [promptIndex, setPromptIndex] = useState(0);
  const [charIndex, setCharIndex] = useState(0);
  const [isDeleting, setIsDeleting] = useState(false);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clearTimeoutRef = useCallback(() => {
    if (timeoutRef.current) {
      clearTimeout(timeoutRef.current);
      timeoutRef.current = null;
    }
  }, []);

  useEffect(() => {
    if (!isActive) {
      clearTimeoutRef();
      setGhostText('');
      return;
    }

    const currentPrompt = GHOST_PROMPTS[promptIndex];

    if (!isDeleting) {
      // Typing
      if (charIndex < currentPrompt.length) {
        timeoutRef.current = setTimeout(() => {
          setGhostText(currentPrompt.slice(0, charIndex + 1));
          setCharIndex((prev) => prev + 1);
        }, 50);
      } else {
        // Pause at end before deleting
        timeoutRef.current = setTimeout(() => {
          setIsDeleting(true);
        }, 2000);
      }
    } else {
      // Deleting
      if (charIndex > 0) {
        timeoutRef.current = setTimeout(() => {
          setGhostText(currentPrompt.slice(0, charIndex - 1));
          setCharIndex((prev) => prev - 1);
        }, 30);
      } else {
        // Move to next prompt
        setIsDeleting(false);
        setPromptIndex((prev) => (prev + 1) % GHOST_PROMPTS.length);
      }
    }

    return clearTimeoutRef;
  }, [isActive, promptIndex, charIndex, isDeleting, clearTimeoutRef]);

  return ghostText;
}

interface ChatInputProps {
  // Core props
  currentInput: string;
  onInputChange: (value: string) => void;
  onSendMessage: () => void;
  onStopResponse: () => void;
  isGenerating: boolean;

  // Computer selection
  computers: Computer[];
  selectedComputer?: Computer;
  onComputerChange: (computerId: string) => void;

  // Model selection
  availableModels: ModelProvider[];
  selectedModel?: Model;
  onModelChange: (modelId: string) => void;

  // Optional props
  hasMessages?: boolean;
  isMobile?: boolean;
  customModelId?: string;
  customVncUrl?: string;
  vmVersionInfo?: Map<string, { unreachable?: boolean; isOutdated?: boolean }>;

  // Navigation props (optional - for cloud usage)
  hasOrg?: boolean;
  hasWorkspace?: boolean;
  hasCredits?: boolean;
  orgSlug?: string;
  renderLink?: (props: {
    to: string;
    children: React.ReactNode;
    className?: string;
  }) => React.ReactNode;
}

export function ChatInput({
  currentInput,
  onInputChange,
  onSendMessage,
  onStopResponse,
  isGenerating,
  computers,
  selectedComputer,
  onComputerChange,
  availableModels,
  selectedModel,
  onModelChange,
  hasMessages = false,
  isMobile = false,
  customModelId,
  customVncUrl,
  vmVersionInfo = new Map(),
  hasOrg = true,
  hasWorkspace = true,
  hasCredits = true,
  renderLink,
}: ChatInputProps) {
  // Lock model/computer selection after first message, but only if they're set
  // This allows users to still pick if the saved model/computer is unavailable
  const isModelLocked = hasMessages && !!selectedModel;
  const isComputerLocked = hasMessages && !!selectedComputer;

  const handleModelChange = (modelId: string) => {
    onModelChange(modelId);
  };

  const handleComputerChange = (computerId: string) => {
    onComputerChange(computerId);
  };

  const handleInputChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    onInputChange(e.target.value);
    // Auto-resize textarea
    e.target.style.height = 'auto';
    e.target.style.height = `${e.target.scrollHeight}px`;
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      onSendMessage();
    }
  };

  const handleOpenVnc = () => {
    if (selectedComputer && isVM(selectedComputer)) {
      const vm = selectedComputer as VM;
      const vncUrl = customVncUrl || vm.vncUrl;
      if (vncUrl) {
        window.open(vncUrl, '_blank');
      }
    }
  };

  const sendDisabled =
    !currentInput.trim() ||
    (selectedComputer && isVM(selectedComputer) && (selectedComputer as VM).status !== 'running');

  // Group computers by workspace
  const groupedComputers = useMemo(() => groupComputersByWorkspace(computers), [computers]);

  // Ghost typing - only show when input is empty and no messages
  const showGhostTyping = !currentInput && !hasMessages && !isGenerating;
  const ghostText = useGhostTyping(showGhostTyping);

  // Desktop layout
  if (!isMobile) {
    return (
      <div className="flex flex-col rounded-2xl bg-neutral-100 p-2 dark:bg-neutral-800">
        {/* Textarea row */}
        <div className="relative">
          <textarea
            value={currentInput}
            onChange={handleInputChange}
            onKeyDown={handleKeyDown}
            placeholder={showGhostTyping ? '' : 'Send a message'}
            disabled={isGenerating}
            rows={1}
            className="w-full resize-none rounded-2xl bg-neutral-100 px-3 pt-2 text-neutral-900 placeholder-neutral-400 focus:outline-none disabled:opacity-50 dark:bg-neutral-800 dark:text-white dark:placeholder-neutral-500"
            style={{ minHeight: '44px', maxHeight: '200px' }}
          />
          {showGhostTyping && ghostText && (
            <div
              className="pointer-events-none absolute top-0 left-0 px-3 pt-2 text-neutral-400 dark:text-neutral-500"
              aria-hidden="true"
            >
              {ghostText}
              <span className="animate-pulse">|</span>
            </div>
          )}
        </div>

        {/* Desktop: Model & Sandbox selectors - button bar row */}
        <div className="flex flex-wrap items-center justify-end gap-2">
          {/* Computer selector - locked after first message */}
          <Select
            value={
              selectedComputer &&
              computers.find((c: Computer) => c.name === selectedComputer.name) !== undefined
                ? getComputerId(selectedComputer)
                : ''
            }
            onValueChange={handleComputerChange}
            disabled={isComputerLocked}
          >
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
                        const isRunning = isVM(c) && (c as VM).status === 'running';
                        const versionInfo = isVM(c) ? vmVersionInfo.get((c as VM).vmId) : undefined;
                        const isUnreachable = versionInfo?.unreachable;
                        const vmComputer = c as VM;
                        return (
                          <SelectItem
                            key={getComputerId(c)}
                            value={getComputerId(c)}
                            className="hover:bg-neutral-200 dark:hover:bg-neutral-700"
                          >
                            <div className="flex items-center gap-2">
                              {isVM(c) &&
                              (vmComputer.status === 'restarting' ||
                                vmComputer.status === 'starting') ? (
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
                              {isVM(c) && vmComputer.status === 'starting' ? (
                                <span className="text-neutral-500 text-xs dark:text-neutral-400">
                                  Starting
                                </span>
                              ) : isVM(c) && vmComputer.status === 'restarting' ? (
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
            </SelectContent>
          </Select>

          {/* Model selector - locked after first message */}
          <Select
            value={selectedModel?.id || ''}
            onValueChange={handleModelChange}
            disabled={!!customModelId || isModelLocked}
          >
            <SelectTrigger
              minimal
              className="border-0 bg-neutral-100 text-neutral-400 hover:bg-white/60 disabled:cursor-not-allowed disabled:opacity-50 dark:bg-neutral-800 dark:text-neutral-400 dark:hover:bg-neutral-700/60 [&>svg]:rotate-180"
            >
              <SelectValue placeholder="Model" />
            </SelectTrigger>
            <SelectContent className="rounded-lg bg-white dark:bg-card">
              {customModelId && (
                <SelectGroup>
                  <SelectLabel>Custom</SelectLabel>
                  <SelectItem
                    key={customModelId}
                    value={customModelId}
                    className="hover:bg-neutral-200 dark:hover:bg-neutral-700"
                  >
                    {customModelId}
                  </SelectItem>
                </SelectGroup>
              )}
              {availableModels.map((provider) => (
                <SelectGroup key={provider.name}>
                  <SelectLabel>
                    {provider.name.charAt(0).toUpperCase() + provider.name.slice(1)}
                  </SelectLabel>
                  {provider.models.map((m) => (
                    <SelectItem
                      key={m.id}
                      value={m.id}
                      className="hover:bg-neutral-200 dark:hover:bg-neutral-700"
                    >
                      <span className="flex items-center gap-2">
                        {m.name}
                        {provider.name.toLowerCase() !== 'anthropic' && (
                          <span className="rounded bg-amber-100 px-1.5 py-0.5 font-medium text-amber-700 text-xs dark:bg-amber-900/30 dark:text-amber-400">
                            Preview
                          </span>
                        )}
                      </span>
                    </SelectItem>
                  ))}
                </SelectGroup>
              ))}
            </SelectContent>
          </Select>

          {/* Open VM button */}
          {selectedComputer &&
            isVM(selectedComputer) &&
            ((selectedComputer as VM).vncUrl || customVncUrl) && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    type="button"
                    onClick={handleOpenVnc}
                    className="flex h-8 w-8 items-center justify-center text-neutral-600 transition-colors hover:text-neutral-900 dark:text-neutral-400 dark:hover:text-white"
                  >
                    <Monitor className="h-4 w-4" />
                  </button>
                </TooltipTrigger>
                <TooltipContent>
                  <p>Open VNC in new tab</p>
                </TooltipContent>
              </Tooltip>
            )}

          {/* Send/Stop button */}
          {isGenerating ? (
            <button
              type="button"
              onClick={onStopResponse}
              className="flex h-8 w-8 items-center justify-center text-neutral-600 transition-colors hover:text-neutral-900 dark:text-neutral-400 dark:hover:text-white"
            >
              <StopCircle className="h-4 w-4" />
            </button>
          ) : (
            <button
              type="button"
              onClick={onSendMessage}
              disabled={sendDisabled}
              className="flex h-8 w-8 items-center justify-center text-neutral-600 transition-colors hover:text-neutral-900 disabled:text-neutral-300 dark:text-neutral-400 dark:disabled:text-neutral-600 dark:hover:text-white"
              title={
                selectedComputer &&
                isVM(selectedComputer) &&
                (selectedComputer as VM).status !== 'running'
                  ? 'Sandbox is not running'
                  : undefined
              }
            >
              <Send className="h-4 w-4" />
            </button>
          )}
        </div>
      </div>
    );
  }

  // Mobile layout
  return (
    <div className="space-y-2">
      {/* Mobile selectors row - locked after first message */}
      <div className="flex gap-2">
        <Select
          value={
            selectedComputer &&
            computers.find((c: Computer) => c.name === selectedComputer.name) !== undefined
              ? getComputerId(selectedComputer)
              : ''
          }
          onValueChange={handleComputerChange}
          disabled={isComputerLocked}
        >
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
          </SelectContent>
        </Select>

        <Select
          value={selectedModel?.id || ''}
          onValueChange={handleModelChange}
          disabled={!!customModelId || isModelLocked}
        >
          <SelectTrigger
            minimal
            className="flex-1 border-0 bg-neutral-100 text-neutral-600 disabled:cursor-not-allowed disabled:opacity-50 dark:bg-neutral-800 dark:text-neutral-400"
          >
            <SelectValue placeholder="Model" />
          </SelectTrigger>
          <SelectContent className="z-[100] rounded-lg bg-white dark:bg-card">
            {customModelId && (
              <SelectGroup>
                <SelectLabel>Custom</SelectLabel>
                <SelectItem
                  key={customModelId}
                  value={customModelId}
                  className="hover:bg-neutral-200 dark:hover:bg-neutral-700"
                >
                  {customModelId}
                </SelectItem>
              </SelectGroup>
            )}
            {availableModels.map((provider) => (
              <SelectGroup key={provider.name}>
                <SelectLabel>
                  {provider.name.charAt(0).toUpperCase() + provider.name.slice(1)}
                </SelectLabel>
                {provider.models.map((m) => (
                  <SelectItem
                    key={m.id}
                    value={m.id}
                    className="hover:bg-neutral-200 dark:hover:bg-neutral-700"
                  >
                    {m.name}
                  </SelectItem>
                ))}
              </SelectGroup>
            ))}
          </SelectContent>
        </Select>
      </div>

      {/* Mobile input row */}
      <div className="flex items-end gap-2 rounded-2xl bg-neutral-100 p-2 dark:bg-neutral-800">
        <div className="relative flex-1">
          <textarea
            value={currentInput}
            onChange={handleInputChange}
            onKeyDown={handleKeyDown}
            placeholder={showGhostTyping ? '' : 'Send a message'}
            disabled={isGenerating}
            rows={1}
            className="w-full resize-none bg-transparent px-2 pt-1 text-neutral-900 placeholder-neutral-400 focus:outline-none disabled:opacity-50 dark:text-white dark:placeholder-neutral-500"
            style={{ minHeight: '36px', maxHeight: '120px' }}
          />
          {showGhostTyping && ghostText && (
            <div
              className="pointer-events-none absolute top-0 left-0 px-2 pt-1 text-neutral-400 dark:text-neutral-500"
              aria-hidden="true"
            >
              {ghostText}
              <span className="animate-pulse">|</span>
            </div>
          )}
        </div>
        {isGenerating ? (
          <button
            type="button"
            onClick={onStopResponse}
            className="flex h-8 w-8 shrink-0 items-center justify-center text-neutral-600 dark:text-neutral-400"
          >
            <StopCircle className="h-4 w-4" />
          </button>
        ) : (
          <button
            type="button"
            onClick={onSendMessage}
            disabled={sendDisabled}
            className="flex h-8 w-8 shrink-0 items-center justify-center text-neutral-600 disabled:text-neutral-300 dark:text-neutral-400 dark:disabled:text-neutral-600"
          >
            <Send className="h-4 w-4" />
          </button>
        )}
      </div>
    </div>
  );
}

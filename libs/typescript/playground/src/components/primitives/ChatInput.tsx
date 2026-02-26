// Chat input component — thin orchestrator
// Sub-components: ComputerSelector, ModelSelector, MobileInputBar

import { Monitor, Send, StopCircle } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { Tooltip, TooltipContent, TooltipTrigger } from '../ui/tooltip';
import type { Computer, Model, ModelProvider, VM } from '../../types';
import { isVM } from '../../types';
import { ComputerSelector } from './ComputerSelector';
import { ModelSelector } from './ModelSelector';
import { MobileInputBar } from './MobileInputBar';

// ---------------------------------------------------------------------------
// Ghost typing hook
// ---------------------------------------------------------------------------

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
      if (charIndex < currentPrompt.length) {
        timeoutRef.current = setTimeout(() => {
          setGhostText(currentPrompt.slice(0, charIndex + 1));
          setCharIndex((prev) => prev + 1);
        }, 50);
      } else {
        timeoutRef.current = setTimeout(() => {
          setIsDeleting(true);
        }, 2000);
      }
    } else {
      if (charIndex > 0) {
        timeoutRef.current = setTimeout(() => {
          setGhostText(currentPrompt.slice(0, charIndex - 1));
          setCharIndex((prev) => prev - 1);
        }, 30);
      } else {
        setIsDeleting(false);
        setPromptIndex((prev) => (prev + 1) % GHOST_PROMPTS.length);
      }
    }

    return clearTimeoutRef;
  }, [isActive, promptIndex, charIndex, isDeleting, clearTimeoutRef]);

  return ghostText;
}

// ---------------------------------------------------------------------------
// ChatInput props & component
// ---------------------------------------------------------------------------

export interface ChatInputProps {
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

  // Add sandbox callback
  onAddComputer?: () => void;
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
  onAddComputer,
}: ChatInputProps) {
  // Lock model/computer selection after first message, but only if they're set
  const isModelLocked = hasMessages && !!selectedModel;
  const isComputerLocked = hasMessages && !!selectedComputer;

  const handleInputChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    onInputChange(e.target.value);
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

  // Ghost typing - only show when input is empty and no messages
  const showGhostTyping = !currentInput && !hasMessages && !isGenerating;
  const ghostText = useGhostTyping(showGhostTyping);

  // -------------------------------------------------------------------------
  // Mobile layout
  // -------------------------------------------------------------------------
  if (isMobile) {
    return (
      <MobileInputBar
        currentInput={currentInput}
        onInputChange={handleInputChange}
        onKeyDown={handleKeyDown}
        onSendMessage={onSendMessage}
        onStopResponse={onStopResponse}
        isGenerating={isGenerating}
        sendDisabled={sendDisabled}
        showGhostTyping={showGhostTyping}
        ghostText={ghostText}
        computers={computers}
        selectedComputer={selectedComputer}
        onComputerChange={onComputerChange}
        isComputerLocked={isComputerLocked}
        onAddComputer={onAddComputer}
        availableModels={availableModels}
        selectedModel={selectedModel}
        onModelChange={onModelChange}
        isModelLocked={isModelLocked}
        customModelId={customModelId}
      />
    );
  }

  // -------------------------------------------------------------------------
  // Desktop layout
  // -------------------------------------------------------------------------
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
        <ComputerSelector
          computers={computers}
          selectedComputer={selectedComputer}
          onComputerChange={onComputerChange}
          disabled={isComputerLocked}
          vmVersionInfo={vmVersionInfo}
          hasOrg={hasOrg}
          hasWorkspace={hasWorkspace}
          onAddComputer={onAddComputer}
        />

        <ModelSelector
          availableModels={availableModels}
          selectedModel={selectedModel}
          onModelChange={onModelChange}
          disabled={isModelLocked}
          customModelId={customModelId}
        />

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

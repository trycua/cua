// Mobile-specific chat input layout
// Extracted from ChatInput.tsx

import { Send, StopCircle } from 'lucide-react';
import type { Computer, Model, ModelProvider } from '../../types';
import { ComputerSelector } from './ComputerSelector';
import { ModelSelector } from './ModelSelector';

export interface MobileInputBarProps {
  // Core props
  currentInput: string;
  onInputChange: (e: React.ChangeEvent<HTMLTextAreaElement>) => void;
  onKeyDown: (e: React.KeyboardEvent<HTMLTextAreaElement>) => void;
  onSendMessage: () => void;
  onStopResponse: () => void;
  isGenerating: boolean;
  sendDisabled: boolean | undefined;

  // Ghost typing
  showGhostTyping: boolean;
  ghostText: string;

  // Computer selection
  computers: Computer[];
  selectedComputer?: Computer;
  onComputerChange: (computerId: string) => void;
  isComputerLocked: boolean;
  onAddComputer?: () => void;

  // Model selection
  availableModels: ModelProvider[];
  selectedModel?: Model;
  onModelChange: (modelId: string) => void;
  isModelLocked: boolean;
  customModelId?: string;
}

export function MobileInputBar({
  currentInput,
  onInputChange,
  onKeyDown,
  onSendMessage,
  onStopResponse,
  isGenerating,
  sendDisabled,
  showGhostTyping,
  ghostText,
  computers,
  selectedComputer,
  onComputerChange,
  isComputerLocked,
  onAddComputer,
  availableModels,
  selectedModel,
  onModelChange,
  isModelLocked,
  customModelId,
}: MobileInputBarProps) {
  return (
    <div className="space-y-2">
      {/* Mobile selectors row */}
      <div className="flex gap-2">
        <ComputerSelector
          computers={computers}
          selectedComputer={selectedComputer}
          onComputerChange={onComputerChange}
          disabled={isComputerLocked}
          onAddComputer={onAddComputer}
          mobile
        />

        <ModelSelector
          availableModels={availableModels}
          selectedModel={selectedModel}
          onModelChange={onModelChange}
          disabled={isModelLocked}
          customModelId={customModelId}
          mobile
        />
      </div>

      {/* Mobile input row */}
      <div className="flex items-end gap-2 rounded-2xl bg-neutral-100 p-2 dark:bg-neutral-800">
        <div className="relative flex-1">
          <textarea
            value={currentInput}
            onChange={onInputChange}
            onKeyDown={onKeyDown}
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

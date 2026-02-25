// Model selector dropdown component
// Extracted from ChatInput.tsx

import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from '../ui/select';
import type { Model, ModelProvider } from '../../types';

export interface ModelSelectorProps {
  availableModels: ModelProvider[];
  selectedModel?: Model;
  onModelChange: (modelId: string) => void;
  disabled?: boolean;
  customModelId?: string;
  /** When true, renders the mobile variant (flex-1, z-index on content) */
  mobile?: boolean;
}

export function ModelSelector({
  availableModels,
  selectedModel,
  onModelChange,
  disabled = false,
  customModelId,
  mobile = false,
}: ModelSelectorProps) {
  if (mobile) {
    return (
      <Select
        value={selectedModel?.id || ''}
        onValueChange={onModelChange}
        disabled={!!customModelId || disabled}
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
    );
  }

  // Desktop variant with preview badges
  return (
    <Select
      value={selectedModel?.id || ''}
      onValueChange={onModelChange}
      disabled={!!customModelId || disabled}
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
  );
}

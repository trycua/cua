// Step 2a: Preset selection grid for local Docker containers

import { ChevronRight } from 'lucide-react';
import { SANDBOX_PRESETS, MACOS_PRESET, type SandboxPreset } from '../../constants/sandboxPresets';

const ICON_MAP: Record<SandboxPreset['icon'], string> = {
  linux: '\u{1F427}',
  windows: '\u{1FA9F}',
  android: '\u{1F4F1}',
  macos: '\u{1F34E}',
};

export interface PresetGridProps {
  onSelect: (preset: SandboxPreset) => void;
}

export function PresetGrid({ onSelect }: PresetGridProps) {
  return (
    <div className="flex flex-col gap-3">
      {SANDBOX_PRESETS.map((preset) => (
        <button
          key={preset.id}
          type="button"
          onClick={() => onSelect(preset)}
          className="flex items-center gap-4 rounded-lg border border-neutral-200 p-4 text-left transition-colors hover:border-blue-400 hover:bg-blue-50/50 dark:border-neutral-600 dark:hover:border-blue-500 dark:hover:bg-blue-950/20"
        >
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-neutral-100 text-xl dark:bg-neutral-700">
            {ICON_MAP[preset.icon]}
          </div>
          <div className="flex-1">
            <p className="font-medium text-neutral-900 dark:text-white">{preset.name}</p>
            <p className="text-sm text-neutral-500 dark:text-neutral-400">{preset.description}</p>
          </div>
          <ChevronRight className="h-5 w-5 text-neutral-400" />
        </button>
      ))}

      {/* macOS informational card */}
      <div className="flex items-center gap-4 rounded-lg border border-dashed border-neutral-300 p-4 dark:border-neutral-600">
        <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-neutral-100 text-xl dark:bg-neutral-700">
          {ICON_MAP.macos}
        </div>
        <div className="flex-1">
          <p className="font-medium text-neutral-900 dark:text-white">{MACOS_PRESET.name}</p>
          <p className="text-sm text-neutral-500 dark:text-neutral-400">{MACOS_PRESET.note}</p>
        </div>
      </div>
    </div>
  );
}

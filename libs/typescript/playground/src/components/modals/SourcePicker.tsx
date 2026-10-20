// Step 1: Source selection cards (Local Docker, Custom URL)

import { ChevronRight, Globe, Monitor } from 'lucide-react';

export type SandboxSource = 'local' | 'custom';

export interface SourcePickerProps {
  onSelect: (source: SandboxSource) => void;
}

export function SourcePicker({ onSelect }: SourcePickerProps) {
  const cards: Array<{
    source: SandboxSource;
    icon: React.ReactNode;
    title: string;
    description: string;
  }> = [
    {
      source: 'local',
      icon: <Monitor className="h-6 w-6" />,
      title: 'Local (Docker)',
      description: 'Run a sandbox on your computer using Docker',
    },
    {
      source: 'custom',
      icon: <Globe className="h-6 w-6" />,
      title: 'Custom URL',
      description: 'Connect to any agent endpoint',
    },
  ];

  return (
    <div className="flex flex-col gap-3">
      {cards.map((card) => (
        <button
          key={card.source}
          type="button"
          onClick={() => onSelect(card.source)}
          className="flex items-center gap-4 rounded-lg border border-neutral-200 p-4 text-left transition-colors hover:border-blue-400 hover:bg-blue-50/50 dark:border-neutral-600 dark:hover:border-blue-500 dark:hover:bg-blue-950/20"
        >
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-neutral-100 text-neutral-600 dark:bg-neutral-700 dark:text-neutral-300">
            {card.icon}
          </div>
          <div className="flex-1">
            <p className="font-medium text-neutral-900 dark:text-white">{card.title}</p>
            <p className="text-sm text-neutral-500 dark:text-neutral-400">{card.description}</p>
          </div>
          <ChevronRight className="h-5 w-5 text-neutral-400" />
        </button>
      ))}
    </div>
  );
}

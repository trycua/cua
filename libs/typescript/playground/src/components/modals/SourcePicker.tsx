// Step 1: Source selection cards (Local Docker, Cloud Sandbox, Custom URL)

import { ChevronRight, Globe, Monitor, Server } from 'lucide-react';

export type SandboxSource = 'local' | 'cloud' | 'custom';

export interface SourcePickerProps {
  onSelect: (source: SandboxSource) => void;
  hasCloud: boolean;
}

export function SourcePicker({ onSelect, hasCloud }: SourcePickerProps) {
  const cards: Array<{
    source: SandboxSource;
    icon: React.ReactNode;
    title: string;
    description: string;
    hidden?: boolean;
  }> = [
    {
      source: 'local',
      icon: <Monitor className="h-6 w-6" />,
      title: 'Local (Docker)',
      description: 'Run a sandbox on your machine using Docker',
    },
    {
      source: 'cloud',
      icon: <Server className="h-6 w-6" />,
      title: 'Cloud Sandbox',
      description: 'Provision a cloud VM',
      hidden: !hasCloud,
    },
    {
      source: 'custom',
      icon: <Globe className="h-6 w-6" />,
      title: 'Custom URL',
      description: 'Connect to any computer-server endpoint',
    },
  ];

  return (
    <div className="flex flex-col gap-3">
      {cards
        .filter((c) => !c.hidden)
        .map((card) => (
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

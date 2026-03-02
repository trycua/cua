'use client';

import { useRouter } from 'next/navigation';
import { ChevronDown } from 'lucide-react';

export interface Version {
  version: string;
  href: string;
  isCurrent: boolean;
}

interface VersionSelectorProps {
  versions: Version[];
  current: string;
  className?: string;
}

export function VersionSelector({ versions, current, className = '' }: VersionSelectorProps) {
  const router = useRouter();

  const handleChange = (e: React.ChangeEvent<HTMLSelectElement>) => {
    router.push(e.target.value);
  };

  if (versions.length <= 1) {
    // Don't show selector if only one version
    return null;
  }

  return (
    <div className={`relative inline-flex items-center ${className}`}>
      <select
        value={versions.find((v) => v.version === current)?.href || ''}
        onChange={handleChange}
        className="appearance-none bg-fd-secondary text-fd-foreground border border-fd-border rounded-md px-3 py-1.5 pr-8 text-sm font-mono cursor-pointer hover:bg-fd-accent focus:outline-none focus:ring-2 focus:ring-fd-ring"
      >
        {versions.map((v) => (
          <option key={v.version} value={v.href}>
            v{v.version} {v.isCurrent ? '(latest)' : ''}
          </option>
        ))}
      </select>
      <ChevronDown className="absolute right-2 top-1/2 -translate-y-1/2 h-4 w-4 text-fd-muted-foreground pointer-events-none" />
    </div>
  );
}

// Static version badge for display
export function VersionBadge({ version, className = '' }: { version: string; className?: string }) {
  return (
    <span
      className={`inline-flex items-center px-2 py-1 bg-blue-100 dark:bg-blue-900 text-blue-800 dark:text-blue-200 rounded text-sm font-mono ${className}`}
    >
      v{version}
    </span>
  );
}

// Combined component with version selector and current version display
export function VersionHeader({
  versions,
  currentVersion,
  fullVersion,
  packageName,
  installCommand,
}: {
  versions: Version[];
  currentVersion: string;
  fullVersion: string;
  packageName: string;
  installCommand?: string;
}) {
  return (
    <div className="flex flex-wrap items-center gap-3 mb-6">
      <VersionSelector versions={versions} current={currentVersion} />
      <VersionBadge version={fullVersion} />
      <span className="text-sm text-fd-muted-foreground">
        {installCommand ?? `pip install ${packageName}`}
      </span>
    </div>
  );
}

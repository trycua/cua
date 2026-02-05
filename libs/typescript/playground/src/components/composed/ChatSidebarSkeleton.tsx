// ChatSidebarSkeleton component
// Adapted from cloud/src/website/app/components/playground/ChatSidebarSkeleton.tsx

import { Skeleton } from '../ui/skeleton';
import { cn } from '../../utils/cn';

interface ChatSidebarSkeletonProps {
  /** Custom class name */
  className?: string;
  /** Number of chat item skeletons to show */
  itemCount?: number;
}

export function ChatSidebarSkeleton({ className, itemCount = 5 }: ChatSidebarSkeletonProps) {
  return (
    <div className={cn('flex flex-col p-4', className)}>
      {/* New Chat button skeleton */}
      <Skeleton className="mb-6 h-9 w-full rounded-md" />

      {/* Task History label */}
      <div className="mb-3 px-2">
        <Skeleton className="h-3 w-20" />
      </div>

      {/* Chat item skeletons */}
      <div className="space-y-1">
        {[...Array(itemCount)].map((_, i) => (
          <div
            key={`chat-skeleton-${i}`}
            className="flex items-center gap-2 rounded-md px-2 py-1.5"
          >
            <Skeleton className="h-4 flex-1" />
          </div>
        ))}
      </div>
    </div>
  );
}

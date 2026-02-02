'use client';

import { useEffect, useRef, useState } from 'react';
import { usePathname } from 'next/navigation';
import Link from 'next/link';
import { ChevronDown, X } from 'lucide-react';
import { cn } from 'fumadocs-ui/utils/cn';
import type * as PageTree from 'fumadocs-core/page-tree';

interface MobileSidebarProps {
  open: boolean;
  onClose: () => void;
  pageTree: PageTree.Root;
}

export function MobileSidebar({ open, onClose, pageTree }: MobileSidebarProps) {
  const pathname = usePathname();
  const previousPathname = useRef(pathname);

  // Close on navigation
  useEffect(() => {
    if (pathname !== previousPathname.current) {
      onClose();
      previousPathname.current = pathname;
    }
  }, [pathname, onClose]);

  // Lock body scroll when open
  useEffect(() => {
    document.body.style.overflow = open ? 'hidden' : '';
    return () => { document.body.style.overflow = ''; };
  }, [open]);

  // Close on Escape
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    if (open) document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [open, onClose]);

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 md:hidden">
      {/* Backdrop */}
      <div
        className="absolute inset-0 bg-black/60 backdrop-blur-sm"
        onClick={onClose}
      />

      {/* Panel */}
      <div className="absolute inset-y-0 left-0 right-0 top-14 bg-fd-background flex flex-col">
        {/* Close button */}
        <div className="flex justify-end p-2 border-b border-fd-border">
          <button
            onClick={onClose}
            className="p-2 rounded-md text-fd-muted-foreground hover:bg-fd-accent"
          >
            <X className="h-5 w-5" />
          </button>
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto p-4">
          <TreeNodes nodes={pageTree.children} pathname={pathname} level={1} />
        </div>
      </div>
    </div>
  );
}

function TreeNodes({ nodes, pathname, level }: { nodes: PageTree.Node[]; pathname: string; level: number }) {
  return (
    <div className="flex flex-col gap-1">
      {nodes.map((node, index) => (
        <TreeNode
          key={node.type === 'separator' ? `sep-${index}` : node.$id ?? index}
          node={node}
          pathname={pathname}
          level={level}
        />
      ))}
    </div>
  );
}

function TreeNode({ node, pathname, level }: { node: PageTree.Node; pathname: string; level: number }) {
  if (node.type === 'separator') {
    return (
      <p className="mt-6 mb-2 text-xs font-semibold text-fd-muted-foreground uppercase tracking-wider first:mt-0">
        {node.name}
      </p>
    );
  }

  if (node.type === 'folder') {
    return <FolderNode folder={node} pathname={pathname} level={level} />;
  }

  const isActive = pathname === node.url;
  const indent = (level - 1) * 16;

  return (
    <Link
      href={node.url}
      className={cn(
        'flex items-center gap-2 rounded-md px-3 py-2 text-sm',
        isActive
          ? 'bg-fd-primary/10 text-fd-primary font-medium'
          : 'text-fd-muted-foreground hover:bg-fd-accent hover:text-fd-foreground'
      )}
      style={{ marginLeft: indent }}
    >
      {node.icon}
      {node.name}
    </Link>
  );
}

function FolderNode({ folder, pathname, level }: { folder: PageTree.Folder; pathname: string; level: number }) {
  const hasActiveChild = checkActiveChild(folder.children, pathname);
  const isActive = folder.index ? pathname === folder.index.url : false;
  const [isOpen, setIsOpen] = useState(hasActiveChild || isActive || level === 1);
  const indent = (level - 1) * 16;

  useEffect(() => {
    if (hasActiveChild || isActive) setIsOpen(true);
  }, [hasActiveChild, isActive]);

  return (
    <div>
      <div
        className="flex items-center"
        style={{ marginLeft: indent }}
      >
        {folder.index ? (
          <Link
            href={folder.index.url}
            className={cn(
              'flex-1 flex items-center gap-2 rounded-md px-3 py-2 text-sm',
              isActive
                ? 'bg-fd-primary/10 text-fd-primary font-medium'
                : 'text-fd-foreground hover:bg-fd-accent'
            )}
            onClick={() => setIsOpen(true)}
          >
            {folder.icon}
            {folder.name}
          </Link>
        ) : (
          <button
            className="flex-1 flex items-center gap-2 rounded-md px-3 py-2 text-sm text-fd-foreground hover:bg-fd-accent text-left"
            onClick={() => setIsOpen(!isOpen)}
          >
            {folder.icon}
            {folder.name}
          </button>
        )}

        {folder.children.length > 0 && (
          <button
            className="p-2 text-fd-muted-foreground hover:text-fd-foreground"
            onClick={() => setIsOpen(!isOpen)}
          >
            <ChevronDown className={cn('h-4 w-4 transition-transform', !isOpen && '-rotate-90')} />
          </button>
        )}
      </div>

      {isOpen && folder.children.length > 0 && (
        <TreeNodes nodes={folder.children} pathname={pathname} level={level + 1} />
      )}
    </div>
  );
}

function checkActiveChild(nodes: PageTree.Node[], pathname: string): boolean {
  for (const node of nodes) {
    if (node.type === 'page' && pathname === node.url) return true;
    if (node.type === 'folder') {
      if (node.index && pathname === node.index.url) return true;
      if (checkActiveChild(node.children, pathname)) return true;
    }
  }
  return false;
}

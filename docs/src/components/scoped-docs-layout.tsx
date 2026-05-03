'use client';

import { usePathname } from 'next/navigation';
import { DocsLayout } from 'fumadocs-ui/layouts/docs';
import type { Root, Node, Folder } from 'fumadocs-core/page-tree';
import type { ReactNode } from 'react';

// Must match the navTabs prefixes defined in custom-header.tsx
const SECTION_PREFIXES = [
  '/cua/guide',
  '/cua/examples',
  '/cua/reference',
  '/cuabench/guide',
  '/cuabench/examples',
  '/cuabench/reference',
  '/lume/guide',
  '/lume/examples',
  '/lume/reference',
  '/cuabot/guide',
  '/cuabot/reference',
];

function hasDescendantWithPrefix(nodes: Node[], prefix: string): boolean {
  for (const node of nodes) {
    if (node.type === 'page' && node.url.startsWith(prefix)) return true;
    if (node.type === 'folder') {
      if (node.index?.url?.startsWith(prefix)) return true;
      if (hasDescendantWithPrefix(node.children, prefix)) return true;
    }
  }
  return false;
}

function findFolder(nodes: Node[], prefix: string): Folder | null {
  for (const node of nodes) {
    if (node.type !== 'folder') continue;
    if (!hasDescendantWithPrefix([node], prefix)) continue;

    // This folder is the right level if its direct children match the prefix
    const isDirectParent = node.children.some(
      (c: Node) =>
        (c.type === 'page' && c.url.startsWith(prefix)) ||
        (c.type === 'folder' &&
          (c.index?.url?.startsWith(prefix) || hasDescendantWithPrefix(c.children, prefix)))
    );

    if (isDirectParent) return node;

    const deeper = findFolder(node.children, prefix);
    if (deeper) return deeper;
  }
  return null;
}

function filterTree(tree: Root, prefix: string): Root {
  const folder = findFolder(tree.children, prefix);
  if (!folder) return tree;
  return { ...tree, children: folder.children };
}

interface Props {
  tree: Root;
  children: ReactNode;
}

export function ScopedDocsLayout({ tree, children }: Props) {
  const pathname = usePathname();
  const prefix = SECTION_PREFIXES.find((p) => pathname.startsWith(p)) ?? null;
  const scopedTree = prefix ? filterTree(tree, prefix) : tree;

  return (
    <DocsLayout
      tree={scopedTree}
      nav={{ enabled: false }}
      searchToggle={{ enabled: false }}
      themeSwitch={{ enabled: false }}
      sidebar={{
        tabs: false,
        collapsible: false,
        banner: <div className="h-6" />,
      }}
    >
      {children}
    </DocsLayout>
  );
}

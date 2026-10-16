// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { memo, type MouseEvent, type ReactNode } from "react";

import { parseBlocks, parseInline, type Block, type Inline } from "@/lib/markdown";
import { cn } from "@/lib/utils";

/**
 * An agent message as Markdown. Blocks are memoized by their own source, so
 * while a message streams only its last block renders again.
 */
export function Markdown({ text, onOpenLink, className }: { text: string; onOpenLink?: (url: string) => void; className?: string }) {
  const blocks = parseBlocks(text);
  return (
    <div className={cn("markdown text-[13px] leading-[1.6] text-foreground", className)}>
      {blocks.map((b, i) => (
        <MarkdownBlock key={i} source={b.source} block={b.block} onOpenLink={onOpenLink} />
      ))}
    </div>
  );
}

const MarkdownBlock = memo(
  function MarkdownBlock({ block, onOpenLink }: { source: string; block: Block; onOpenLink?: (url: string) => void }) {
    const inline = (t: string) => <Inlines nodes={parseInline(t)} onOpenLink={onOpenLink} />;
    switch (block.type) {
      case "paragraph":
        return <p className="my-2 first:mt-0 last:mb-0">{inline(block.text)}</p>;
      case "heading": {
        const size = block.level <= 1 ? "text-[15px]" : block.level === 2 ? "text-[14px]" : "text-[13px]";
        return <p className={cn("mt-4 mb-1.5 font-semibold first:mt-0", size)}>{inline(block.text)}</p>;
      }
      case "rule":
        return <hr className="my-4 border-border" />;
      case "quote":
        return <blockquote className="my-2 border-l-2 pl-3 text-muted-foreground">{inline(block.text)}</blockquote>;
      case "list": {
        const List = block.ordered ? "ol" : "ul";
        return (
          <List
            start={block.ordered && block.start !== 1 ? block.start : undefined}
            className={cn("my-2 space-y-1 pl-5 first:mt-0 last:mb-0", block.ordered ? "list-decimal" : "list-disc", "marker:text-muted-foreground")}
          >
            {block.items.map((item, i) => (
              <li key={i} className="pl-0.5">
                {inline(item)}
              </li>
            ))}
          </List>
        );
      }
      case "code":
        return <CodeBlock lang={block.lang} code={block.code} />;
    }
  },
  (a, b) => a.source === b.source && a.onOpenLink === b.onOpenLink,
);

function CodeBlock({ lang, code }: { lang: string; code: string }) {
  const diff = lang === "diff";
  return (
    <div className="my-3 overflow-hidden rounded-lg border bg-muted/40 first:mt-0 last:mb-0">
      {lang ? <div className="border-b px-3 py-1 font-mono text-2xs text-muted-foreground">{lang}</div> : null}
      <pre className="overflow-x-auto px-3 py-2.5 font-mono text-xs leading-[1.55]">
        {diff ? (
          code.split("\n").map((line, i) => (
            <div
              key={i}
              className={cn(
                "-mx-3 px-3",
                line.startsWith("+") && "bg-success/10 text-success",
                line.startsWith("-") && "bg-destructive/10 text-destructive",
              )}
            >
              {line || " "}
            </div>
          ))
        ) : (
          <code>{code}</code>
        )}
      </pre>
    </div>
  );
}

function Inlines({ nodes, onOpenLink }: { nodes: Inline[]; onOpenLink?: (url: string) => void }): ReactNode {
  return nodes.map((n, i) => {
    switch (n.type) {
      case "text":
        return n.text;
      case "break":
        return <br key={i} />;
      case "code":
        return (
          <code key={i} className="rounded-[4px] bg-muted px-1 py-px font-mono text-[0.92em]">
            {n.text}
          </code>
        );
      case "strong":
        return (
          <strong key={i} className="font-semibold">
            <Inlines nodes={n.children} onOpenLink={onOpenLink} />
          </strong>
        );
      case "em":
        return (
          <em key={i}>
            <Inlines nodes={n.children} onOpenLink={onOpenLink} />
          </em>
        );
      case "link": {
        const open = (e: MouseEvent) => {
          if (!onOpenLink) return;
          e.preventDefault();
          onOpenLink(n.href);
        };
        return (
          <a key={i} href={n.href} target="_blank" rel="noreferrer" onClick={open} className="text-brand-strong underline decoration-brand/40 underline-offset-2 hover:decoration-brand">
            <Inlines nodes={n.children} onOpenLink={onOpenLink} />
          </a>
        );
      }
    }
  });
}

'use client';

import { useState } from 'react';
import { SiOpenai, SiAnthropic, SiMarkdown, SiGithub } from 'react-icons/si';

interface DocActionsMenuProps {
  pageUrl: string;
  pageTitle: string;
  filePath?: string;
}

export function DocActionsMenu({ pageUrl, pageTitle, filePath }: DocActionsMenuProps) {
  const [copied, setCopied] = useState(false);

  const handleCopyMarkdown = async () => {
    try {
      if (!filePath) {
        throw new Error('No file path available');
      }
      const githubRawUrl = `https://raw.githubusercontent.com/trycua/cua/refs/heads/main/docs/content/docs/${filePath}`;

      const response = await fetch(githubRawUrl);
      if (!response.ok) {
        throw new Error('Failed to fetch markdown');
      }
      const markdown = await response.text();

      await navigator.clipboard.writeText(markdown);

      setCopied(true);
      setTimeout(() => setCopied(false), 2000);

    } catch (error) {
      console.error('Error copying markdown:', error);

      try {
        const urlWithUtm = `https://cua.ai${pageUrl}?utm_source=cua.ai/docs`;
        await navigator.clipboard.writeText(urlWithUtm);
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      } catch (fallbackError) {
        console.error('Error copying URL:', fallbackError);
      }

    }
  };

  const handleEditGithub = () => {
    if (!filePath) {
      return;
    }

    const githubEditUrl = `https://github.com/trycua/cua/edit/main/docs/content/docs/${filePath}`;
    window.open(githubEditUrl, '_blank', 'noopener,noreferrer');
  };

  const handleOpenChatGPT = () => {

    const docUrl = `https://cua.ai${pageUrl}?utm_source=cua.ai/docs`;
    const prompt = `I need help understanding this cua.ai documentation page: "${pageTitle}". Please read and help me with: ${docUrl}`;
    const chatgptUrl = `https://chatgpt.com/?q=${encodeURIComponent(prompt)}`;
    window.open(chatgptUrl, '_blank', 'noopener,noreferrer');
  };

  const handleOpenClaude = () => {

    const docUrl = `https://cua.ai${pageUrl}?utm_source=cua.ai/docs`;
    const prompt = `I need help understanding this cua.ai documentation page: "${pageTitle}". Please read and help me with: ${docUrl}`;
    const claudeUrl = `https://claude.ai/new?q=${encodeURIComponent(prompt)}`;
    window.open(claudeUrl, '_blank', 'noopener,noreferrer');
  };

  return (
    <div className="flex flex-col gap-2">
      <button
        onClick={handleCopyMarkdown}
        className="inline-flex gap-3 w-full items-center rounded-md p-1 text-sm hover:bg-fd-accent hover:text-fd-accent-foreground text-left transition-colors px-2 hover:cursor-pointer"
      >
        <SiMarkdown className="w-2 h-4 flex-shrink-0" />
        <span>{copied ? 'Copied!' : 'Copy as markdown'}</span>
      </button>

      <button
        onClick={handleEditGithub}
        className="inline-flex gap-3 w-full items-center rounded-md p-1 text-sm hover:bg-fd-accent hover:text-fd-accent-foreground text-left transition-colors px-2 hover:cursor-pointer"
      >
        <SiGithub className="w-4 h-4 flex-shrink-0" />
        <span>Edit on GitHub</span>
      </button>

      <button
        onClick={handleOpenChatGPT}
        className="inline-flex gap-3 w-full items-center rounded-md p-1 text-sm hover:bg-fd-accent hover:text-fd-accent-foreground text-left transition-colors px-2 hover:cursor-pointer"
      >
        <SiOpenai className="w-4 h-4 flex-shrink-0" />
        <span>Open in ChatGPT</span>
      </button>

      <button
        onClick={handleOpenClaude}
        className="inline-flex gap-3 w-full items-center rounded-md p-1 text-sm hover:bg-fd-accent hover:text-fd-accent-foreground text-left transition-colors px-2 hover:cursor-pointer"
      >
        <SiAnthropic className="w-4 h-4 flex-shrink-0" />
        <span>Open in Claude</span>
      </button>
    </div>
  );
}

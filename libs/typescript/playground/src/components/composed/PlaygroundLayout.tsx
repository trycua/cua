// Playground layout component
// Adapted from cloud/src/website/app/components/playground/PlaygroundLayout.tsx

import {
  ArrowLeft,
  Download,
  Menu,
  Moon,
  PanelLeftClose,
  PanelLeftOpen,
  Play,
  Sun,
  X,
} from 'lucide-react';
import { motion } from 'motion/react';
import { type ReactNode, useEffect, useState } from 'react';
import { ChatList } from './ChatList';
import { useActiveChat, usePlayground } from '../../hooks/usePlayground';
import type { Chat } from '../../types';

interface PlaygroundLayoutProps {
  /** Main content to render */
  children: ReactNode;
  /** Whether dark mode is enabled */
  isDarkMode?: boolean;
  /** Callback to toggle theme */
  onToggleTheme?: () => void;
  /** Custom render function for theme toggle (for animated toggles) */
  renderThemeToggle?: (props: { isDarkMode: boolean; onToggle: () => void }) => ReactNode;
  /** Render function for custom link (e.g., react-router Link) */
  renderLink?: (props: { to: string; children: ReactNode; className?: string }) => ReactNode;
  /** Custom back link URL (defaults to '/dashboard') */
  backLinkUrl?: string;
  /** Custom back link text (defaults to 'Back to Dashboard') */
  backLinkText?: string;
  /** Whether to show the preview badge */
  showPreviewBadge?: boolean;
  /** Callback for export action */
  onExportChat?: (chat: Chat) => void;
  /** Callback for replay action */
  onReplayChat?: (chat: Chat) => void;
  /** Optional loading bar component */
  loadingBar?: ReactNode;
  /** Optional sidebar skeleton component */
  sidebarSkeleton?: ReactNode;
  /** Optional VNC overlay panel */
  vncOverlayPanel?: ReactNode;
  /** Whether chats are loading */
  chatsLoading?: boolean;
  /** Callback when chat is selected */
  onChatSelect?: () => void;
  /** Toast callback for user notifications */
  onToast?: (message: string, type?: 'success' | 'error' | 'info') => void;
}

export function PlaygroundLayout({
  children,
  isDarkMode = false,
  onToggleTheme,
  renderThemeToggle,
  renderLink,
  backLinkUrl = '/dashboard',
  backLinkText = 'Back to Dashboard',
  showPreviewBadge = true,
  onExportChat,
  onReplayChat,
  loadingBar,
  sidebarSkeleton,
  vncOverlayPanel,
  chatsLoading = false,
  onChatSelect,
  onToast,
}: PlaygroundLayoutProps) {
  const { state, dispatch } = usePlayground();
  const { isSidebarCollapsed } = state;
  const activeChat = useActiveChat();
  const [isMobileSidebarOpen, setIsMobileSidebarOpen] = useState(false);
  const [isMobile, setIsMobile] = useState(false);

  // Check if mobile on mount and window resize
  useEffect(() => {
    const checkMobile = () => {
      setIsMobile(window.innerWidth < 768); // 768px is md breakpoint
    };

    checkMobile();
    window.addEventListener('resize', checkMobile);
    return () => window.removeEventListener('resize', checkMobile);
  }, []);

  const handleSidebarToggle = () => {
    dispatch({ type: 'TOGGLE_SIDEBAR' });
  };

  const handleChatSelect = () => {
    setIsMobileSidebarOpen(false);
    onChatSelect?.();
  };

  // Default link renderer (just renders an anchor)
  const defaultRenderLink = ({
    to,
    children,
    className,
  }: {
    to: string;
    children: ReactNode;
    className?: string;
  }) => (
    <a href={to} className={className}>
      {children}
    </a>
  );

  const LinkComponent = renderLink || defaultRenderLink;

  return (
    <div className="relative flex h-screen w-screen flex-col overflow-hidden bg-white dark:bg-[#0C0C0C]">
      {loadingBar}

      {/* Top header bar */}
      <div className="flex h-12 flex-shrink-0 items-center justify-between border-neutral-200 border-b bg-white px-4 dark:border-neutral-800 dark:bg-[#0C0C0C]">
        <LinkComponent
          to={backLinkUrl}
          className="flex items-center gap-2 text-neutral-600 text-sm transition-colors hover:text-neutral-900 dark:text-neutral-400 dark:hover:text-white"
        >
          <ArrowLeft className="h-4 w-4" />
          <span>{backLinkText}</span>
        </LinkComponent>

        <div className="flex items-center gap-3">
          {/* Replay and Export buttons - only show when there's an active chat with messages */}
          {activeChat?.messages && activeChat.messages.length > 0 && (
            <>
              {onReplayChat && (
                <button
                  type="button"
                  onClick={() => onReplayChat(activeChat)}
                  className="flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-neutral-600 text-sm transition-colors hover:bg-neutral-100 hover:text-neutral-900 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-white"
                  title="Replay trajectory"
                >
                  <Play className="h-4 w-4" />
                  <span className="hidden sm:inline">Replay</span>
                </button>
              )}
              {onExportChat && (
                <button
                  type="button"
                  onClick={() => onExportChat(activeChat)}
                  className="flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-neutral-600 text-sm transition-colors hover:bg-neutral-100 hover:text-neutral-900 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-white"
                  title="Export trajectory"
                >
                  <Download className="h-4 w-4" />
                  <span className="hidden sm:inline">Export</span>
                </button>
              )}
            </>
          )}

          {/* Preview label */}
          {showPreviewBadge && (
            <div className="flex h-7 items-center rounded-full border border-blue-500/20 bg-blue-500/10 px-3 font-medium text-blue-600 text-xs dark:border-blue-500/30 dark:bg-blue-500/20 dark:text-blue-400">
              Preview
            </div>
          )}

          {/* Theme toggle button */}
          {onToggleTheme &&
            (renderThemeToggle ? (
              renderThemeToggle({ isDarkMode, onToggle: onToggleTheme })
            ) : (
              <button
                type="button"
                onClick={onToggleTheme}
                className="flex h-8 w-8 items-center justify-center rounded-md text-neutral-500 transition-colors hover:bg-neutral-100 hover:text-neutral-900 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-white"
                aria-label="Toggle theme"
              >
                {isDarkMode ? <Sun className="h-4 w-4" /> : <Moon className="h-4 w-4" />}
              </button>
            ))}
        </div>
      </div>

      {/* Mobile menu button - top left corner, only visible on mobile */}
      <button
        type="button"
        onClick={() => setIsMobileSidebarOpen(!isMobileSidebarOpen)}
        className="absolute top-16 left-4 z-50 flex h-10 w-10 items-center justify-center rounded-full bg-neutral-100 text-neutral-600 transition-colors hover:bg-neutral-200 hover:text-neutral-900 md:hidden dark:bg-neutral-800 dark:text-neutral-400 dark:hover:bg-neutral-700 dark:hover:text-white"
      >
        {isMobileSidebarOpen ? <X className="h-5 w-5" /> : <Menu className="h-5 w-5" />}
      </button>

      <div className="flex flex-1 overflow-hidden">
        {/* Mobile backdrop overlay */}
        {isMobileSidebarOpen && (
          <motion.button
            type="button"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.2 }}
            className="absolute inset-0 z-40 bg-black/50 md:hidden"
            onClick={() => setIsMobileSidebarOpen(false)}
            aria-label="Close sidebar"
          />
        )}

        {/* Sidebar - Chat history */}
        <motion.div
          animate={{
            width: isSidebarCollapsed ? 0 : 288,
            x: isMobile ? (isMobileSidebarOpen ? 0 : -288) : 0,
            opacity: isSidebarCollapsed ? 0 : 1,
          }}
          transition={{ duration: 0.3, ease: [0.4, 0, 0.2, 1] }}
          className="absolute inset-y-0 left-0 z-50 w-72 overflow-hidden border-neutral-200 border-r bg-neutral-50 p-4 pt-16 md:relative md:z-auto md:p-4 md:pt-4 dark:border-neutral-800 dark:bg-[#0C0C0C]"
          style={{
            padding: isSidebarCollapsed ? 0 : undefined,
          }}
        >
          {chatsLoading && sidebarSkeleton ? (
            sidebarSkeleton
          ) : (
            <ChatList
              onChatSelect={handleChatSelect}
              onExportChat={onExportChat}
              onReplayChat={onReplayChat}
              onToast={onToast}
            />
          )}
        </motion.div>

        {/* Main content area */}
        <div className="relative flex flex-1 flex-col overflow-hidden">
          {/* Collapse/Expand button - positioned at the top left of content area */}
          {!isMobile && (
            <button
              type="button"
              onClick={handleSidebarToggle}
              className="absolute top-3 left-3 z-[60] flex h-8 w-8 items-center justify-center rounded-full text-neutral-500 transition-colors hover:bg-neutral-100 hover:text-neutral-900 dark:text-neutral-500 dark:hover:bg-neutral-800 dark:hover:text-white"
            >
              {isSidebarCollapsed ? (
                <PanelLeftOpen className="h-4 w-4" />
              ) : (
                <PanelLeftClose className="h-4 w-4" />
              )}
            </button>
          )}
          {children}
        </div>
      </div>

      {/* VNC overlay panel slot */}
      {vncOverlayPanel}
    </div>
  );
}

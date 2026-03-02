import { GripVertical, Maximize2, Minimize2, Monitor, X } from 'lucide-react';
import { AnimatePresence, motion, useDragControls } from 'motion/react';
import {
  type PointerEvent as ReactPointerEvent,
  useCallback,
  useEffect,
  useRef,
  useState,
} from 'react';
import VNCIframe from './VNCIframe';

// Inline useIsMobile hook since the playground package doesn't have this hook
function useIsMobile(): boolean {
  const [isMobile, setIsMobile] = useState(false);

  useEffect(() => {
    const checkMobile = () => {
      setIsMobile(window.innerWidth < 768);
    };

    checkMobile();
    window.addEventListener('resize', checkMobile);
    return () => window.removeEventListener('resize', checkMobile);
  }, []);

  return isMobile;
}

interface VNCOverlayPanelProps {
  vncUrl: string | null;
  isOpen: boolean;
  onToggle: () => void;
  width?: number;
  height?: number;
}

const MIN_WIDTH = 240;
const MAX_WIDTH = 800;
const MIN_HEIGHT = 135; // Roughly 16:9 at MIN_WIDTH
const DEFAULT_WIDTH = 320;
const DEFAULT_HEIGHT = 180;
const HEADER_HEIGHT = 32;
const MARGIN = 24;

/**
 * Floating VNC mini-player that appears at the bottom-left of the screen,
 * similar to YouTube's mini-player. Hidden on mobile devices.
 * Supports dragging within the viewport and resizing by dragging edges/corners.
 */
export function VNCOverlayPanel({
  vncUrl,
  isOpen,
  onToggle,
  width: propWidth,
  height: propHeight,
}: VNCOverlayPanelProps) {
  const isMobile = useIsMobile();
  const dragControls = useDragControls();
  const constraintsRef = useRef<HTMLDivElement>(null);

  // Panel size state
  const [size, setSize] = useState({
    width: DEFAULT_WIDTH,
    height: DEFAULT_HEIGHT,
  });

  // Panel position state (top-left based)
  const [position, setPosition] = useState<{ x: number; y: number } | null>(null);

  // Resize state
  const [isResizing, setIsResizing] = useState(false);
  const resizeRef = useRef<{
    startX: number;
    startY: number;
    startWidth: number;
    startHeight: number;
    direction: string;
  } | null>(null);

  // Initialize position on first render (bottom-left of viewport)
  useEffect(() => {
    if (typeof window !== 'undefined' && position === null) {
      setPosition({
        x: MARGIN,
        y: window.innerHeight - (size.height + HEADER_HEIGHT) - MARGIN,
      });
    }
  }, [position, size.height]);

  // Handle resize move - must be declared before handleResizeEnd
  const handleResizeMove = useCallback((e: globalThis.PointerEvent) => {
    if (!resizeRef.current) return;

    const { startX, startY, startWidth, startHeight, direction } = resizeRef.current;
    const deltaX = e.clientX - startX;
    const deltaY = e.clientY - startY;

    let newWidth = startWidth;
    let newHeight = startHeight;

    // Calculate new dimensions based on direction
    if (direction.includes('e')) {
      newWidth = Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, startWidth + deltaX));
    }
    if (direction.includes('w')) {
      newWidth = Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, startWidth - deltaX));
    }
    if (direction.includes('s')) {
      newHeight = Math.max(MIN_HEIGHT, startHeight + deltaY);
    }
    if (direction.includes('n')) {
      newHeight = Math.max(MIN_HEIGHT, startHeight - deltaY);
    }

    setSize({ width: newWidth, height: newHeight });
  }, []);

  // Handle resize end
  const handleResizeEnd = useCallback(() => {
    setIsResizing(false);
    resizeRef.current = null;
    document.removeEventListener('pointermove', handleResizeMove);
    document.removeEventListener('pointerup', handleResizeEnd);
  }, [handleResizeMove]);

  // Cleanup listeners on unmount
  useEffect(() => {
    return () => {
      document.removeEventListener('pointermove', handleResizeMove);
      document.removeEventListener('pointerup', handleResizeEnd);
    };
  }, [handleResizeMove, handleResizeEnd]);

  // Don't render anything on mobile
  if (isMobile) {
    return null;
  }

  // Don't render if no VNC URL available
  if (!vncUrl) {
    return null;
  }

  // Handle resize start
  const handleResizeStart = (e: ReactPointerEvent<HTMLDivElement>, direction: string) => {
    e.preventDefault();
    e.stopPropagation();
    setIsResizing(true);
    resizeRef.current = {
      startX: e.clientX,
      startY: e.clientY,
      startWidth: size.width,
      startHeight: size.height,
      direction,
    };

    // Add global listeners
    document.addEventListener('pointermove', handleResizeMove);
    document.addEventListener('pointerup', handleResizeEnd);
  };

  // Cycle through preset sizes
  const cycleSize = () => {
    const presets = [
      { width: 280, height: 158 },
      { width: 360, height: 203 },
      { width: 480, height: 270 },
    ];

    // Find closest preset and cycle to next
    const currentArea = size.width * size.height;
    let closestIndex = 0;
    let closestDiff = Infinity;

    presets.forEach((preset, index) => {
      const diff = Math.abs(preset.width * preset.height - currentArea);
      if (diff < closestDiff) {
        closestDiff = diff;
        closestIndex = index;
      }
    });

    const nextIndex = (closestIndex + 1) % presets.length;
    setSize(presets[nextIndex]);
  };

  // Check if current size is "large"
  const isLargeSize = size.width >= 440;

  // Start drag from header
  const startDrag = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (isResizing) return;
    dragControls.start(e);
  };

  const totalHeight = size.height + HEADER_HEIGHT;

  return (
    <>
      {/* Invisible constraints container that covers the viewport */}
      <div ref={constraintsRef} className="pointer-events-none fixed inset-0" />

      {/* Floating toggle button - visible when mini-player is closed */}
      <AnimatePresence>
        {!isOpen && (
          <motion.button
            type="button"
            initial={{ opacity: 0, scale: 0.8 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.8 }}
            transition={{ duration: 0.2, ease: [0.4, 0, 0.2, 1] }}
            onClick={onToggle}
            className="group fixed bottom-6 left-6 z-[70] flex h-12 w-12 items-center justify-center rounded-full bg-neutral-900 text-white shadow-lg transition-colors hover:bg-neutral-800 dark:bg-neutral-700 dark:hover:bg-neutral-600"
          >
            <Monitor className="h-5 w-5" />
            {/* Tooltip on hover - positioned to the right */}
            <span className="-translate-y-1/2 pointer-events-none absolute top-1/2 left-full ml-3 whitespace-nowrap rounded-md bg-neutral-800 px-3 py-1.5 text-white text-xs opacity-0 shadow-lg transition-opacity group-hover:opacity-100 dark:bg-neutral-700">
              Click for live view of computer
            </span>
          </motion.button>
        )}
      </AnimatePresence>

      {/* Floating mini-player - draggable and resizable */}
      <AnimatePresence>
        {isOpen && position && (
          <motion.div
            drag
            dragControls={dragControls}
            dragListener={false}
            dragMomentum={false}
            dragElastic={0}
            dragConstraints={constraintsRef}
            initial={{
              opacity: 0,
              scale: 0.95,
              x: position.x,
              y: position.y,
            }}
            animate={{
              opacity: 1,
              scale: 1,
              width: size.width,
              height: totalHeight,
            }}
            exit={{ opacity: 0, scale: 0.95 }}
            transition={{
              duration: 0.2,
              ease: [0.4, 0, 0.2, 1],
              width: { duration: isResizing ? 0 : 0.2 },
              height: { duration: isResizing ? 0 : 0.2 },
            }}
            className="fixed top-0 left-0 z-[70] flex flex-col overflow-hidden rounded-xl bg-neutral-900 shadow-2xl"
            style={{
              cursor: isResizing ? 'grabbing' : undefined,
            }}
          >
            {/* Header with title and controls - drag handle */}
            <div
              onPointerDown={startDrag}
              className="flex h-8 flex-shrink-0 cursor-grab items-center justify-between bg-neutral-800 px-1 active:cursor-grabbing"
            >
              <div className="flex items-center gap-1 text-neutral-300 text-xs">
                {/* Drag grip indicator */}
                <GripVertical className="h-4 w-4 text-neutral-500" />
                <Monitor className="h-3.5 w-3.5" />
                <span>Live View</span>
              </div>
              <div className="flex items-center gap-1">
                {/* Resize button */}
                <button
                  type="button"
                  onClick={cycleSize}
                  className="flex h-6 w-6 items-center justify-center rounded text-neutral-400 transition-colors hover:bg-neutral-700 hover:text-white"
                  title="Cycle size presets"
                >
                  {isLargeSize ? (
                    <Minimize2 className="h-3.5 w-3.5" />
                  ) : (
                    <Maximize2 className="h-3.5 w-3.5" />
                  )}
                </button>
                {/* Close button */}
                <button
                  type="button"
                  onClick={onToggle}
                  className="flex h-6 w-6 items-center justify-center rounded text-neutral-400 transition-colors hover:bg-neutral-700 hover:text-white"
                  title="Close mini-player"
                >
                  <X className="h-4 w-4" />
                </button>
              </div>
            </div>

            {/* VNC iframe container */}
            <div className="relative flex-1 bg-black" style={{ height: size.height }}>
              <VNCIframe src={vncUrl} width={propWidth} height={propHeight} />

              {/* Resize handles - edges first, then corners on top */}
              {/* Right edge */}
              <div
                onPointerDown={(e) => handleResizeStart(e, 'e')}
                className="absolute top-2 right-0 bottom-2 w-2 cursor-e-resize opacity-0 transition-opacity hover:bg-white/20 hover:opacity-100"
              />
              {/* Bottom edge */}
              <div
                onPointerDown={(e) => handleResizeStart(e, 's')}
                className="absolute right-2 bottom-0 left-2 h-2 cursor-s-resize opacity-0 transition-opacity hover:bg-white/20 hover:opacity-100"
              />
              {/* Left edge */}
              <div
                onPointerDown={(e) => handleResizeStart(e, 'w')}
                className="absolute top-2 bottom-2 left-0 w-2 cursor-w-resize opacity-0 transition-opacity hover:bg-white/20 hover:opacity-100"
              />
              {/* Top edge */}
              <div
                onPointerDown={(e) => handleResizeStart(e, 'n')}
                className="absolute top-0 right-2 left-2 h-2 cursor-n-resize opacity-0 transition-opacity hover:bg-white/20 hover:opacity-100"
              />

              {/* Corner handles - higher z-index to be on top of edges */}
              {/* Bottom-right corner - visible resize handle */}
              <div
                onPointerDown={(e) => handleResizeStart(e, 'se')}
                className="absolute right-0 bottom-0 z-10 flex h-5 w-5 cursor-se-resize items-end justify-end p-0.5"
              >
                {/* Resize grip lines */}
                <svg
                  width="10"
                  height="10"
                  viewBox="0 0 10 10"
                  className="text-neutral-500 transition-colors hover:text-neutral-300"
                >
                  <line
                    x1="9"
                    y1="1"
                    x2="1"
                    y2="9"
                    stroke="currentColor"
                    strokeWidth="1.5"
                    strokeLinecap="round"
                  />
                  <line
                    x1="9"
                    y1="5"
                    x2="5"
                    y2="9"
                    stroke="currentColor"
                    strokeWidth="1.5"
                    strokeLinecap="round"
                  />
                </svg>
              </div>
              {/* Top-right corner */}
              <div
                onPointerDown={(e) => handleResizeStart(e, 'ne')}
                className="absolute top-0 right-0 z-10 h-4 w-4 cursor-ne-resize opacity-0 transition-opacity hover:bg-white/30 hover:opacity-100"
              />
              {/* Bottom-left corner */}
              <div
                onPointerDown={(e) => handleResizeStart(e, 'sw')}
                className="absolute bottom-0 left-0 z-10 h-4 w-4 cursor-sw-resize opacity-0 transition-opacity hover:bg-white/30 hover:opacity-100"
              />
              {/* Top-left corner */}
              <div
                onPointerDown={(e) => handleResizeStart(e, 'nw')}
                className="absolute top-0 left-0 z-10 h-4 w-4 cursor-nw-resize opacity-0 transition-opacity hover:bg-white/30 hover:opacity-100"
              />
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </>
  );
}

export type { VNCOverlayPanelProps };

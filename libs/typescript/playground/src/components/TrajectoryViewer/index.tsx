import {
  Bot,
  Camera,
  Keyboard,
  MousePointer,
  MousePointerClick,
  Pause,
  Play,
  Terminal,
} from 'lucide-react';
import type React from 'react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useDarkMode } from '../../hooks/useDarkMode';

interface FileEntry {
  name: string;
  path: string;
  type: string; // 'screenshot' | 'json' | 'other'
  parentFolder: string;
}

interface Action {
  type: string;
  text: string;
  keys: string[];
  start_x: number;
  start_y: number;
  end_x: number;
  end_y: number;
  x: number;
  y: number;
}

interface TurnFolder {
  name: string;
  files: FileEntry[];
  screenshot?: string; // URL to the first screenshot
  frameIndex?: number; // Frame index in video for lazy loading
  agentResponse?: {
    text: string[];
    actions: Action[];
    cursorPositions: [number, number][];
    thought: string | null;
    model?: string; // Model name that generated the response
  };
}

interface TrajectoryViewerProps {
  files?: File[];
  zipUrl?: string;
  mp4Url?: string;
  jsonUrl?: string;
  showToolbar?: boolean;
  scroll?: boolean;
  scenarioName?: string;
  operatingSystem?: 'linux' | 'macos' | 'windows';
  autoLoop?: boolean;
  userPrompt?: string;
  isActive?: boolean;
  playbackDelay?: number;
  modelOverride?: string;
}

const TrajectoryViewer: React.FC<TrajectoryViewerProps> = ({
  files,
  zipUrl,
  mp4Url,
  jsonUrl,
  showToolbar = true,
  scroll = true,
  scenarioName = 'Demo',
  operatingSystem = 'linux',
  autoLoop = false,
  userPrompt,
  isActive = true,
  playbackDelay = 0,
  modelOverride,
}) => {
  const isDarkMode = useDarkMode();
  const [folders, setFolders] = useState<TurnFolder[]>([]);
  const [selectedFolder, setSelectedFolder] = useState<string | null>(null);

  const [isPlaying, setIsPlaying] = useState<boolean>(false);
  const [hasStartedPlaying, setHasStartedPlaying] = useState<boolean>(false);
  const folderRefs = useRef<{ [key: string]: HTMLButtonElement | null }>({});
  // Original dimensions for the screenshots
  const [originalDimensions, setOriginalDimensions] = useState({
    width: 1920,
    height: 1080,
  });
  // Add state for cursor animation
  const [cursorPosition, setCursorPosition] = useState<[number, number]>([0, 0]);
  const [cursorTarget, setCursorTarget] = useState<[number, number]>([0, 0]);
  const [isAnimating, setIsAnimating] = useState<boolean>(false);
  const [isClicking, setIsClicking] = useState<boolean>(false);
  const [cursorScale, setCursorScale] = useState<number>(1);
  const [isDragging, setIsDragging] = useState<boolean>(false);
  const [dragStart, setDragStart] = useState<[number, number]>([0, 0]);
  const [dragEnd, setDragEnd] = useState<[number, number]>([0, 0]);
  const [dragProgress, setDragProgress] = useState<number>(0);
  // Initialize isLoading to true if we have URLs to load, false otherwise
  const [isLoading, setIsLoading] = useState<boolean>(
    !!(mp4Url && jsonUrl) || !!zipUrl || !!(files && files.length > 0)
  );

  const animationRef = useRef<number | null>(null);
  const clickAnimationRef = useRef<number | null>(null);
  const dragAnimationRef = useRef<number | null>(null);

  // Control bar dragging state - x is percentage from left, bottom is percentage from bottom
  const [controlBarPosition, setControlBarPosition] = useState<{
    x: number;
    bottom: number;
  }>(() => {
    // 20% from bottom on mobile, 10% on desktop
    const isMobileInitial = typeof window !== 'undefined' && window.innerWidth < 1024;
    return { x: 50, bottom: isMobileInitial ? 20 : 10 };
  });
  const [isControlBarDragging, setIsControlBarDragging] = useState<boolean>(false);
  const [controlBarDragOffset, setControlBarDragOffset] = useState<{
    x: number;
    y: number;
  }>({ x: 0, y: 0 });
  const controlBarRef = useRef<HTMLDivElement>(null);

  // Ref to track processed MP4/JSON URLs to prevent infinite loops
  const processedUrlsRef = useRef<string>('');

  // Refs for lazy frame loading from MP4
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const videoUrlRef = useRef<string | null>(null);
  const extractingFrameRef = useRef<boolean>(false);

  // Handle control bar dragging (mouse and touch)
  useEffect(() => {
    const updatePosition = (clientX: number, clientY: number) => {
      if (!controlBarRef.current) return;

      const videoContainer = controlBarRef.current.parentElement?.parentElement;
      if (!videoContainer) return;

      const rect = videoContainer.getBoundingClientRect();
      const barRect = controlBarRef.current.getBoundingClientRect();

      // Calculate bar size as percentage
      const barWidthPercent = (barRect.width / rect.width) * 100;
      const barHeightPercent = (barRect.height / rect.height) * 100;

      // Calculate new position as percentage
      // Add half bar size because position represents center (due to transform: translate(-50%, 50%))
      const newX =
        ((clientX - rect.left - controlBarDragOffset.x) / rect.width) * 100 + barWidthPercent / 2;
      // Calculate bottom position (distance from bottom edge)
      const newBottom =
        ((rect.bottom - clientY - controlBarDragOffset.y) / rect.height) * 100 +
        barHeightPercent / 2;

      // Constrain within bounds
      const constrainedX = Math.max(barWidthPercent / 2, Math.min(100 - barWidthPercent / 2, newX));
      const constrainedBottom = Math.max(
        barHeightPercent / 2,
        Math.min(100 - barHeightPercent / 2, newBottom)
      );

      setControlBarPosition({ x: constrainedX, bottom: constrainedBottom });
    };

    const handleMouseMove = (e: MouseEvent) => {
      if (!isControlBarDragging) return;
      updatePosition(e.clientX, e.clientY);
    };

    const handleTouchMove = (e: TouchEvent) => {
      if (!isControlBarDragging || e.touches.length === 0) return;
      const touch = e.touches[0];
      updatePosition(touch.clientX, touch.clientY);
      e.preventDefault();
    };

    const handleEnd = () => {
      setIsControlBarDragging(false);
    };

    if (isControlBarDragging) {
      document.addEventListener('mousemove', handleMouseMove);
      document.addEventListener('mouseup', handleEnd);
      document.addEventListener('touchmove', handleTouchMove, {
        passive: false,
      });
      document.addEventListener('touchend', handleEnd);
      document.addEventListener('touchcancel', handleEnd);
    }

    return () => {
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleEnd);
      document.removeEventListener('touchmove', handleTouchMove);
      document.removeEventListener('touchend', handleEnd);
      document.removeEventListener('touchcancel', handleEnd);
    };
  }, [isControlBarDragging, controlBarDragOffset]);

  const handleControlBarMouseDown = (e: React.MouseEvent<HTMLDivElement>) => {
    if (!controlBarRef.current) return;

    const barRect = controlBarRef.current.getBoundingClientRect();

    // Calculate offset - x from left, y from bottom of the bar
    setControlBarDragOffset({
      x: e.clientX - barRect.left,
      y: barRect.bottom - e.clientY,
    });

    setIsControlBarDragging(true);
    e.preventDefault();
  };

  const handleControlBarTouchStart = (e: React.TouchEvent<HTMLDivElement>) => {
    if (!controlBarRef.current || e.touches.length === 0) return;

    const touch = e.touches[0];
    const barRect = controlBarRef.current.getBoundingClientRect();

    // Calculate offset - x from left, y from bottom of the bar
    setControlBarDragOffset({
      x: touch.clientX - barRect.left,
      y: barRect.bottom - touch.clientY,
    });

    setIsControlBarDragging(true);
  };

  const parseAgentResponse = useCallback(
    async (
      file: File
    ): Promise<{
      text: string[];
      actions: Action[];
      cursorPositions: [number, number][];
      thought: string | null;
      model?: string; // Model name that generated the response
    }> => {
      const result = {
        text: [] as string[],
        actions: [] as Action[],
        cursorPositions: [] as [number, number][],
        thought: null as string | null,
        model: undefined as string | undefined,
      };

      try {
        const content = await file.text();
        const data = JSON.parse(content);
        const responseData = data.response || {};

        // Extract model information if available
        if (data.model) {
          result.model = data.model;
        }

        // First check for content field (simple text response)
        if (responseData.content) {
          result.text.push(responseData.content);
        }

        // Process outputs array if present
        const outputs = responseData.output || [];
        for (const output of outputs) {
          const outputType = output.type || 'message';

          if (outputType === 'message') {
            const contentParts = output.content || [];
            if (Array.isArray(contentParts)) {
              for (const contentPart of contentParts) {
                if (contentPart.text) {
                  result.text.push(contentPart.text);
                }
              }
            } else {
              result.text.push(contentParts);
            }
          } else if (outputType === 'reasoning') {
            // Handle reasoning (thought) content
            const summaryContent = output.summary || [];
            if (summaryContent.length > 0) {
              for (const summaryPart of summaryContent) {
                if (summaryPart.type === 'summary_text') {
                  result.text.push(summaryPart.text);
                }
              }
            } else {
              const summaryText = output.text || '';
              if (summaryText) {
                result.text.push(summaryText);
              }
            }
          } else if (outputType === 'computer_call') {
            const action = output.action || {};
            if (Object.keys(action).length > 0) {
              result.actions.push(action);
              // Extract cursor position if available
              if (
                action.type === 'drag' &&
                action.start_x !== undefined &&
                action.start_y !== undefined &&
                action.end_x !== undefined &&
                action.end_y !== undefined
              ) {
                // For drag actions, store both start and end positions
                const startPosition: [number, number] = [action.start_x, action.start_y];
                const endPosition: [number, number] = [action.end_x, action.end_y];
                result.cursorPositions.push(startPosition, endPosition);
              } else if (action.x !== undefined && action.y !== undefined) {
                const position: [number, number] = [action.x, action.y];
                result.cursorPositions.push(position);
              }

              // // Determine action type
              // const actionType = action.type || '';
              // if (['click', 'double_click', 'triple_click', 'left_mouse_down'].includes(actionType)) {
              //   result.actionType = 'clicking';
              // } else if (['type', 'keypress'].includes(actionType)) {
              //   result.actionType = 'typing';
              // } else if (actionType === 'drag') {
              //   result.actionType = 'dragging';
              // }
            }
          }
        }

        // Set thought from text if available
        if (result.text.length > 0) {
          result.thought = result.text.join(' ');
        }

        // Set cursor position if not found
        // if (result.cursorPositions.length === 0 && lastKnownCursorPosition) {
        //   result.cursorPositions = [lastKnownCursorPosition];
        // }
      } catch (error) {
        console.error('[Trajectory Viewer] Error parsing agent response:', error);
      }

      return result;
    },
    []
  );

  const getCursorPosition = useCallback(
    (selectedFolder: string | null): [number, number] | null => {
      if (!selectedFolder || folders.length === 0) return [50, 50]; // Default to middle

      // Find the index of the current folder
      const currentIndex = folders.findIndex((folder) => folder.name === selectedFolder);
      if (currentIndex === -1) return [50, 50]; // Default if folder not found

      // Check current folder first
      const currentFolder = folders[currentIndex];
      if ((currentFolder.agentResponse?.cursorPositions?.length ?? 0) > 0) {
        return currentFolder.agentResponse!.cursorPositions[0];
      }

      // Search backwards through previous folders
      for (let i = currentIndex - 1; i >= 0; i--) {
        const prevFolder = folders[i];
        if ((prevFolder.agentResponse?.cursorPositions?.length ?? 0) > 0) {
          // Get the last position from the array
          return prevFolder.agentResponse!.cursorPositions[
            prevFolder.agentResponse!.cursorPositions.length - 1
          ];
        }
      }

      // No position found, default to middle
      return [originalDimensions.width / 2, originalDimensions.height / 2];
    },
    [folders, originalDimensions.width, originalDimensions.height]
  );

  const processFiles = useCallback(
    (files: File[]) => {
      if (!files || files.length === 0) return;

      setIsLoading(true);
      const turnFolders: { [key: string]: TurnFolder } = {};

      // First identify the trajectory folder structure
      let trajectoryFolderName = '';

      // Find the first turn_001 folder to identify the trajectory folder
      for (const file of files) {
        const path = file.webkitRelativePath || file.name;
        const pathParts = path.split('/');

        // Looking for paths like: trajectory_folder/turn_001/...
        if (pathParts.length >= 2 && pathParts[1].startsWith('turn_')) {
          trajectoryFolderName = pathParts[0];
          break;
        }
      }

      if (!trajectoryFolderName) {
        console.error('Could not find a valid trajectory folder with turn_XXX subfolders');
        setIsLoading(false);
        return;
      }

      const updateFolders = () => {
        // Sort folders by name (assuming they follow turn_001, turn_002 pattern)
        // using regex to extract the number
        const sortedFolders = Object.values(turnFolders).sort(
          (a, b) =>
            Number.parseInt(a.name.match(/\d+/)![0]) - Number.parseInt(b.name.match(/\d+/)![0])
        );
        setFolders(sortedFolders);
        if (sortedFolders.length > 0) {
          setSelectedFolder(sortedFolders[0].name);
          // Initialize cursor position with the first folder's cursor position
          const initialPosition = getCursorPosition(sortedFolders[0].name);
          if (initialPosition) {
            setCursorPosition(initialPosition);
            setCursorTarget(initialPosition);
          }
        }

        // Set loading to false once processing is complete
        setIsLoading(false);
      };

      // Now process all files within the turn folders
      for (const file of files) {
        const path = file.webkitRelativePath || file.name;
        const pathParts = path.split('/');

        // We're looking for files inside turn folders (depth 2 or more)
        if (
          pathParts.length < 3 ||
          pathParts[0] !== trajectoryFolderName ||
          !pathParts[1].startsWith('turn_')
        ) {
          continue; // Skip files not in a proper turn folder
        }

        const turnFolder = pathParts[1]; // This is the turn_XXX folder name
        const fileName = pathParts[pathParts.length - 1];

        // Initialize folder if it doesn't exist
        if (!turnFolders[turnFolder]) {
          turnFolders[turnFolder] = {
            name: turnFolder,
            files: [],
          };
        }

        // Determine file type
        let fileType = 'other';
        if (
          (fileName.startsWith('screenshot_') && fileName.endsWith('.png')) ||
          fileName.endsWith('screenshot.png') ||
          fileName.endsWith('screenshot_action.png') ||
          fileName.endsWith('screenshot_after.png') ||
          fileName.endsWith('screenshot_before.png')
        ) {
          fileType = 'screenshot';

          // Create URL for the first screenshot found
          if (!turnFolders[turnFolder].screenshot) {
            turnFolders[turnFolder].screenshot = URL.createObjectURL(file);
          }
        } else if (fileName.endsWith('_agent_response.json')) {
          fileType = 'json';

          // Parse the JSON file
          parseAgentResponse(file).then((parsedData) => {
            turnFolders[turnFolder].agentResponse = parsedData;
            updateFolders();
          });
        }

        // Add file to the folder
        turnFolders[turnFolder].files.push({
          name: fileName,
          path,
          type: fileType,
          parentFolder: turnFolder,
        });
      }

      updateFolders();
      setIsLoading(false);
    },
    [parseAgentResponse, getCursorPosition]
  );

  // Function to animate the click action
  const animateClick = useCallback((onComplete: () => void) => {
    setIsClicking(true);

    // Start time for animation
    const startTime = performance.now();
    const clickDuration = 300; // 300ms for the entire click animation
    const minScale = 0.5; // Minimum scale during click

    const animateClickStep = (timestamp: number) => {
      const elapsed = timestamp - startTime;
      const progress = Math.min(elapsed / clickDuration, 1);

      // Scale down quickly, then back up with spring effect
      let newScale: number;

      if (progress < 0.3) {
        // Scale down phase (first 30% of animation)
        newScale = 1 - (1 - minScale) * (progress / 0.3);
      } else {
        // Scale up phase with spring effect
        // Use a sine wave for spring effect
        const springProgress = (progress - 0.3) / minScale; // Normalized 0-1 for the spring phase
        const springEffect =
          Math.sin(springProgress * Math.PI * 2) * 0.1 * (1 - springProgress) ** 2;
        newScale = minScale + (1 - minScale) * springProgress + springEffect;
      }

      setCursorScale(newScale);

      if (progress < 1) {
        clickAnimationRef.current = requestAnimationFrame(animateClickStep);
      } else {
        // Animation complete
        setCursorScale(1);
        setIsClicking(false);
        onComplete();
      }

      // When cursor reaches minimum scale (around progress = 0.3), reveal the next frame
      if (progress >= 0.3 && progress <= 0.32) {
        onComplete();
      }
    };

    clickAnimationRef.current = requestAnimationFrame(animateClickStep);

    return () => {
      if (clickAnimationRef.current) {
        cancelAnimationFrame(clickAnimationRef.current);
      }
    };
  }, []);

  const animateDrag = useCallback(
    (start: [number, number], end: [number, number], onComplete: () => void) => {
      // Start time for animation
      const startTime = performance.now();
      const dragDuration = 500; // 500ms for the entire drag animation

      const animateDragStep = (timestamp: number) => {
        const elapsed = timestamp - startTime;
        const progress = Math.min(elapsed / dragDuration, 1);

        // Update drag progress for line rendering
        setDragProgress(progress);

        // Calculate new cursor position along the drag path
        const newX = start[0] + (end[0] - start[0]) * progress;
        const newY = start[1] + (end[1] - start[1]) * progress;

        // Update cursor position
        setCursorPosition([newX, newY]);

        if (progress < 1) {
          dragAnimationRef.current = requestAnimationFrame(animateDragStep);
        } else {
          // Animation complete
          setDragProgress(1); // Ensure it's at 100%
          onComplete();
        }
      };

      dragAnimationRef.current = requestAnimationFrame(animateDragStep);

      return () => {
        if (dragAnimationRef.current) {
          cancelAnimationFrame(dragAnimationRef.current);
        }
      };
    },
    []
  );

  // Ref for the sidebar scroll container
  const sidebarRef = useRef<HTMLDivElement>(null);

  // Helper function to scroll a folder button into view within the sidebar only
  // This prevents affecting the main page scroll
  const scrollFolderIntoView = useCallback((folderName: string) => {
    const folderButton = folderRefs.current[folderName];
    const sidebar = sidebarRef.current;

    if (!folderButton || !sidebar) return;

    const buttonRect = folderButton.getBoundingClientRect();
    const sidebarRect = sidebar.getBoundingClientRect();

    // Check if button is outside the visible area of the sidebar
    if (buttonRect.top < sidebarRect.top) {
      // Button is above visible area - scroll up
      sidebar.scrollTop += buttonRect.top - sidebarRect.top - 10;
    } else if (buttonRect.bottom > sidebarRect.bottom) {
      // Button is below visible area - scroll down
      sidebar.scrollTop += buttonRect.bottom - sidebarRect.bottom + 10;
    }
  }, []);

  const goToNextFolder = useCallback(() => {
    if (!selectedFolder || folders.length === 0) return;

    // Find the index of the current folder
    const currentIndex = folders.findIndex((folder) => folder.name === selectedFolder);
    if (currentIndex === -1) return;

    // If at the last folder and autoLoop is enabled, go back to the first folder
    if (currentIndex >= folders.length - 1) {
      if (autoLoop) {
        setSelectedFolder(folders[0].name);
      }
      return;
    }

    // Get the next folder
    const nextFolder = folders[currentIndex + 1].name;

    // Check if we have cursor positions to animate to
    const nextFolderData = folders[currentIndex + 1];
    const hasClickAction = nextFolderData.agentResponse?.actions?.some(
      (action) => action.type === 'click' || action.type === 'double_click'
    );
    const hasDragAction = nextFolderData.agentResponse?.actions?.some(
      (action) => action.type === 'drag'
    );

    if ((nextFolderData.agentResponse?.cursorPositions?.length ?? 0) > 0) {
      // Set animation target
      const target = nextFolderData.agentResponse!.cursorPositions[0];
      setCursorTarget(target);
      setIsAnimating(true);

      // Check if this is a drag action
      if (hasDragAction) {
        // Find the drag action
        const dragAction = nextFolderData.agentResponse?.actions?.find(
          (action) => action.type === 'drag'
        );
        if (dragAction) {
          // Set up drag animation
          const dragStartPos: [number, number] = [dragAction.start_x, dragAction.start_y];
          const dragEndPos: [number, number] = [dragAction.end_x, dragAction.end_y];
          // After cursor reaches start position, begin drag animation
          setTimeout(() => {
            // Start drag animation
            setDragStart(dragStartPos);
            setDragEnd(dragEndPos);
            setIsDragging(true);
            setDragProgress(0);

            // Force cursor position to start position
            setCursorPosition(dragStartPos);
            setIsAnimating(false);

            animateDrag(dragStartPos, dragEndPos, () => {
              // After drag animation completes
              setIsDragging(false);
              setIsAnimating(false);
              setSelectedFolder(nextFolder);

              // Scroll the next folder into view within the sidebar only
              if (scroll) {
                scrollFolderIntoView(nextFolder);
              }
            });
          }, 500); // Give time for cursor to move to start position
        }
      }
      // If this is a click action, start with cursor movement, then do click animation
      else if (hasClickAction) {
        // First animate cursor to position
        // After cursor movement completes, do click animation
        setTimeout(() => {
          animateClick(() => {
            // After click animation completes, reveal next frame
            setSelectedFolder(nextFolder);
            setIsAnimating(false);

            // Scroll the next folder into view within the sidebar only
            if (scroll) {
              scrollFolderIntoView(nextFolder);
            }
          });
        }, 500); // Give time for cursor to move to position
      } else {
        // After animation completes (transition is 500ms), update selected folder
        setTimeout(() => {
          setSelectedFolder(nextFolder);
          setIsAnimating(false);

          // Scroll the next folder into view within the sidebar only
          if (scroll) {
            scrollFolderIntoView(nextFolder);
          }
        }, 500);
      }
    } else {
      // If we don't have positions, just change folders immediately
      setSelectedFolder(nextFolder);

      // Scroll the next folder into view within the sidebar only
      if (scroll) {
        scrollFolderIntoView(nextFolder);
      }
    }
  }, [folders, selectedFolder, scroll, animateClick, animateDrag, scrollFolderIntoView, autoLoop]);

  const togglePlayback = () => {
    setIsPlaying(!isPlaying);
  };

  // Auto-advance when playing
  useEffect(() => {
    let intervalId: ReturnType<typeof setInterval> | null = null;

    if (isPlaying && !isAnimating) {
      intervalId = setInterval(() => {
        goToNextFolder();
      }, 1000); // Increased delay to allow for animation
    }

    return () => {
      if (intervalId) clearInterval(intervalId);
    };
  }, [isPlaying, goToNextFolder, isAnimating]);

  // Handle playback based on isActive state and playbackDelay
  useEffect(() => {
    let timeoutId: ReturnType<typeof setTimeout> | null = null;

    if (isActive && folders.length > 0) {
      // Start playback after delay when becoming active
      if (!hasStartedPlaying) {
        timeoutId = setTimeout(() => {
          setIsPlaying(true);
          setHasStartedPlaying(true);
        }, playbackDelay);
      } else {
        // Resume playback immediately if already started before
        setIsPlaying(true);
      }
    } else if (!isActive) {
      // Pause when becoming inactive
      setIsPlaying(false);
    }

    return () => {
      if (timeoutId) clearTimeout(timeoutId);
    };
  }, [isActive, folders.length, playbackDelay, hasStartedPlaying]);

  const selectedFolderData = folders.find((folder) => folder.name === selectedFolder);

  // Get the first valid model name from any folder
  const getFirstValidModel = () => {
    for (const folder of folders) {
      if (folder.agentResponse?.model) {
        return folder.agentResponse.model;
      }
    }
    return null;
  };

  // Get the model once when folders are loaded
  const modelName = modelOverride || getFirstValidModel();

  // Update original dimensions when the image loads
  useEffect(() => {
    if (selectedFolderData?.screenshot) {
      const img = new Image();
      img.onload = () => {
        setOriginalDimensions({
          width: img.naturalWidth,
          height: img.naturalHeight,
        });
      };
      img.src = selectedFolderData.screenshot;
    }
  }, [selectedFolderData?.screenshot]);

  // Effect to animate cursor movement
  useEffect(() => {
    // Only animate if not at target already
    if (
      isAnimating &&
      (cursorPosition[0] !== cursorTarget[0] || cursorPosition[1] !== cursorTarget[1])
    ) {
      // Calculate step sizes for smooth animation
      const startTime = performance.now();
      const duration = 100; // 100ms animation for movement

      const animateStep = (timestamp: number) => {
        const elapsed = timestamp - startTime;
        const progress = Math.min(elapsed / duration, 1);

        // Use easing function for smoother animation (ease-out-cubic)
        const easeProgress = 1 - (1 - progress) ** 3;

        // Calculate new position
        const newX = cursorPosition[0] + (cursorTarget[0] - cursorPosition[0]) * easeProgress;
        const newY = cursorPosition[1] + (cursorTarget[1] - cursorPosition[1]) * easeProgress;

        setCursorPosition([newX, newY]);

        // Continue animation if not complete
        if (progress < 1) {
          animationRef.current = requestAnimationFrame(animateStep);
        }
      };

      animationRef.current = requestAnimationFrame(animateStep);

      // Cleanup function
      return () => {
        if (animationRef.current !== null) {
          cancelAnimationFrame(animationRef.current);
        }
      };
    }
  }, [isAnimating, cursorPosition, cursorTarget]);

  // Get cursor position for the selected folder, checking previous folders if needed
  // Get previous screenshot for the selected folder if current folder has none
  const getPreviousScreenshot = (selectedFolder: string | null): string | null => {
    if (!selectedFolder || folders.length === 0) return null;

    // Find the index of the current folder
    const currentIndex = folders.findIndex((folder) => folder.name === selectedFolder);
    if (currentIndex === -1) return null;

    // Search backwards through previous folders
    for (let i = currentIndex - 1; i >= 0; i--) {
      const prevFolder = folders[i];
      if (prevFolder.screenshot) {
        return prevFolder.screenshot;
      }
    }

    // No previous screenshot found
    return null;
  };

  // Function to animate drag action
  // Generate skeleton turns for loading state
  const generateSkeletonTurns = (count: number) => {
    const skeletons = [];
    for (let i = folders.length + 1; i <= count; i++) {
      skeletons.push(
        <div key={`skeleton-${i}`} className="flex animate-pulse flex-col gap-1 py-2 opacity-50">
          {/* Turn number skeleton */}
          <div className="px-2">
            <div className="h-4 w-16 rounded bg-neutral-300 dark:bg-neutral-600" />
          </div>

          {/* Thought skeleton */}
          <div className="px-2">
            <div className="mb-1 h-4 w-full rounded bg-neutral-300 dark:bg-neutral-600" />
            <div className="h-4 w-3/4 rounded bg-neutral-300 dark:bg-neutral-600" />
          </div>

          {/* Action skeleton */}
          <div className="rounded border border-neutral-200 px-2 py-1 dark:border-neutral-700">
            <div className="flex items-center gap-2">
              <div className="h-5 w-5 rounded bg-neutral-300 dark:bg-neutral-600" />
              <div className="h-4 w-32 rounded bg-neutral-300 dark:bg-neutral-600" />
            </div>
          </div>
        </div>
      );
    }
    return skeletons;
  };

  // Clean up animations and video on unmount
  useEffect(() => {
    return () => {
      if (animationRef.current !== null) {
        cancelAnimationFrame(animationRef.current);
      }
      if (clickAnimationRef.current !== null) {
        cancelAnimationFrame(clickAnimationRef.current);
      }
      if (dragAnimationRef.current !== null) {
        cancelAnimationFrame(dragAnimationRef.current);
      }
      // Clean up video element and object URL
      if (videoUrlRef.current) {
        URL.revokeObjectURL(videoUrlRef.current);
      }
      videoRef.current = null;
      videoUrlRef.current = null;
    };
  }, []);

  // Function to fetch and process zip file from URL
  const processZipUrl = useCallback(
    async (url: string) => {
      try {
        setIsLoading(true);

        // Import zip.js dynamically to avoid issues with SSR
        const { BlobReader, ZipReader, TextWriter, BlobWriter } = await import('@zip.js/zip.js');

        // Fetch the zip file
        const response = await fetch(url);
        if (!response.ok) {
          throw new Error(`Failed to fetch zip: ${response.status} ${response.statusText}`);
        }

        const zipBlob = await response.blob();

        // Create a zip reader
        const zipReader = new ZipReader(new BlobReader(zipBlob));

        // Get all entries from the zip file
        const entries = await zipReader.getEntries();

        if (entries.length === 0) {
          setIsLoading(false);
          throw new Error('Zip file is empty');
        }

        // Convert zip entries to File objects for processing
        const extractedFiles: File[] = [];

        // Find the base directory name (trajectory folder)
        let trajectoryFolderName = '';

        // Look for the first turn_001 folder to identify the trajectory folder
        for (const entry of entries) {
          const pathParts = entry.filename.split('/');
          if (pathParts.length >= 2 && pathParts[1].startsWith('turn_')) {
            trajectoryFolderName = pathParts[0];
            break;
          }
        }

        if (!trajectoryFolderName) {
          console.error(
            '[Trajectory Viewer] Could not find a valid trajectory folder with turn_XXX subfolders'
          );
          return;
        }

        // Process each entry
        for (const entry of entries) {
          if (!entry.directory) {
            try {
              // For text files (like JSON)
              let fileData: Blob;
              if (entry.filename.endsWith('.json')) {
                const text = (await entry?.getData?.(new TextWriter())) || '';
                fileData = new Blob([text], { type: 'application/json' });
              } else {
                // For binary files (like images)
                const blobData = await entry?.getData?.(new BlobWriter());
                fileData = blobData || new Blob([]);
              }

              // Create a File object with the correct path information
              const file = new File([fileData], entry.filename.split('/').pop() || '', {
                type: entry.filename.endsWith('.json')
                  ? 'application/json'
                  : entry.filename.endsWith('.png')
                    ? 'image/png'
                    : entry.filename.endsWith('.jpg')
                      ? 'image/jpeg'
                      : 'application/octet-stream',
              });

              // Add webkitRelativePath to mimic file input with webkitdirectory
              Object.defineProperty(file, 'webkitRelativePath', {
                value: entry.filename,
                writable: false,
              });

              extractedFiles.push(file);
            } catch (err) {
              console.error(`[Trajectory Viewer] Error extracting ${entry.filename}:`, err);
            }
          }
        }

        // Close the zip reader
        await zipReader.close();

        // Process the extracted files
        if (extractedFiles.length > 0) {
          processFiles(extractedFiles);
        }
      } catch (error) {
        console.error('[Trajectory Viewer] Error processing zip URL:', error);
        setIsLoading(false);
      }
    },
    [processFiles]
  );

  // Extract a single frame from the video at the given frame index
  const extractFrame = useCallback(async (frameIndex: number): Promise<string | null> => {
    const video = videoRef.current;
    if (!video || extractingFrameRef.current) return null;

    extractingFrameRef.current = true;

    try {
      // Seek to the frame time
      video.currentTime = frameIndex;

      await new Promise<void>((resolve) => {
        video.onseeked = () => resolve();
      });

      // Draw frame to canvas
      const canvas = document.createElement('canvas');
      canvas.width = video.videoWidth;
      canvas.height = video.videoHeight;
      const ctx = canvas.getContext('2d');

      if (ctx) {
        ctx.drawImage(video, 0, 0);

        // Convert to blob and create URL
        const frameBlob = await new Promise<Blob>((resolve) => {
          canvas.toBlob((blob) => {
            resolve(blob!);
          }, 'image/png');
        });

        return URL.createObjectURL(frameBlob);
      }
    } catch (error) {
      console.error('[Trajectory Viewer] Error extracting frame:', error);
    } finally {
      extractingFrameRef.current = false;
    }

    return null;
  }, []);

  // Lazy load frame for a specific folder
  const lazyLoadFrame = useCallback(
    async (folderName: string) => {
      const folderIndex = folders.findIndex((f) => f.name === folderName);
      if (folderIndex === -1) return;

      const folder = folders[folderIndex];
      // Already has screenshot or no frame index
      if (folder.screenshot || folder.frameIndex === undefined) return;

      const screenshotUrl = await extractFrame(folder.frameIndex);
      if (screenshotUrl) {
        setFolders((prevFolders) => {
          const newFolders = [...prevFolders];
          const idx = newFolders.findIndex((f) => f.name === folderName);
          if (idx !== -1 && !newFolders[idx].screenshot) {
            newFolders[idx] = { ...newFolders[idx], screenshot: screenshotUrl };
          }
          return newFolders;
        });
      }
    },
    [folders, extractFrame]
  );

  // Preload frames ahead of current selection
  const preloadAhead = useCallback(
    async (currentFolderName: string, count: number = 3) => {
      const currentIndex = folders.findIndex((f) => f.name === currentFolderName);
      if (currentIndex === -1) return;

      // Preload next N frames
      for (let i = 1; i <= count && currentIndex + i < folders.length; i++) {
        const nextFolder = folders[currentIndex + i];
        if (!nextFolder.screenshot && nextFolder.frameIndex !== undefined) {
          await lazyLoadFrame(nextFolder.name);
        }
      }
    },
    [folders, lazyLoadFrame]
  );

  // Process MP4 and JSON files with lazy frame loading
  const processMP4Json = useCallback(
    async (mp4Url: string, jsonUrl: string) => {
      if (processedUrlsRef.current === `${mp4Url},${jsonUrl}`) {
        return;
      }

      processedUrlsRef.current = `${mp4Url},${jsonUrl}`;

      try {
        setIsLoading(true);

        // Fetch both files in parallel
        const [mp4Response, jsonResponse] = await Promise.all([fetch(mp4Url), fetch(jsonUrl)]);

        if (!mp4Response.ok || !jsonResponse.ok) {
          throw new Error('Failed to fetch MP4 or JSON file');
        }

        const [mp4Blob, jsonText] = await Promise.all([mp4Response.blob(), jsonResponse.text()]);

        // Parse JSON file
        const jsonData = JSON.parse(jsonText);
        const { screenshots, agent_responses } = jsonData;

        const turnFolders: { [key: string]: TurnFolder } = {};

        // Create turn folders from screenshots with frame indices for lazy loading
        if (screenshots) {
          for (const screenshot of screenshots) {
            const { turn_name, frame_index } = screenshot;
            if (!turnFolders[turn_name]) {
              turnFolders[turn_name] = {
                name: turn_name,
                files: [],
                frameIndex: frame_index, // Store frame index for lazy loading
              };
            }
          }
        }

        // Process agent responses
        if (agent_responses) {
          for (const response of agent_responses) {
            const { turn_name, content } = response;

            if (!turnFolders[turn_name]) {
              turnFolders[turn_name] = {
                name: turn_name,
                files: [],
              };
            }

            // Create a File object from the content to reuse parseAgentResponse
            const agentResponseFile = new File([content], `${turn_name}_agent_response.json`, {
              type: 'application/json',
            });

            try {
              const parsedData = await parseAgentResponse(agentResponseFile);
              turnFolders[turn_name].agentResponse = parsedData;
            } catch (error) {
              console.error(
                `[Trajectory Viewer] Error parsing agent response for ${turn_name}:`,
                error
              );
            }

            turnFolders[turn_name].files.push({
              name: `${turn_name}_agent_response.json`,
              path: `trajectory/${turn_name}/${turn_name}_agent_response.json`,
              type: 'json',
              parentFolder: turn_name,
            });
          }
        }

        // Create video element and store in ref for lazy frame extraction
        const video = document.createElement('video');
        const videoObjectUrl = URL.createObjectURL(mp4Blob);
        video.src = videoObjectUrl;
        video.muted = true;
        video.preload = 'metadata';

        await new Promise((resolve, reject) => {
          video.onloadedmetadata = resolve;
          video.onerror = reject;
        });

        // Store video ref for lazy loading
        videoRef.current = video;
        videoUrlRef.current = videoObjectUrl;

        // Set original dimensions from video
        setOriginalDimensions({
          width: video.videoWidth,
          height: video.videoHeight,
        });

        // Extract only the first frame immediately
        if (screenshots && screenshots.length > 0) {
          const firstScreenshot = screenshots[0];
          const firstTurnName = firstScreenshot.turn_name;

          video.currentTime = firstScreenshot.frame_index || 0;
          await new Promise((resolve) => {
            video.onseeked = () => resolve(undefined);
          });

          const canvas = document.createElement('canvas');
          canvas.width = video.videoWidth;
          canvas.height = video.videoHeight;
          const ctx = canvas.getContext('2d');

          if (ctx) {
            ctx.drawImage(video, 0, 0);
            const frameBlob = await new Promise<Blob>((resolve) => {
              canvas.toBlob((blob) => resolve(blob!), 'image/png');
            });

            turnFolders[firstTurnName].screenshot = URL.createObjectURL(frameBlob);
          }
        }

        // Sort and set folders (filter out any entries with invalid names)
        const sortedFolders = Object.values(turnFolders)
          .filter((folder) => folder.name && typeof folder.name === 'string')
          .sort((a, b) => {
            const aMatch = a.name.match(/\d+/);
            const bMatch = b.name.match(/\d+/);
            if (!aMatch || !bMatch) return 0;
            return Number.parseInt(aMatch[0]) - Number.parseInt(bMatch[0]);
          });

        setFolders(sortedFolders);
        setSelectedFolder(sortedFolders[0]?.name || null);
        setIsLoading(false);

        // Initialize cursor position
        if (sortedFolders.length > 0) {
          const initialPosition = getCursorPosition(sortedFolders[0].name);
          if (initialPosition) {
            setCursorPosition(initialPosition);
            setCursorTarget(initialPosition);
          }
        }
      } catch (error) {
        console.error('[Trajectory Viewer] Error processing MP4 and JSON files:', error);
        setIsLoading(false);
      }
    },
    [parseAgentResponse, getCursorPosition]
  );

  // Process MP4 and JSON files when URLs are provided
  useEffect(() => {
    if (mp4Url && jsonUrl) {
      processMP4Json(mp4Url, jsonUrl);
    }
  }, [mp4Url, jsonUrl, processMP4Json]);

  // Lazy load frame when selected folder changes
  useEffect(() => {
    if (selectedFolder && videoRef.current) {
      // Load the current frame if not already loaded
      lazyLoadFrame(selectedFolder);
      // Preload next few frames
      preloadAhead(selectedFolder, 3);
    }
  }, [selectedFolder, lazyLoadFrame, preloadAhead]);

  // Process files when they're provided as props
  // biome-ignore lint/correctness/useExhaustiveDependencies: don't re-render
  useEffect(() => {
    if (files && files.length > 0) {
      processFiles(files);
    }
  }, [files]);

  // Ref to track processed ZIP URL to prevent infinite loops
  const processedZipUrlRef = useRef<string>('');

  // Process zip URL when provided
  useEffect(() => {
    if (zipUrl && processedZipUrlRef.current !== zipUrl) {
      processedZipUrlRef.current = zipUrl;
      processZipUrl(zipUrl);
    }
  }, [zipUrl, processZipUrl]);

  return (
    <div className={'flex aspect-auto h-full flex-col overflow-hidden'}>
      {/* Skeleton loading state - matches exact layout of loaded content */}
      {isLoading && folders.length === 0 && (
        <div className="flex h-full flex-col">
          {/* Skeleton toolbar placeholder to match actual toolbar height */}
          {showToolbar && (
            <div className="flex items-center gap-2 border-neutral-200 border-b px-2 py-1.5 md:px-3 md:py-2 dark:border-neutral-800">
              <div className="flex h-[22px] items-center gap-1.5 rounded-t-lg border border-neutral-300 border-b-0 px-2 py-1 md:h-[26px] md:gap-2 md:px-3 md:py-1.5 dark:border-neutral-700">
                <div className="h-3 w-3 animate-pulse rounded bg-neutral-300 md:h-3.5 md:w-3.5 dark:bg-neutral-600" />
                <div className="h-3 w-12 animate-pulse rounded bg-neutral-300 md:h-4 dark:bg-neutral-600" />
              </div>
            </div>
          )}
          <div
            className={`${showToolbar ? 'h-[calc(100%-48px)]' : 'h-full'} relative flex flex-col gap-0 lg:flex-row lg:gap-3 lg:p-3`}
          >
            {/* Sidebar skeleton - order-2 on mobile/tablet so it appears below video */}
            <div className="-mt-px order-2 flex h-80 w-full flex-col lg:order-1 lg:mt-0 lg:h-full lg:w-1/4">
              <div className="flex h-full flex-col gap-3 overflow-hidden p-0 lg:rounded-lg lg:p-3">
                {/* Skeleton turn items */}
                {[1, 2, 3, 4].map((i) => (
                  <div key={i} className="flex animate-pulse flex-col gap-2">
                    <div className="flex items-start gap-2">
                      <div className="mt-1 hidden h-6 w-6 flex-shrink-0 rounded-full bg-neutral-300 lg:block dark:bg-neutral-700" />
                      <div className="flex flex-1 flex-col gap-2">
                        <div className="h-3 w-12 rounded bg-neutral-300 dark:bg-neutral-700" />
                        <div className="h-4 w-full rounded bg-neutral-300 dark:bg-neutral-700" />
                        <div className="h-4 w-3/4 rounded bg-neutral-300 dark:bg-neutral-700" />
                        <div className="flex items-center gap-2 rounded-lg px-2 py-1.5">
                          <div className="h-4 w-4 rounded bg-neutral-300 dark:bg-neutral-700" />
                          <div className="h-3 w-24 rounded bg-neutral-300 dark:bg-neutral-700" />
                        </div>
                      </div>
                    </div>
                  </div>
                ))}
              </div>
            </div>

            {/* Main content skeleton - order-1 on mobile/tablet so it appears above messages */}
            <div className="order-1 flex h-full w-full flex-col lg:order-2 lg:w-3/4">
              <div className="relative w-full overflow-hidden lg:h-full lg:flex-grow lg:rounded-lg">
                {/* Aspect ratio container to match video dimensions */}
                <div className="aspect-video w-full animate-pulse bg-neutral-200 lg:rounded-lg dark:bg-neutral-800">
                  <div className="flex h-full w-full items-center justify-center">
                    {/* Play button circle */}
                    <div className="flex h-16 w-16 items-center justify-center rounded-full bg-neutral-300/50 dark:bg-neutral-700/50">
                      <div className="ml-1 h-0 w-0 border-y-8 border-y-transparent border-l-12 border-l-neutral-400 dark:border-l-neutral-600" />
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Trajectory viewer */}
      {!isLoading && folders.length > 0 && (
        <div className="flex h-full min-h-0 flex-col">
          {/* Browser-like tab bar */}
          {showToolbar && (
            <div className="flex items-center gap-2 border-neutral-200 border-b px-2 py-1.5 md:px-3 md:py-2 dark:border-neutral-800">
              {/* Tab for current scenario */}
              <div className="flex items-center gap-1.5 rounded-t-lg border border-neutral-300 border-b-0 px-2 py-1 md:gap-2 md:px-3 md:py-1.5 dark:border-neutral-700">
                <Bot
                  size={12}
                  className="text-neutral-600 md:h-3.5 md:w-3.5 dark:text-neutral-400"
                />
                <span className="font-medium text-neutral-900 text-xs md:text-sm dark:text-white">
                  {scenarioName}
                </span>
              </div>
            </div>
          )}

          {/* Agent content */}
          <div
            id="trajectory-container"
            className={`${showToolbar ? 'h-[calc(100%-48px)]' : 'h-full'} relative flex min-h-0 flex-col gap-0 overflow-hidden lg:flex-row lg:gap-3 lg:p-3`}
          >
            {/* Chat sidebar - order-2 on mobile/tablet so it appears below video */}
            <div className="-mt-px order-2 flex w-full flex-col overflow-hidden lg:order-1 lg:mt-0 lg:w-1/4">
              <div ref={sidebarRef} className="flex-1 overflow-y-auto p-0 lg:p-3">
                {/* User prompt display */}
                {userPrompt && (
                  <div className="mb-4 flex items-start gap-2 rounded-lg bg-blue-50 p-3 dark:bg-blue-900/20">
                    <div className="mt-0.5 flex h-6 w-6 flex-shrink-0 items-center justify-center rounded-full bg-blue-500 text-white">
                      <svg
                        xmlns="http://www.w3.org/2000/svg"
                        viewBox="0 0 24 24"
                        fill="currentColor"
                        className="h-3.5 w-3.5"
                      >
                        <path
                          fillRule="evenodd"
                          d="M7.5 6a4.5 4.5 0 119 0 4.5 4.5 0 01-9 0zM3.751 20.105a8.25 8.25 0 0116.498 0 .75.75 0 01-.437.695A18.683 18.683 0 0112 22.5c-2.786 0-5.433-.608-7.812-1.7a.75.75 0 01-.437-.695z"
                          clipRule="evenodd"
                        />
                      </svg>
                    </div>
                    <div className="flex flex-col gap-1">
                      <span className="font-medium text-blue-700 text-xs dark:text-blue-300">
                        User
                      </span>
                      <p className="text-neutral-800 text-sm dark:text-neutral-200">{userPrompt}</p>
                    </div>
                  </div>
                )}
                {folders.map((folder) => {
                  // Check if this turn only has a screenshot (no thought or actions)
                  const hasTextContent =
                    folder.agentResponse?.thought ||
                    (folder.agentResponse?.actions && folder.agentResponse.actions.length > 0);
                  const isScreenshotOnly = !hasTextContent && folder.screenshot;

                  return (
                    <button
                      className={`flex flex-col gap-2 ${isPlaying && folder.name !== selectedFolder ? 'opacity-30' : 'opacity-100'}`}
                      key={folder.name}
                      ref={(el) => {
                        folderRefs.current[folder.name] = el;
                      }}
                      onClick={() => setSelectedFolder(folder.name)}
                      type="button"
                    >
                      {/* Chat message bubble */}
                      <div className="flex flex-col gap-2">
                        {/* Agent message */}
                        <div className="flex items-start gap-2">
                          {/* Agent avatar - hidden on mobile */}
                          <div className="mt-1 hidden h-6 w-6 flex-shrink-0 items-center justify-center rounded-full text-neutral-900 lg:flex dark:text-white">
                            <Bot size={14} />
                          </div>

                          {/* Message content */}
                          <div className="flex min-w-0 flex-1 flex-col gap-2">
                            {/* Turn number label */}
                            <div className="text-neutral-500 text-xs dark:text-neutral-400">
                              Turn {folder.name.split('_')[1]}
                            </div>

                            {/* Thought bubble */}
                            {folder.agentResponse?.thought && (
                              <div className="rounded-lg rounded-tl-sm px-3 py-2 text-neutral-800 text-sm dark:text-neutral-200">
                                {folder.agentResponse.thought}
                              </div>
                            )}

                            {/* Action cards */}
                            {folder.agentResponse?.actions &&
                              folder.agentResponse.actions.length > 0 && (
                                <div className="flex flex-col gap-1">
                                  {folder.agentResponse.actions.map((action, index) => (
                                    <div
                                      key={index}
                                      className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-neutral-600 text-xs dark:text-neutral-400"
                                    >
                                      {action.type === 'click' || action.type === 'double_click' ? (
                                        <>
                                          <MousePointerClick size={14} className="flex-shrink-0" />
                                          <span className="overflow-hidden text-ellipsis">
                                            {action.type === 'double_click'
                                              ? 'Double Click'
                                              : 'Click'}{' '}
                                            ({action.x}, {action.y})
                                          </span>
                                        </>
                                      ) : action.type === 'drag' ? (
                                        <>
                                          <MousePointer size={14} className="flex-shrink-0" />
                                          <span className="overflow-hidden text-ellipsis">
                                            Drag to ({action.end_x}, {action.end_y})
                                          </span>
                                        </>
                                      ) : action.type === 'type' ? (
                                        <>
                                          <Keyboard size={14} className="flex-shrink-0" />
                                          <span className="overflow-hidden text-ellipsis">
                                            Type "{action.text}"
                                          </span>
                                        </>
                                      ) : action.type === 'hotkey' ? (
                                        <>
                                          <Keyboard size={14} className="flex-shrink-0" />
                                          <span className="overflow-hidden text-ellipsis">
                                            Press {action.keys.join(' + ')}
                                          </span>
                                        </>
                                      ) : (
                                        <>
                                          <Terminal size={14} className="flex-shrink-0" />
                                          <span className="overflow-hidden text-ellipsis">
                                            {action.type}
                                          </span>
                                        </>
                                      )}
                                    </div>
                                  ))}
                                </div>
                              )}

                            {/* Screenshot indicator - shows on mobile for screenshot-only turns */}
                            {isScreenshotOnly && (
                              <div className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-neutral-600 text-xs lg:hidden dark:text-neutral-400">
                                <Camera size={14} className="flex-shrink-0" />
                                <span className="overflow-hidden text-ellipsis">Screenshot</span>
                              </div>
                            )}

                            {/* Screenshot thumbnail - hidden on mobile */}
                            {folder.screenshot && (
                              <div className="hidden max-w-full overflow-hidden rounded-lg border border-neutral-200 lg:block dark:border-neutral-700">
                                <img
                                  src={folder.screenshot}
                                  alt="Screenshot"
                                  className="w-full max-w-full object-cover"
                                />
                              </div>
                            )}

                            {/* Empty state */}
                            {!folder.agentResponse?.thought &&
                              !folder.agentResponse?.actions?.length &&
                              !folder.screenshot && (
                                <div className="rounded-lg rounded-tl-sm px-3 py-2 text-neutral-500 text-sm italic dark:text-neutral-400">
                                  No response
                                </div>
                              )}
                          </div>
                        </div>
                      </div>
                    </button>
                  );
                })}
                {isLoading && generateSkeletonTurns(10)}
              </div>
            </div>

            {/* Screen area - order-1 on mobile/tablet so it appears above messages */}
            <div className="order-1 flex h-full w-full flex-col lg:order-2 lg:w-3/4">
              {/* Video view (top row) */}
              <div className="relative w-full overflow-hidden lg:flex-grow lg:rounded-lg">
                {selectedFolderData?.screenshot ? (
                  <div className="relative flex h-full w-full justify-center overflow-hidden">
                    <img
                      src={selectedFolderData.screenshot}
                      alt={`Screenshot from ${selectedFolderData.name}`}
                      className="h-full w-auto"
                    />
                    {/* Drag line overlay */}
                    {isDragging && (
                      <svg className="pointer-events-none absolute top-0 left-0 z-[5] h-full w-full">
                        <line
                          x1={`${(dragStart[0] / originalDimensions.width) * 100}%`}
                          y1={`${(dragStart[1] / originalDimensions.height) * 100}%`}
                          x2={`${
                            (((dragEnd[0] - dragStart[0]) * dragProgress + dragStart[0]) /
                              originalDimensions.width) *
                            100
                          }%`}
                          y2={`${
                            (((dragEnd[1] - dragStart[1]) * dragProgress + dragStart[1]) /
                              originalDimensions.height) *
                            100
                          }%`}
                          stroke="rgba(255, 255, 255, 0.8)"
                          strokeWidth="2"
                          strokeDasharray="5,3"
                          style={{
                            filter: 'drop-shadow(0px 0px 3px rgba(0, 0, 0, 0.7))',
                          }}
                        />
                      </svg>
                    )}

                    {/* Animated Cursor overlay */}
                    <div
                      className="pointer-events-none absolute"
                      style={{
                        left: `${((cursorPosition[0] || originalDimensions.width / 2) / originalDimensions.width) * 100}%`,
                        top: `${((cursorPosition[1] || originalDimensions.height / 2) / originalDimensions.height) * 100}%`,
                        transform: `translate(-25%, -5%) scale(${cursorScale})`,
                        zIndex: 10,
                      }}
                    >
                      <img
                        src="/content/cursors/handpointing.svg"
                        alt="Cursor"
                        width="32"
                        height="32"
                        style={{
                          filter: 'drop-shadow(0px 0px 5px rgba(0, 0, 0, 0.5))',
                        }}
                      />
                      {/* Click animation circle */}
                      {isClicking && (
                        <div
                          className="pointer-events-none absolute border-2 border-blue-500"
                          style={{
                            width: '40px',
                            height: '40px',
                            borderRadius: '50%',
                            animation: 'pulseCircle 1s ease-out',
                            zIndex: 5,
                          }}
                        />
                      )}
                    </div>

                    {/* Define keyframe animation for pulse */}
                    <style>{`
                          @keyframes pulseCircle {
                            0% {
                              transform: translate(-15%, -100%) scale(0.5);
                              opacity: 0.7;
                            }
                            100% {
                              transform: translate(-15%, -100%) scale(2);
                              opacity: 0;
                            }
                          }
                        `}</style>
                  </div>
                ) : (
                  // Use previous screenshot if available
                  (() => {
                    const prevScreenshot = getPreviousScreenshot(selectedFolder);
                    if (prevScreenshot) {
                      return (
                        <div className="relative flex h-full w-full justify-center overflow-hidden border border-neutral-200 opacity-70 lg:rounded-lg dark:border-neutral-700">
                          <img
                            src={prevScreenshot}
                            alt={'Previous screenshot'}
                            className="h-full w-auto"
                          />
                          {/* Drag line overlay */}
                          {isDragging && (
                            <svg className="pointer-events-none absolute top-0 left-0 z-[5] h-full w-full">
                              <line
                                x1={`${(dragStart[0] / originalDimensions.width) * 100}%`}
                                y1={`${(dragStart[1] / originalDimensions.height) * 100}%`}
                                x2={`${
                                  (((dragEnd[0] - dragStart[0]) * dragProgress + dragStart[0]) /
                                    originalDimensions.width) *
                                  100
                                }%`}
                                y2={`${
                                  (((dragEnd[1] - dragStart[1]) * dragProgress + dragStart[1]) /
                                    originalDimensions.height) *
                                  100
                                }%`}
                                stroke="rgba(255, 255, 255, 0.8)"
                                strokeWidth="2"
                                strokeDasharray="5,3"
                                style={{
                                  filter: 'drop-shadow(0px 0px 3px rgba(0, 0, 0, 0.7))',
                                }}
                              />
                            </svg>
                          )}

                          {/* Animated Cursor overlay */}
                          <div
                            className="pointer-events-none absolute"
                            style={{
                              left: `${((cursorPosition[0] || originalDimensions.width / 2) / originalDimensions.width) * 100}%`,
                              top: `${((cursorPosition[1] || originalDimensions.height / 2) / originalDimensions.height) * 100}%`,
                              transform: `translate(-50%, -25%) scale(${cursorScale})`,
                              zIndex: 10,
                              transition: isAnimating
                                ? 'none'
                                : 'left 0.05s ease-out, top 0.05s ease-out',
                            }}
                          >
                            <img
                              src="/content/cursors/handpointing.svg"
                              alt="Cursor"
                              width="32"
                              height="32"
                              style={{
                                transformOrigin: '50% 25%',
                                filter: 'drop-shadow(0px 0px 5px rgba(0, 0, 0, 0.5))',
                              }}
                            />
                          </div>
                        </div>
                      );
                    }
                    return (
                      <div className="flex h-full w-full items-center justify-center rounded bg-neutral-100 p-8 text-center text-neutral-600 dark:bg-neutral-800 dark:text-neutral-300">
                        <div className="flex flex-col items-center">
                          <div className="h-4 w-16 rounded bg-neutral-300 dark:bg-neutral-600" />
                          <div className="mt-2 h-4 w-32 rounded bg-neutral-300 dark:bg-neutral-600" />
                        </div>
                      </div>
                    );
                  })()
                )}

                {/* Frosted overlay controls - draggable on all screen sizes */}
                <div className="pointer-events-none absolute inset-0 z-20">
                  <div
                    ref={controlBarRef}
                    role="toolbar"
                    className="pointer-events-auto absolute cursor-move"
                    style={{
                      left: `${controlBarPosition.x}%`,
                      bottom: `${controlBarPosition.bottom}%`,
                      transform: 'translate(-50%, 50%)',
                    }}
                    onMouseDown={handleControlBarMouseDown}
                    onTouchStart={handleControlBarTouchStart}
                  >
                    {/* Outer frosted frame */}
                    <div className="rounded-2xl bg-neutral-200/50 p-[3px] shadow-sm dark:bg-white/10">
                      {/* Inner content */}
                      <div className="flex items-center gap-4 rounded-xl border border-neutral-200/50 bg-white/80 px-4 py-3 backdrop-blur-sm dark:border-neutral-700/50 dark:bg-neutral-900/80">
                        {/* Play/Pause icon */}
                        <button
                          onClick={togglePlayback}
                          type="button"
                          className="text-neutral-900 transition-colors hover:text-neutral-600 dark:text-white dark:hover:text-neutral-400"
                          title={isPlaying ? 'Pause' : 'Play'}
                        >
                          {isPlaying ? (
                            <Pause size={20} strokeWidth={2.5} />
                          ) : (
                            <Play size={20} strokeWidth={2.5} />
                          )}
                        </button>

                        {/* Model info */}
                        {modelName && (
                          <>
                            {/* Model icon and name */}
                            <div className="flex items-center gap-2">
                              {modelName.includes('ByteDance') || modelName.includes('UI-TARS') ? (
                                <>
                                  <img
                                    src="/img/logos/bytedance.svg"
                                    alt="ByteDance"
                                    className="h-5 w-5 flex-shrink-0"
                                  />
                                  <span className="hidden whitespace-nowrap font-medium text-neutral-900 text-sm sm:inline dark:text-white">
                                    {modelName}
                                  </span>
                                </>
                              ) : modelName.includes('computer-use-preview') ? (
                                <>
                                  <img
                                    src={
                                      isDarkMode
                                        ? '/img/logos/openai-white.svg'
                                        : '/img/logos/openai-black.svg'
                                    }
                                    alt="OpenAI"
                                    className="h-5 w-5 flex-shrink-0"
                                  />
                                  <span className="hidden whitespace-nowrap font-medium text-neutral-900 text-sm sm:inline dark:text-white">
                                    {modelName}
                                  </span>
                                </>
                              ) : modelName.includes('claude-opus') ||
                                modelName.includes('claude-opus-4.5') ||
                                modelName.includes('anthropic/claude-opus') ||
                                modelName.includes('cua/anthropic/claude-opus') ? (
                                <>
                                  <img
                                    src="/img/logos/claude.svg"
                                    alt="Anthropic"
                                    className="h-5 w-5 flex-shrink-0"
                                  />
                                  <span className="hidden whitespace-nowrap font-medium text-neutral-900 text-sm sm:inline dark:text-white">
                                    {modelName.includes('claude-opus-4.5')
                                      ? 'Claude Opus 4.5'
                                      : modelName.includes('claude-opus')
                                        ? 'Claude Opus'
                                        : modelName}
                                  </span>
                                </>
                              ) : modelName.includes('gemini') || modelName.includes('Gemini') ? (
                                <>
                                  <img
                                    src="/img/providers/google.svg"
                                    alt="Google"
                                    className="h-5 w-5 flex-shrink-0"
                                  />
                                  <span className="hidden whitespace-nowrap font-medium text-neutral-900 text-sm sm:inline dark:text-white">
                                    {modelName}
                                  </span>
                                </>
                              ) : null}
                            </div>

                            {/* Separator */}
                            <div className="h-6 w-px bg-neutral-300 dark:bg-neutral-600" />

                            {/* Platform icon */}
                            <div className="flex items-center gap-1.5">
                              <img
                                src={
                                  operatingSystem === 'macos'
                                    ? isDarkMode
                                      ? '/img/providers/macos-white.svg'
                                      : '/img/providers/macos-black.svg'
                                    : operatingSystem === 'windows'
                                      ? isDarkMode
                                        ? '/img/providers/windows-white.svg'
                                        : '/img/providers/windows-black.svg'
                                      : isDarkMode
                                        ? '/img/providers/linux-white.svg'
                                        : '/img/providers/linux-black.svg'
                                }
                                alt={
                                  operatingSystem === 'macos'
                                    ? 'macOS'
                                    : operatingSystem === 'windows'
                                      ? 'Windows'
                                      : 'Linux'
                                }
                                className="h-5 w-5 flex-shrink-0"
                              />
                              <span className="hidden whitespace-nowrap font-medium text-neutral-900 text-xs sm:inline dark:text-white">
                                {operatingSystem === 'macos'
                                  ? 'macOS'
                                  : operatingSystem === 'windows'
                                    ? 'Windows'
                                    : 'Linux'}
                              </span>
                            </div>
                          </>
                        )}
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};

export default TrajectoryViewer;

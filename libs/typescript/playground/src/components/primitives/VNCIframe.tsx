import React from 'react';

interface VNCIframeProps {
  src: string;
  width?: number;
  height?: number;
}

/**
 * Memoized VNC iframe component to prevent unnecessary re-renders.
 * This component will only re-render if src, width, or height actually change.
 *
 * Note: This is functionally identical to VNCViewer.tsx but kept as a separate
 * component for compatibility with imports from cloud repo patterns.
 */
const VNCIframe = React.memo(
  ({ src, width, height }: VNCIframeProps) => {
    // When explicit dimensions are provided, use their aspect ratio.
    // Otherwise, let the iframe fill its container naturally.
    const hasExplicitDimensions = width && height;

    return (
      <div
        className={hasExplicitDimensions ? 'w-full' : 'w-full h-full'}
        style={
          hasExplicitDimensions
            ? {
                aspectRatio: (width / height).toString(),
                maxHeight: `${height}px`,
              }
            : undefined
        }
      >
        <iframe
          key={src}
          src={src}
          className="h-full w-full"
          title="VNC Viewer"
          allow="clipboard-read; clipboard-write"
        />
      </div>
    );
  },
  // Custom comparison function - only re-render if src, width, or height actually change
  (prevProps, nextProps) => {
    return (
      prevProps.src === nextProps.src &&
      prevProps.width === nextProps.width &&
      prevProps.height === nextProps.height
    );
  }
);

VNCIframe.displayName = 'VNCIframe';

export default VNCIframe;
export type { VNCIframeProps };

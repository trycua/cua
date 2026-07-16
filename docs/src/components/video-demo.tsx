import type { ReactNode } from 'react';

interface VideoDemoProps {
  src: string;
  poster: string;
  title: string;
  sourceUrl?: string;
  className?: string;
  children?: ReactNode;
}

function withDocsBasePath(path: string) {
  if (!path.startsWith('/') || path.startsWith('/docs/')) return path;
  return `/docs${path}`;
}

export function VideoDemo({ src, poster, title, sourceUrl, className, children }: VideoDemoProps) {
  const mediaSrc = withDocsBasePath(src);
  const mediaPoster = withDocsBasePath(poster);

  return (
    <figure
      className={[
        'not-prose overflow-hidden rounded-xl border border-fd-border bg-fd-card',
        className,
      ]
        .filter(Boolean)
        .join(' ')}
    >
      <video
        aria-label={title}
        className="block aspect-video w-full bg-black object-contain"
        controls
        playsInline
        poster={mediaPoster}
        preload="metadata"
      >
        <source src={mediaSrc} type="video/mp4" />
        <a href={mediaSrc}>Open the video.</a>
      </video>
      <figcaption className="space-y-1.5 border-t border-fd-border px-4 py-3 text-sm">
        <div className="font-medium text-fd-foreground">{title}</div>
        {children ? <div className="text-fd-muted-foreground">{children}</div> : null}
        {sourceUrl ? (
          <a
            className="inline-block text-fd-primary underline underline-offset-4"
            href={sourceUrl}
            rel="noreferrer noopener"
            target="_blank"
          >
            View the original post on X
          </a>
        ) : null}
      </figcaption>
    </figure>
  );
}

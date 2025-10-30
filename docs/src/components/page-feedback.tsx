'use client';

import { useState } from 'react';
import posthog from 'posthog-js';
import { ThumbsUp, ThumbsDown } from 'lucide-react';

export function PageFeedback() {
  const [feedback, setFeedback] = useState<'helpful' | 'not_helpful' | null>(null);

  const handleFeedback = (isHelpful: boolean) => {
    const feedbackType = isHelpful ? 'helpful' : 'not_helpful';
    setFeedback(feedbackType);

    posthog.capture(`page_feedback_${feedbackType}`, {
      page: window.location.pathname,
      page_title: document.title,
    });
  };

  return (
    <div className="mt-8 pt-4 border-t border-fd-border">
      {feedback === null ? (
        <div className="flex flex-col sm:flex-row items-center justify-between gap-3">
          <p className="text-sm text-fd-muted-foreground">Was this page helpful?</p>
          <div className="flex gap-2">
            <button
              onClick={() => handleFeedback(true)}
              className="flex items-center gap-1.5 px-3 py-1.5 text-sm hover:bg-fd-accent rounded transition-colors"
              aria-label="This page was helpful"
            >
              <ThumbsUp className="w-4 h-4" />
              Yes
            </button>
            <button
              onClick={() => handleFeedback(false)}
              className="flex items-center gap-1.5 px-3 py-1.5 text-sm hover:bg-fd-accent rounded transition-colors"
              aria-label="This page was not helpful"
            >
              <ThumbsDown className="w-4 h-4" />
              No
            </button>
          </div>
        </div>
      ) : (
        <p className="text-sm text-fd-muted-foreground text-left">
          {feedback === 'helpful'
            ? 'Thanks for your feedback!'
            : "Thanks for your feedback. We'll work on improving this page."}
        </p>
      )}
    </div>
  );
}

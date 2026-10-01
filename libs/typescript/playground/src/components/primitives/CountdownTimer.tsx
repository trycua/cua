import { useEffect, useState } from 'react';

interface CountdownTimerProps {
  /** Total duration in seconds */
  durationSeconds: number;
  /** Callback when timer reaches 0 */
  onTimeout: () => void;
  /** Whether the timer is active */
  isActive: boolean;
}

export function CountdownTimer({ durationSeconds, onTimeout, isActive }: CountdownTimerProps) {
  const [secondsRemaining, setSecondsRemaining] = useState(durationSeconds);

  // Reset timer when it becomes active
  useEffect(() => {
    if (isActive) {
      setSecondsRemaining(durationSeconds);
    }
  }, [isActive, durationSeconds]);

  // Countdown effect
  useEffect(() => {
    if (!isActive) return;

    const interval = setInterval(() => {
      setSecondsRemaining((prev) => {
        if (prev <= 1) {
          clearInterval(interval);
          onTimeout();
          return 0;
        }
        return prev - 1;
      });
    }, 1000);

    return () => clearInterval(interval);
  }, [isActive, onTimeout]);

  if (!isActive) return null;

  // SVG circle properties
  const size = 14;
  const strokeWidth = 2;
  const radius = (size - strokeWidth) / 2;
  const circumference = 2 * Math.PI * radius;
  const progress = secondsRemaining / durationSeconds;
  const strokeDashoffset = circumference * (1 - progress);

  return (
    <div className="flex items-center gap-1.5">
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} className="rotate-[-90deg]">
        {/* Background circle */}
        <circle
          cx={size / 2}
          cy={size / 2}
          r={radius}
          fill="none"
          stroke="currentColor"
          strokeWidth={strokeWidth}
          className="text-neutral-200 dark:text-neutral-700"
        />
        {/* Progress circle */}
        <circle
          cx={size / 2}
          cy={size / 2}
          r={radius}
          fill="none"
          stroke="currentColor"
          strokeWidth={strokeWidth}
          strokeDasharray={circumference}
          strokeDashoffset={strokeDashoffset}
          strokeLinecap="round"
          className="text-neutral-400 transition-[stroke-dashoffset] duration-1000 ease-linear dark:text-neutral-500"
        />
      </svg>
      <span className="text-neutral-400 text-xs dark:text-neutral-500">
        {secondsRemaining}s until timeout
      </span>
    </div>
  );
}

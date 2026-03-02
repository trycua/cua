import { useEffect, useMemo, useState } from 'react';

const CHARACTERS = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz';

interface ScrambledTextProps {
  text: string;
  className?: string;
  scrambleDuration?: number;
  revealDelay?: number;
}

export function ScrambledText({
  text,
  className = '',
  scrambleDuration = 50,
  revealDelay = 30,
}: ScrambledTextProps) {
  const [displayText, setDisplayText] = useState('');
  const [revealedCount, setRevealedCount] = useState(0);

  const scrambleChar = () => CHARACTERS[Math.floor(Math.random() * CHARACTERS.length)];

  // biome-ignore lint/correctness/useExhaustiveDependencies: text dep needed to reset animation on phrase change
  useEffect(() => {
    setRevealedCount(0);
    setDisplayText('');
  }, [text]);

  useEffect(() => {
    if (revealedCount >= text.length) return;

    const revealInterval = setInterval(() => {
      setRevealedCount((prev) => Math.min(prev + 1, text.length));
    }, revealDelay);

    return () => clearInterval(revealInterval);
  }, [text, revealDelay, revealedCount]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: scrambleChar is intentionally excluded to avoid infinite re-renders
  useEffect(() => {
    const scrambleInterval = setInterval(() => {
      setDisplayText(
        text
          .split('')
          .map((char, index) => {
            if (char === ' ') return ' ';
            if (index < revealedCount) return text[index];
            return scrambleChar();
          })
          .join('')
      );
    }, scrambleDuration);

    return () => clearInterval(scrambleInterval);
  }, [text, revealedCount, scrambleDuration]);

  const chars = useMemo(() => displayText.split(''), [displayText]);

  return (
    <span className={className}>
      {chars.map((char, index) => (
        <span
          key={index}
          className={
            index < revealedCount
              ? 'text-neutral-600 dark:text-neutral-400'
              : 'text-neutral-400 dark:text-neutral-500'
          }
        >
          {char}
        </span>
      ))}
    </span>
  );
}

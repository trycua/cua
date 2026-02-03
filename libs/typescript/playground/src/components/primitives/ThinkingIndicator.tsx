import Lottie from 'lottie-react';
import { useEffect, useState } from 'react';
import { ScrambledText } from './ScrambledText';

const THINKING_PHRASES = [
  'cuala is brainstorming...',
  'cuala is ideating...',
  'cuala is philosophizing...',
  'cuala is pondering...',
  'cuala is contemplating...',
  'cuala is reasoning...',
  'cuala is analyzing...',
  'cuala is synthesizing...',
];

interface ThinkingIndicatorProps {
  phrases?: string[];
  lightAnimationUrl?: string;
  darkAnimationUrl?: string;
}

export function ThinkingIndicator({
  phrases = THINKING_PHRASES,
  lightAnimationUrl = '/playground/thinking_animation_black.json',
  darkAnimationUrl = '/playground/thinking_animation_white.json',
}: ThinkingIndicatorProps) {
  const [phraseIndex, setPhraseIndex] = useState(0);
  const [lightAnimation, setLightAnimation] = useState<object | null>(null);
  const [darkAnimation, setDarkAnimation] = useState<object | null>(null);

  useEffect(() => {
    const interval = setInterval(() => {
      setPhraseIndex((prev) => (prev + 1) % phrases.length);
    }, 3000);

    return () => clearInterval(interval);
  }, [phrases.length]);

  useEffect(() => {
    // Fetch both animation JSON files
    fetch(lightAnimationUrl)
      .then((res) => res.json())
      .then(setLightAnimation)
      .catch(() => {});

    fetch(darkAnimationUrl)
      .then((res) => res.json())
      .then(setDarkAnimation)
      .catch(() => {});
  }, [lightAnimationUrl, darkAnimationUrl]);

  return (
    <div className="flex items-center gap-3">
      <div className="flex items-center">
        {/* Light mode: black animation for light backgrounds */}
        {lightAnimation && (
          <Lottie animationData={lightAnimation} loop={true} className="h-6 w-6 dark:hidden" />
        )}
        {/* Dark mode: white animation for dark backgrounds */}
        {darkAnimation && (
          <Lottie animationData={darkAnimation} loop={true} className="hidden h-6 w-6 dark:block" />
        )}
      </div>
      <ScrambledText
        text={phrases[phraseIndex]}
        className="font-mono text-sm"
        scrambleDuration={40}
        revealDelay={50}
      />
    </div>
  );
}

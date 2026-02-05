import Lottie from 'lottie-react';
import { useEffect, useState } from 'react';

interface ThinkingCompleteAnimationProps {
  className?: string;
  lightAnimationUrl?: string;
  darkAnimationUrl?: string;
}

export function ThinkingCompleteAnimation({
  className = '',
  lightAnimationUrl = '/playground/lfg_animation_black.json',
  darkAnimationUrl = '/playground/lfg_animation_white.json',
}: ThinkingCompleteAnimationProps) {
  const [lightAnimation, setLightAnimation] = useState<object | null>(null);
  const [darkAnimation, setDarkAnimation] = useState<object | null>(null);

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
    <>
      {/* Light mode: black animation for light backgrounds */}
      {lightAnimation && (
        <Lottie
          animationData={lightAnimation}
          loop={false}
          className={`h-4 w-4 dark:hidden ${className}`}
        />
      )}
      {/* Dark mode: white animation for dark backgrounds */}
      {darkAnimation && (
        <Lottie
          animationData={darkAnimation}
          loop={false}
          className={`hidden h-4 w-4 dark:block ${className}`}
        />
      )}
    </>
  );
}

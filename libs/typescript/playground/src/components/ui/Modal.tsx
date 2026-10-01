import { X } from 'lucide-react';
import type React from 'react';
import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { cn } from '../../utils/cn';

export interface ModalProps {
  isOpen: boolean;
  onClose: () => void;
  title?: string;
  description?: string;
  children: React.ReactNode;
  className?: string;
  contentClassName?: string;
  showCloseButton?: boolean;
  closeOnOutsideClick?: boolean;
}

export const Modal = ({
  isOpen,
  onClose,
  children,
  className,
  contentClassName,
  showCloseButton = true,
  closeOnOutsideClick = true,
}: ModalProps) => {
  const [isMounted, setIsMounted] = useState(false);

  useEffect(() => {
    setIsMounted(true);

    if (isOpen) {
      document.body.style.overflow = 'hidden';
    }

    return () => {
      document.body.style.overflow = 'auto';
    };
  }, [isOpen]);

  useEffect(() => {
    const handleEscape = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose();
      }
    };

    if (isOpen) {
      document.addEventListener('keydown', handleEscape);
    }

    return () => {
      document.removeEventListener('keydown', handleEscape);
    };
  }, [isOpen, onClose]);

  const handleBackdropClick = (e: React.MouseEvent | React.KeyboardEvent) => {
    // For keyboard events, ignore if modifier keys are pressed
    if ('key' in e && (e.ctrlKey || e.metaKey || e.altKey || e.shiftKey)) {
      return;
    }

    if (closeOnOutsideClick && e.target === e.currentTarget) {
      onClose();
    }
  };

  if (!isMounted || !isOpen) {
    return null;
  }

  const modalContent = (
    <dialog
      className={cn(
        'fixed inset-0 z-[60] flex h-full w-full items-center justify-center bg-black/50 px-4 backdrop-blur-sm md:px-0',
        className
      )}
      onClick={handleBackdropClick}
      onKeyDown={handleBackdropClick}
      aria-modal="true"
    >
      <div
        className={cn(
          'fade-in-0 zoom-in-95 animate-in duration-200',
          'relative w-full max-w-xl rounded-lg border bg-white p-6 shadow-lg dark:border-neutral-700 dark:bg-neutral-800',
          'overflow-y-auto',
          contentClassName
        )}
      >
        {showCloseButton && (
          <button
            type="button"
            onClick={onClose}
            className="absolute top-4 right-4 rounded-sm opacity-70 ring-offset-white transition-opacity hover:opacity-100 focus:outline-none focus:ring-2 focus:ring-neutral-400 focus:ring-offset-2 disabled:pointer-events-none dark:text-neutral-300 dark:ring-offset-neutral-800 dark:focus:ring-neutral-500"
            aria-label="Close modal"
          >
            <X className="h-4 w-4" />
          </button>
        )}

        {children}
      </div>
    </dialog>
  );

  return createPortal(modalContent, document.body);
};

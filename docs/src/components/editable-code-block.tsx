'use client';

import React, { createContext, useContext, useState, ReactNode } from 'react';
import * as Base from 'fumadocs-ui/components/codeblock';
import { cn } from 'fumadocs-ui/utils/cn';

/**
 * Context for managing editable values within code blocks
 */
interface EditableCodeContextValue {
  values: Record<string, string>;
  updateValue: (key: string, value: string) => void;
}

const EditableCodeContext = createContext<EditableCodeContextValue | null>(null);

/**
 * Hook to access the editable code context
 */
function useEditableCode() {
  const context = useContext(EditableCodeContext);
  if (!context) {
    throw new Error('useEditableCode must be used within EditableCodeBlock');
  }
  return context;
}

/**
 * Props for EditableCodeBlock component
 */
interface EditableCodeBlockProps {
  /** Programming language for styling */
  lang?: string;
  /** Initial values for placeholders */
  defaultValues?: Record<string, string>;
  /** Code content with embedded EditableValue components */
  children: ReactNode;
  /** Additional CSS classes */
  className?: string;
  /** Title for the code block */
  title?: string;
}

/**
 * Code block component that supports inline editable values
 * Uses fumadocs-ui styling with interactive input fields
 */
export function EditableCodeBlock({
  lang = 'python',
  defaultValues = {},
  children,
  className,
  title,
}: EditableCodeBlockProps) {
  const [values, setValues] = useState<Record<string, string>>(defaultValues);

  const updateValue = (key: string, value: string) => {
    setValues((prev) => ({ ...prev, [key]: value }));
  };

  return (
    <EditableCodeContext.Provider value={{ values, updateValue }}>
      <Base.CodeBlock title={title} className={cn('my-4', className)}>
        <Base.Pre className={cn(`language-${lang}`, 'px-3')}>
          <code
            className={cn(`language-${lang}`)}
            style={{ display: 'block', whiteSpace: 'pre-wrap' }}
          >
            {children}
          </code>
        </Base.Pre>
      </Base.CodeBlock>
    </EditableCodeContext.Provider>
  );
}

/**
 * Props for EditableValue component
 */
interface EditableValueProps {
  /** Unique identifier for this value */
  placeholder: string;
  /** Display width in characters (default: auto) */
  width?: number;
  /** Optional default value */
  defaultValue?: string;
  /** Input type */
  type?: 'text' | 'password';
}

/**
 * Inline editable input that blends with code styling
 * Appears as an underlined, hoverable value within code
 */
export function EditableValue({
  placeholder,
  width: explicitWidth,
  defaultValue = '',
  type = 'text',
}: EditableValueProps) {
  const { values, updateValue } = useEditableCode();
  const value = values[placeholder] ?? defaultValue;
  const spanRef = React.useRef<HTMLSpanElement>(null);
  const placeholderSpanRef = React.useRef<HTMLSpanElement>(null);
  const inputRef = React.useRef<HTMLInputElement>(null);
  const [measuredWidth, setMeasuredWidth] = React.useState(0);
  const [placeholderWidth, setPlaceholderWidth] = React.useState(0);
  const [isHovered, setIsHovered] = React.useState(false);
  const [tooltipPosition, setTooltipPosition] = React.useState({ top: 0, left: 0 });
  const [isVisible, setIsVisible] = React.useState(false);

  // Observe visibility changes to trigger remeasurement
  React.useEffect(() => {
    if (!inputRef.current) return;

    const observer = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          setIsVisible(entry.isIntersecting);
        });
      },
      { threshold: 0.01 }
    );

    observer.observe(inputRef.current);

    return () => {
      observer.disconnect();
    };
  }, []);

  // Measure the actual text width using a hidden span
  React.useEffect(() => {
    if (spanRef.current && isVisible) {
      setMeasuredWidth(spanRef.current.offsetWidth);
    }
  }, [value, isVisible]);

  // Measure placeholder width when visible
  React.useEffect(() => {
    if (placeholderSpanRef.current && isVisible) {
      setPlaceholderWidth(placeholderSpanRef.current.offsetWidth);
    }
  }, [placeholder, isVisible]);

  // Update tooltip position when hovered
  React.useEffect(() => {
    if (isHovered && inputRef.current) {
      const rect = inputRef.current.getBoundingClientRect();
      setTooltipPosition({
        top: rect.top - 28,
        left: rect.left + rect.width / 2,
      });
    }
  }, [isHovered]);

  const inputWidth = explicitWidth
    ? `${explicitWidth}ch`
    : `${Math.max(placeholderWidth, measuredWidth, 80)}px`;

  return (
    <span
      style={{ display: 'inline', whiteSpace: 'nowrap', position: 'relative' }}
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
    >
      {/* Hidden span to measure current value width */}
      <span
        ref={spanRef}
        style={{
          position: 'absolute',
          visibility: 'hidden',
          whiteSpace: 'pre',
          fontFamily: 'inherit',
          pointerEvents: 'none',
        }}
        aria-hidden="true"
      >
        {value}
      </span>

      {/* Hidden span to measure placeholder width */}
      <span
        ref={placeholderSpanRef}
        style={{
          position: 'absolute',
          visibility: 'hidden',
          whiteSpace: 'pre',
          fontFamily: 'inherit',
          pointerEvents: 'none',
        }}
        aria-hidden="true"
      >
        {placeholder}
      </span>

      {/* Tooltip */}
      <span
        style={{
          position: 'fixed',
          top: tooltipPosition.top,
          left: tooltipPosition.left,
          transform: 'translateX(-50%)',
          padding: '4px 8px',
          backgroundColor: 'rgba(0, 0, 0, 0.8)',
          color: 'white',
          fontSize: '12px',
          borderRadius: '4px',
          whiteSpace: 'nowrap',
          pointerEvents: 'none',
          opacity: isHovered ? 1 : 0,
          transition: 'opacity 0.2s ease-in-out',
          zIndex: 9999,
        }}
      >
        Edit me!
      </span>

      <input
        ref={inputRef}
        type={type}
        value={value}
        onChange={(e) => updateValue(placeholder, e.target.value)}
        placeholder={placeholder}
        className={cn(type === 'password' && value && 'text-security-disc')}
        style={{
          display: 'inline',
          width: inputWidth,
          verticalAlign: 'baseline',
          lineHeight: 'inherit',
          fontSize: 'inherit',
          fontFamily: 'inherit',
          height: 'auto',
          padding: 0,
          margin: 0,
          background: 'transparent',
          border: 'none',
          borderBottom: '2px dashed rgba(96, 165, 250, 0.5)',
          outline: 'none',
          color: 'inherit',
          transition: 'border-bottom-color 0.2s ease-in-out',
        }}
      />
    </span>
  );
}

/**
 * Container for form inputs outside the code block
 */
export function EditableForm({
  children,
  className = '',
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        'p-4 border rounded-lg bg-fd-secondary/50 dark:bg-fd-secondary/30 mb-6',
        className
      )}
    >
      <h3 className="text-lg font-semibold mb-4">Configuration</h3>
      {children}
    </div>
  );
}

/**
 * Form input for editing values outside code block
 */
interface EditableInputProps {
  /** Placeholder key to bind to */
  placeholder: string;
  /** Label text */
  label: string;
  /** Input type */
  type?: 'text' | 'email' | 'password';
  /** Custom class name */
  className?: string;
}

export function EditableInput({
  placeholder,
  label,
  type = 'text',
  className = '',
}: EditableInputProps) {
  const { values, updateValue } = useEditableCode();
  const value = values[placeholder] || '';

  return (
    <div className={cn('mb-4', className)}>
      <label className="block text-sm font-medium mb-2">{label}</label>
      <input
        type={type}
        value={value}
        onChange={(e) => updateValue(placeholder, e.target.value)}
        placeholder={placeholder}
        className={cn(
          'w-full px-3 py-2 border rounded-md',
          'focus:outline-none focus:ring-2 focus:ring-blue-500',
          'bg-fd-background border-fd-border'
        )}
      />
    </div>
  );
}

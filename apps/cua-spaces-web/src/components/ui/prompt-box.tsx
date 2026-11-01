// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, CopyIcon } from "lucide-react";
import { useEffect, useRef, useState, type ComponentProps } from "react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/**
 * A prompt the user copies into an agent: a read-only, filled text box with
 * no border and Copy inside it, the way every agent prompt in the app looks.
 * Copy writes `value` to the clipboard and says "Copied" for a moment
 * (`copied` keeps it saying so). Selecting the text selects all of it.
 */
export function PromptBox({
  value,
  onCopy,
  copied: held,
  disabled,
  className,
  ...props
}: {
  value: string;
  /** After the text is on the clipboard. */
  onCopy?: () => void;
  /** Keep showing "Copied" (a sheet that then waits for the agent). */
  copied?: boolean;
  disabled?: boolean;
  className?: string;
} & Omit<ComponentProps<"textarea">, "value" | "onCopy" | "readOnly" | "className">) {
  const [flash, setFlash] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => void (timer.current && clearTimeout(timer.current)), []);
  const copied = held || flash;
  const copy = () => {
    if (disabled) return;
    const done = () => {
      setFlash(true);
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => setFlash(false), 1500);
      onCopy?.();
    };
    const write = navigator.clipboard?.writeText(value);
    if (write) void write.then(done, done);
    else done();
  };
  return (
    <div data-prompt-box="" className={cn("relative rounded-lg bg-muted", disabled && "opacity-60", className)}>
      <textarea
        readOnly
        value={value}
        rows={1}
        onFocus={(e) => e.currentTarget.select()}
        className="block w-full resize-none bg-transparent py-2.5 pr-24 pl-3 font-sans text-[13px] leading-relaxed text-foreground outline-none [field-sizing:content] focus-visible:ring-2 focus-visible:ring-ring/60 rounded-lg"
        {...props}
      />
      <Button
        size="sm"
        variant={copied ? "ghost" : "default"}
        disabled={disabled}
        data-prompt-copy={copied ? "copied" : ""}
        onClick={copy}
        className="absolute top-1.5 right-1.5"
      >
        {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
        {copied ? "Copied" : "Copy"}
      </Button>
    </div>
  );
}

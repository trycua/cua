// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Checkbox as CheckboxPrimitive } from "@base-ui/react/checkbox";
import { CheckIcon, MinusIcon } from "lucide-react";

import { cn } from "@/lib/utils";

export function Checkbox({ className, ...props }: CheckboxPrimitive.Root.Props) {
  return (
    <CheckboxPrimitive.Root
      className={cn(
        "flex size-4 shrink-0 items-center justify-center rounded-[5px] border border-input bg-card shadow-xs outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/60 data-checked:border-brand data-checked:bg-brand data-indeterminate:border-brand data-indeterminate:bg-brand dark:bg-input/30",
        className,
      )}
      data-slot="checkbox"
      {...props}
    >
      <CheckboxPrimitive.Indicator
        className="text-white"
        render={(p, state) => (
          <span {...p}>{state.indeterminate ? <MinusIcon className="size-3" strokeWidth={3} /> : <CheckIcon className="size-3" strokeWidth={3} />}</span>
        )}
      />
    </CheckboxPrimitive.Root>
  );
}

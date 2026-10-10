// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Switch as SwitchPrimitive } from "@base-ui/react/switch";

import { cn } from "@/lib/utils";

export function Switch({ className, ...props }: SwitchPrimitive.Root.Props) {
  return (
    <SwitchPrimitive.Root
      className={cn(
        "inline-flex h-[18px] w-[30px] shrink-0 items-center rounded-full p-[2px] outline-none transition-colors duration-200 focus-visible:ring-2 focus-visible:ring-ring/60 data-checked:bg-brand data-unchecked:bg-input data-disabled:opacity-50",
        className,
      )}
      data-slot="switch"
      {...props}
    >
      <SwitchPrimitive.Thumb className="pointer-events-none block size-[14px] rounded-full bg-white shadow-sm transition-transform duration-200 ease-out-soft data-checked:translate-x-[12px]" />
    </SwitchPrimitive.Root>
  );
}

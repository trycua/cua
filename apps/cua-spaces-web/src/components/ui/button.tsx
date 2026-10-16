// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { mergeProps } from "@base-ui/react/merge-props";
import { useRender } from "@base-ui/react/use-render";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "relative inline-flex shrink-0 cursor-default items-center justify-center gap-1.5 whitespace-nowrap rounded-lg border font-medium text-[13px] outline-none transition-[background-color,box-shadow,scale] duration-150 active:not-disabled:scale-[0.98] focus-visible:ring-2 focus-visible:ring-ring/60 disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default: "border-transparent bg-primary text-primary-foreground shadow-xs hover:bg-primary/90",
        outline:
          "border-input bg-card text-foreground shadow-xs hover:bg-accent dark:bg-input/30 dark:hover:bg-input/50",
        secondary: "border-transparent bg-secondary text-secondary-foreground hover:bg-accent",
        ghost: "border-transparent text-foreground hover:bg-accent",
        destructive: "border-transparent bg-destructive text-destructive-foreground hover:bg-destructive/90",
      },
      size: {
        default: "h-8 px-3",
        sm: "h-7 rounded-md px-2.5 text-xs",
        lg: "h-9 px-4",
        icon: "size-8",
        "icon-sm": "size-7 rounded-md",
      },
    },
    defaultVariants: { variant: "default", size: "default" },
  },
);

interface ButtonProps extends useRender.ComponentProps<"button">, VariantProps<typeof buttonVariants> {}

export function Button({ className, variant, size, render, ...props }: ButtonProps) {
  return useRender({
    defaultTagName: "button",
    render,
    props: mergeProps<"button">(
      {
        className: cn(buttonVariants({ variant, size }), className),
        type: render ? undefined : "button",
      },
      props,
    ),
  });
}

export { buttonVariants };

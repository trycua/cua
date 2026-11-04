// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cn } from "@/lib/utils";

/** Each cloud's own mark, as the vendor publishes it (`public/cloud-logos/README.md`). */
const LOGO: Record<string, { name: string; src: string; dark?: string; wide?: boolean }> = {
  aws: {
    name: "AWS",
    // The dark-ink logo on light pages, the white header logo on dark ones.
    src: "/cloud-logos/aws_logo_smile_179x109.png",
    dark: "/cloud-logos/aws_smile-header-desktop-en-white_59x35.png",
    wide: true,
  },
  gcp: { name: "Google Cloud", src: "/cloud-logos/google-cloud-super_cloud_gradient.png" },
  azure: { name: "Microsoft Azure", src: "/cloud-logos/azure-logo_azure.svg" },
  hetzner: { name: "Hetzner", src: "/cloud-logos/hetzner-apple-icon-180x180.png" },
  modal: { name: "Modal", src: "/cloud-logos/modal-favicon.svg" },
};

/**
 * A cloud's logo at the size of the OS icons beside it (`size`: the square
 * it fits, in px). AWS's wordmark is wide, so it keeps its height and takes
 * its own width.
 */
export function CloudLogo({ cloud, size = 14, className }: { cloud: string; size?: number; className?: string }) {
  const l = LOGO[cloud];
  if (!l) return <span aria-hidden className={cn("shrink-0", className)} style={{ width: size, height: size }} />;
  if (!l.wide) {
    return (
      <span className={cn("inline-flex shrink-0 items-center justify-center", className)} style={{ width: size, height: size }} data-cloud-logo={cloud}>
        <img src={l.src} alt={l.name} draggable={false} className="size-full rounded-[3px] object-contain" />
      </span>
    );
  }
  // The wordmark is wide: it keeps the row's height and takes its own width. The white header logo (dark
  // pages) fills its file; the dark-ink logo (light pages) has margins, so it is drawn larger than its box,
  // and multiplied so that its white file background takes the page's color.
  const h = size + 4;
  return (
    <span className={cn("relative inline-block shrink-0", className)} style={{ height: h, width: Math.round(h * 1.7) }} data-cloud-logo={cloud}>
      <img
        src={l.src}
        alt={l.name}
        draggable={false}
        className="absolute top-1/2 left-1/2 max-w-none -translate-x-1/2 -translate-y-1/2 mix-blend-multiply dark:hidden"
        style={{ height: Math.round(h * 1.75) }}
      />
      <img
        src={l.dark}
        alt={l.name}
        draggable={false}
        className="absolute top-1/2 left-1/2 hidden max-w-none -translate-x-1/2 -translate-y-1/2 dark:block"
        style={{ height: h }}
      />
    </span>
  );
}

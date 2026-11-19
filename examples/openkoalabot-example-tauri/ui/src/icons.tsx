// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Line icons (24px grid, 1.8 stroke), drawn for this app.
import type { ReactNode } from "react"

function Icon({ children, size = 16 }: { children: ReactNode; size?: number }) {
  return (
    <svg className="icon" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {children}
    </svg>
  )
}

export const Plus = () => <Icon><path d="M12 5v14M5 12h14" /></Icon>
export const Search = () => <Icon><circle cx="11" cy="11" r="6.5" /><path d="m20 20-4-4" /></Icon>
export const Paperclip = () => <Icon><path d="M20.5 11.5 12.4 19.6a5 5 0 0 1-7.1-7.1l8.5-8.5a3.3 3.3 0 0 1 4.7 4.7l-8.5 8.5a1.7 1.7 0 0 1-2.4-2.4l7.8-7.8" /></Icon>
export const ArrowUp = () => <Icon size={18}><path d="M12 19V5M6 11l6-6 6 6" /></Icon>
export const Monitor = ({ size }: { size?: number }) => <Icon size={size}><rect x="3" y="4" width="18" height="12" rx="2" /><path d="M8 20h8M12 16v4" /></Icon>
export const Stop = () => <Icon><rect x="6" y="6" width="12" height="12" rx="2" /></Icon>
export const Play = () => <Icon><path d="M7 5v14l11-7z" /></Icon>
export const Pip = () => <Icon><rect x="3" y="5" width="18" height="14" rx="1.5" /><rect x="12" y="11.5" width="6.5" height="5" rx="0.8" /></Icon>
export const Expand = () => <Icon><path d="M14 4h6v6M10 20H4v-6M20 4l-7 7M4 20l7-7" /></Icon>
export const Close = () => <Icon><path d="M6 6l12 12M18 6 6 18" /></Icon>
export const PanelRight = () => <Icon><rect x="3" y="4" width="18" height="16" rx="2" /><path d="M15 4v16" /></Icon>
export const Upload = () => <Icon size={20}><path d="M12 16V4M7 9l5-5 5 5M4 16v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2" /></Icon>
export const Terminal = () => <Icon><path d="m5 8 4 4-4 4M12 16h7" /></Icon>
export const Info = () => <Icon><circle cx="12" cy="12" r="9" /><path d="M12 11v5M12 8h.01" /></Icon>
export const Alert = () => <Icon><path d="M12 4 2.8 19.5h18.4z" /><path d="M12 10v4M12 17h.01" /></Icon>
export const FileIcon = () => <Icon><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" /><path d="M14 3v5h5" /></Icon>
export const Cloud = () => <Icon><path d="M7 18h10a4 4 0 0 0 .6-8A6 6 0 0 0 6 9.5 4.3 4.3 0 0 0 7 18z" /></Icon>
export const Laptop = () => <Icon><rect x="4" y="5" width="16" height="11" rx="1.5" /><path d="M2 19h20" /></Icon>
export const Teleport = () => <Icon><path d="M4 12h11M11 7l5 5-5 5" /><path d="M20 4v16" /></Icon>
export const Link = () => <Icon><path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1" /><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1" /></Icon>
export const Check = () => <Icon><path d="m5 12 5 5 9-10" /></Icon>

/** OS glyphs for the wizard tiles (simple geometric marks, no logos). */
export const OsGlyph = ({ os }: { os: "linux" | "windows" | "macos" }) =>
  os === "windows" ? (
    <Icon size={26}><rect x="4" y="4" width="7" height="7" rx="1" /><rect x="13" y="4" width="7" height="7" rx="1" /><rect x="4" y="13" width="7" height="7" rx="1" /><rect x="13" y="13" width="7" height="7" rx="1" /></Icon>
  ) : os === "macos" ? (
    <Icon size={26}><rect x="3" y="4" width="18" height="13" rx="2" /><path d="M9 21h6M3 8h18" /><circle cx="6" cy="6" r=".3" /></Icon>
  ) : (
    <Icon size={26}><path d="M5 7l5 5-5 5M12 17h7" /><rect x="2" y="3" width="20" height="18" rx="2" /></Icon>
  )
export const Clock = () => <Icon><circle cx="12" cy="12" r="9" /><path d="M12 7v5l3 2" /></Icon>

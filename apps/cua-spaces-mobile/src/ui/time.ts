// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

const DAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

const pad = (n: number) => String(n).padStart(2, "0");
export const clock = (ms: number) => {
  const d = new Date(ms);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
};

const startOfDay = (ms: number) => {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
};

/** A conversation row's time: the clock today, "Yesterday", the weekday this week, else the date. */
export function rowTime(ms: number, now: number): string {
  const days = Math.round((startOfDay(now) - startOfDay(ms)) / 86_400_000);
  if (days <= 0) return clock(ms);
  if (days === 1) return "Yesterday";
  if (days < 7) return DAYS[new Date(ms).getDay()]!;
  const d = new Date(ms);
  return `${d.getMonth() + 1}/${d.getDate()}/${String(d.getFullYear()).slice(2)}`;
}

/** The thread's day line: "Today 09:12", "Yesterday 18:02", "Tuesday 10:31". */
export function dayLine(ms: number, now: number): string {
  const days = Math.round((startOfDay(now) - startOfDay(ms)) / 86_400_000);
  const when =
    days <= 0 ? "Today" : days === 1 ? "Yesterday" : days < 7 ? DAYS[new Date(ms).getDay()]! : `${new Date(ms).getMonth() + 1}/${new Date(ms).getDate()}`;
  return `${when} ${clock(ms)}`;
}

/** "last seen 26 hours ago" for an offline machine. */
export function ago(ms: number, now: number): string {
  const m = Math.max(0, Math.round((now - ms) / 60_000));
  if (m < 1) return "just now";
  if (m < 60) return `${m} minute${m === 1 ? "" : "s"} ago`;
  const h = Math.round(m / 60);
  if (h < 48) return `${h} hour${h === 1 ? "" : "s"} ago`;
  return `${Math.round(h / 24)} days ago`;
}

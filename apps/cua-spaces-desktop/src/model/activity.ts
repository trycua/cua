// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// What the notch's indicator left of the camera shows (the SwiftUI app's
// `NotchModel.setActivity`): the hotspot, and whether a transfer runs (a
// teleport, files dropped on a notch tile). Transfers overlap: the
// indicator stays until the last one ends.

export class NotchActivity {
  hotspot = false;
  private transfers = 0;
  private readonly listeners = new Set<() => void>();

  /** A transfer runs. */
  get transfer(): boolean {
    return this.transfers > 0;
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private changed(): void {
    for (const l of [...this.listeners]) l();
  }

  setHotspot(on: boolean): void {
    if (on === this.hotspot) return;
    this.hotspot = on;
    this.changed();
  }

  /** A transfer started; the returned function ends it (once). */
  begin(): () => void {
    this.transfers += 1;
    if (this.transfers === 1) this.changed();
    let ended = false;
    return () => {
      if (ended) return;
      ended = true;
      this.transfers -= 1;
      if (this.transfers === 0) this.changed();
    };
  }

  /** Runs `body` as a transfer. */
  async during<T>(body: () => Promise<T>): Promise<T> {
    const end = this.begin();
    try {
      return await body();
    } finally {
      end();
    }
  }
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// What the notch's indicator left of the camera shows (the SwiftUI app's
// `NotchModel.setActivity`): the hotspot (a manual one, or a Space whose
// traffic exits through this machine), and whether a transfer runs (a
// teleport, files dropped on a notch tile). Transfers overlap: the
// indicator stays until the last one ends.

export class NotchActivity {
  private manual = false;
  private exits: string[] = [];
  private transfers = 0;

  /** This machine carries some Space's internet traffic. */
  get hotspot(): boolean {
    return this.manual || this.exits.length > 0;
  }

  /** The Spaces whose traffic exits through this machine. */
  get exitSpaces(): readonly string[] {
    return this.exits;
  }
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
    if (on === this.manual) return;
    const was = this.hotspot;
    this.manual = on;
    if (was !== this.hotspot) this.changed();
  }

  /** The Spaces whose "Exit through" is this machine, as the host keeps them. */
  setExits(spaceIds: string[]): void {
    const next = [...new Set(spaceIds)].sort();
    if (next.length === this.exits.length && next.every((id, i) => id === this.exits[i])) return;
    this.exits = next;
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

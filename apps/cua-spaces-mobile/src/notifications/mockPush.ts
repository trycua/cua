// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A mock push provider: stands in for the relay's sender and APNs, so push is built and tested without an APNs key.
 * It keeps the registrations the phone made (what `PUT /v1/push/registrations/{device}` would store) and "delivers"
 * the payload the relay sends (`cua-push`): an `aps` alert with a category, and the structured keys the app routes from at
 * the top level. Used by the tests and by nothing else.
 */

/** An APNs body as the relay builds it (`cua-push` `Payload::apns_json`): `aps` for the system, and the keys the app routes from at the top level. */
export interface PushPayload {
  aps: {
    alert: { title: string; body: string };
    category: "CUA_APPROVAL" | "CUA_DONE";
    "thread-id": string;
    sound: "default";
    "interruption-level": "time-sensitive" | "active";
  };
  v: 1;
  kind: "approval" | "done" | "error" | "blocked";
  space: string;
  entry?: number;
  approval?: number;
  scope_choices?: ("once" | "space")[];
}

/** What the system hands the app as the notification's `data`: the payload without `aps`. */
export function dataOf(p: PushPayload): Omit<PushPayload, "aps"> {
  const { aps: _aps, ...data } = p;
  return data;
}

export class MockPushProvider {
  readonly registrations: Record<string, unknown>[] = [];
  removed = 0;

  /** The transport the sample backend (or a test) gives the app as `backend.push`. */
  /** The relay's `GET /v1/push`: whether it can send (a provider is set up). The mock sends, unless told it cannot. */
  configured = true;

  readonly transport = {
    status: async () => ({ configured: this.configured }),
    register: async (body: Record<string, unknown>) => {
      this.registrations.push(body);
    },
    unregister: async () => {
      this.removed += 1;
    },
  };

  /** What the relay would send for an approval. */
  approval(space: string, entry: number, title = "Needs you", body = "An agent is waiting for your answer."): PushPayload {
    return {
      aps: { alert: { title, body }, category: "CUA_APPROVAL", "thread-id": space, sound: "default", "interruption-level": "time-sensitive" },
      v: 1,
      kind: "approval",
      space,
      entry,
      approval: entry,
      scope_choices: ["once"],
    };
  }

  /** What the relay would send when an agent finishes. */
  done(space: string, entry: number, title = "Done", body = "An agent finished."): PushPayload {
    return {
      aps: { alert: { title, body }, category: "CUA_DONE", "thread-id": space, sound: "default", "interruption-level": "active" },
      v: 1,
      kind: "done",
      space,
      entry,
    };
  }

  /** Whether a push to this phone would be sent for `kind`, from what it registered. */
  wouldSend(kind: "needs_you" | "done"): boolean {
    const last = this.registrations.at(-1) as { categories?: string[] } | undefined;
    return !!last && (last.categories ?? []).includes(kind);
  }
}

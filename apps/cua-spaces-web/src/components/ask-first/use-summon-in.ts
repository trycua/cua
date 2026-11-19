// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { firstMessage, threadOpAvailable, useBridge, type AgentMember } from "@/bridge";
import { sendWithSummon } from "@/components/space-thread/summon";
import { useConnectors } from "@/hooks/use-connectors";

/**
 * Putting a message in a Space's thread the way the composer does: through
 * the summon, so `@claude ...` adds the agent when it is not a member yet.
 * Ask first's box and the preparing driver both post through this, so a
 * message queued while the home was made and one typed once it was ready go
 * the same way.
 */
export function useSummonIn() {
  const { data: host, core } = useBridge();
  const { input: connectors } = useConnectors();

  /** What summoning reads for `spaceId`: its agents now and this device's connectors. */
  const summonInput = async (spaceId: string) => {
    const members = ((await host!.call("thread.members", { spaceId }).catch(() => null))?.members ?? []) as AgentMember[];
    // The Space's handle for the member (`claude`), never the host's key for it (`claude-8` in `cua agent ls`): the thread matches the handle.
    return {
      members: members.map((m) => ({ handle: m.handle ?? m.name, key: m.name, mine: true, harness: m.harness, label: m.name })),
      people: [],
      canAdd: true,
      reportUnknown: false,
      connectors,
    };
  };
  const send = (spaceId: string, input: Awaited<ReturnType<typeof summonInput>>) => (text: string) =>
    sendWithSummon({ core, data: host!, spaceId, input, text, post: (t) => host!.call("thread.post", { spaceId, text: t }) });
  const canSummon = () => !!host && threadOpAvailable(host.mode, "thread.agentAdd");

  /**
   * The job as the Space's first message, through the summon; false when this agent can't join a Space. A
   * home's agent is kept on standby with the picked model (adding it again updates it), so its session is open
   * before the first words and the model it asks for is the one its next turn uses.
   */
  const summonIn = async (
    spaceId: string,
    agent: string,
    prompt: string,
    model?: string,
    standby = false,
    brief?: string,
    effort?: string,
  ): Promise<boolean> => {
    if (!canSummon()) return false;
    const input = await summonInput(spaceId);
    const first = firstMessage(core, input, agent, prompt);
    if (!first?.handle) return false;
    if (standby || model || brief || effort) {
      await host!
        .call("thread.agentAdd", {
          spaceId,
          name: first.handle,
          harness: first.harness || agent,
          ...(model ? { model } : {}),
          ...(effort ? { effort } : {}),
          ...(standby ? { standby: true } : {}),
          ...(brief ? { brief } : {}),
        })
        .catch(() => null);
    }
    // The member is in now: the message goes with it listed.
    await send(spaceId, standby || model || brief || effort ? await summonInput(spaceId) : input)(first.text);
    return true;
  };

  return { summonInput, send, summonIn, canSummon };
}

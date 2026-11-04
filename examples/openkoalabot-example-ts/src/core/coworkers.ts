/**
 * Routines and group chats over the app's `BotStore`: the live
 * `RoutineRunner` and `GroupMessenger` for the SDK's `RoutineStore` and
 * `GroupChatStore` (`@trycua/cua/spaces/routines`, `/groups`). The same
 * semantics as the Swift sample's `BotStoreRoutineRunner` and
 * `BotStoreGroupMessenger`:
 *
 * - a routine for a Bot mid-turn is refused (the slot is skipped), never
 *   interrupted; a Bot with a thread that takes a message gets a new turn on
 *   it; otherwise the Bot is hired with the routine's text;
 * - a group member without a thread is hired for the group; one with a
 *   thread gets the framed message (or refuses, and the group shows it).
 */
import type { GroupDelivery, GroupMessenger } from "@trycua/cua/spaces/groups"
import { memoryStorage, routineTurnText, type Routine, type RoutineFiring, type RoutineRunner, type RoutineStorage } from "@trycua/cua/spaces/routines"
import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs"
import { SETTLED, type BotStore, type BotThread } from "./app.js"

/** A JSON file (written whole, then renamed), or memory when `path` is unset. */
export function jsonStorage(path: string | undefined): RoutineStorage {
  if (!path) return memoryStorage()
  return {
    load: () => (existsSync(path) ? readFileSync(path, "utf8") : undefined),
    save: (text) => {
      writeFileSync(`${path}.tmp`, text)
      renameSync(`${path}.tmp`, path)
    },
  }
}

/**
 * The reply text of a thread's latest turn: the agent's messages in it. The
 * SDK's event fold keeps install progress, tool calls, turn ends and the
 * echo of the prompt out of messages, so nothing is filtered here.
 */
export function replyText(t: BotThread): string {
  return t.outputOf(t.currentTurn).join("\n").trim()
}

/** Gives a Bot its routine turn in the shared Space. */
export class BotStoreRoutineRunner implements RoutineRunner {
  constructor(private readonly store: BotStore) {}

  async fire(routine: Routine): Promise<RoutineFiring> {
    const bot = this.store.bot(routine.botID)
    if (!bot) return { kind: "failed", reason: `no Bot ${routine.botID}` }
    const text = routineTurnText(routine)
    const t = this.store.threadFor(bot.id)
    if (t) await t.refresh()
    if (t && (t.state === "running" || t.state === "starting")) {
      return { kind: "refused", reason: `${bot.name} is mid-turn; the slot was skipped` }
    }
    try {
      if (t && t.acceptsMessage) {
        const d = await t.send(text)
        return d.accepted ? { kind: "started", runId: t.runId } : { kind: "refused", reason: d.reason }
      }
      const hired = await this.store.hire(bot, text)
      return { kind: "started", runId: hired.runId }
    } catch (e) {
      return { kind: "failed", reason: e instanceof Error ? e.message : String(e) }
    }
  }
}

/** Fans a group message out over the Bots' own threads. */
export class BotStoreGroupMessenger implements GroupMessenger {
  /** botID -> the run and turn the group last delivered, so a reply to any
   * other turn (the Bot's own thread) is never folded in. */
  private readonly asked = new Map<string, { runId: string; turn: number }>()

  constructor(private readonly store: BotStore) {}

  async deliver(text: string, botID: string): Promise<GroupDelivery> {
    const bot = this.store.bot(botID)
    if (!bot) return { botID, accepted: false, reason: `no Bot ${botID}` }
    const t = this.store.threadFor(botID)
    try {
      if (!t) {
        const hired = await this.store.hire(bot, text)
        this.asked.set(botID, { runId: hired.runId, turn: hired.currentTurn })
        return { botID, accepted: true, reason: "started for this group" }
      }
      await t.refresh()
      const d = await t.send(text)
      if (d.accepted) this.asked.set(botID, { runId: t.runId, turn: t.currentTurn })
      return { botID, accepted: d.accepted, reason: d.reason }
    } catch (e) {
      return { botID, accepted: false, reason: e instanceof Error ? e.message : String(e) }
    }
  }

  async latestReply(botID: string): Promise<string | undefined> {
    const t = this.store.threadFor(botID)
    const asked = this.asked.get(botID)
    // Only the turn the group asked for: a later turn (a routine, a direct
    // message) is the Bot's own thread, not the group's.
    if (!t || !asked || asked.runId !== t.runId || t.currentTurn !== asked.turn) return undefined
    await t.refresh()
    // Only a finished turn has a reply; a partial one would be folded in
    // twice as it grows.
    if (!SETTLED.has(t.state)) return undefined
    return replyText(t) || undefined
  }

  isWorking(botID: string): boolean {
    const s = this.store.threadFor(botID)?.state
    return s === "running" || s === "starting"
  }

  displayName(botID: string): string {
    return this.store.bot(botID)?.name ?? botID
  }
}

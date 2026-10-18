/**
 * `@trycua/cua/spaces/groups`: group chats, one human and two to six Bots in
 * one thread. No native library, no Node imports.
 *
 * A group chat is a fan-out over the members' own agent threads in the one
 * shared Space plus a merged, attributed transcript; it is not a new kind of
 * agent. The harness gives a Bot no way to know who else is in the room, so
 * every message is framed with the room ({@link frameGroupMessage}).
 * Delivery is per member and partial success is normal: a Bot mid-turn
 * refuses, and the refusal lands in the transcript as a marked line.
 *
 * ```ts
 * import { GroupChatStore } from "@trycua/cua/spaces/groups"
 *
 * const groups = new GroupChatStore(messenger)          // deliver / latestReply / isWorking / displayName
 * const chat = groups.create("Launch", ["ada", "bo"])   // throws outside 2..6
 * await groups.send("Where are we?", chat.id)
 * await groups.collectReplies(chat.id)                  // poll; never duplicates a line
 * ```
 */

/** The product bound: a group of one is a thread; every message fans out to every member. */
export const GROUP_MIN_BOTS = 2
export const GROUP_MAX_BOTS = 6

export type GroupChatErrorCode = "tooFewBots" | "tooManyBots" | "full" | "atFloor" | "alreadyAMember" | "notAMember" | "unknownChat"

/** A membership rule was broken. The message says which number. */
export class GroupChatError extends Error {
  constructor(
    readonly code: GroupChatErrorCode,
    message: string,
  ) {
    super(message)
    this.name = "GroupChatError"
  }

  static tooFewBots(have: number): GroupChatError {
    return new GroupChatError("tooFewBots", `A group chat needs at least ${GROUP_MIN_BOTS} bots; ${have} selected.`)
  }
  static tooManyBots(have: number): GroupChatError {
    return new GroupChatError("tooManyBots", `A group chat holds at most ${GROUP_MAX_BOTS} bots; ${have} selected.`)
  }
  static full(): GroupChatError {
    return new GroupChatError("full", `This group is full: ${GROUP_MAX_BOTS} bots is the limit. Remove one to add another.`)
  }
  static atFloor(): GroupChatError {
    return new GroupChatError("atFloor", `A group chat needs at least ${GROUP_MIN_BOTS} bots. Add one before removing this one.`)
  }
  static alreadyAMember(id: string): GroupChatError {
    return new GroupChatError("alreadyAMember", `${id} is already in this group.`)
  }
  static notAMember(id: string): GroupChatError {
    return new GroupChatError("notAMember", `${id} is not in this group.`)
  }
  static unknownChat(id: string): GroupChatError {
    return new GroupChatError("unknownChat", `No such group chat: ${id}`)
  }
}

/** Who said a line: the human, a Bot, or the group itself (joins, leaves, refusals). */
export type GroupSpeaker = { kind: "human" } | { kind: "bot"; botID: string } | { kind: "system" }

export interface GroupMessage {
  id: string
  speaker: GroupSpeaker
  text: string
  at: string
  /** This line records a message that did not reach its Bot. */
  undelivered: boolean
  reaction?: string
}

/** The result of fanning one message out to one member. */
export interface GroupDelivery {
  botID: string
  accepted: boolean
  reason: string
}

/** How a group reaches its Bots. `deliver` never throws: a refusal is a result. */
export interface GroupMessenger {
  deliver(text: string, botID: string): Promise<GroupDelivery>
  /** The Bot's most recent utterance, or `undefined`. */
  latestReply(botID: string): Promise<string | undefined>
  /** Whether the Bot is producing output right now (the typing row). */
  isWorking(botID: string): boolean
  displayName(botID: string): string
}

function newId(): string {
  return globalThis.crypto.randomUUID().toUpperCase()
}

function dedupe(ids: readonly string[]): string[] {
  const out: string[] = []
  for (const id of ids) if (!out.includes(id)) out.push(id)
  return out
}

/** Whether a Create button should be enabled for `members`. */
export function canCreateGroup(members: readonly string[]): boolean {
  const n = new Set(members).size
  return n >= GROUP_MIN_BOTS && n <= GROUP_MAX_BOTS
}

/** A group chat. The 2..6 bound holds for every instance: construction and membership changes throw. */
export class GroupChat {
  readonly id: string
  title: string
  readonly createdAt: string
  messages: GroupMessage[] = []
  private members: string[]

  constructor(title: string, members: readonly string[], options: { id?: string; createdAt?: Date } = {}) {
    const d = dedupe(members)
    if (d.length < GROUP_MIN_BOTS) throw GroupChatError.tooFewBots(d.length)
    if (d.length > GROUP_MAX_BOTS) throw GroupChatError.tooManyBots(d.length)
    this.id = options.id ?? newId()
    this.title = title
    this.createdAt = (options.createdAt ?? new Date()).toISOString()
    this.members = d
  }

  /** Bot ids in join order. The human is implicit. */
  get memberIDs(): readonly string[] {
    return this.members
  }
  get isFull(): boolean {
    return this.members.length >= GROUP_MAX_BOTS
  }
  get isAtFloor(): boolean {
    return this.members.length <= GROUP_MIN_BOTS
  }
  /** `4 of 6 bots`. */
  get membershipLabel(): string {
    return `${this.members.length} of ${GROUP_MAX_BOTS} bots`
  }
  get remainingSeats(): number {
    return Math.max(0, GROUP_MAX_BOTS - this.members.length)
  }

  add(botID: string): void {
    if (this.members.includes(botID)) throw GroupChatError.alreadyAMember(botID)
    if (this.isFull) throw GroupChatError.full()
    this.members.push(botID)
  }

  remove(botID: string): void {
    if (!this.members.includes(botID)) throw GroupChatError.notAMember(botID)
    if (this.isAtFloor) throw GroupChatError.atFloor()
    this.members = this.members.filter((m) => m !== botID)
  }
}

/**
 * The framing each member receives: the room, then the human's words on the
 * last line (so an agent that reads the last line still sees them).
 */
export function frameGroupMessage(text: string, botID: string, chat: GroupChat, name: (botID: string) => string): string {
  const others = chat.memberIDs
    .filter((m) => m !== botID)
    .map(name)
    .join(", ")
  return `[group:${chat.title}] You are in a group chat with the user and ${others}. Answer for your own area only and keep it to a few lines.\n${text}`
}

/** Group chats: membership, fan-out, and the merged transcript. */
export class GroupChatStore {
  chats: GroupChat[] = []
  /** The last membership complaint, cleared on the next success. */
  lastError: string | undefined
  /** Bots producing output right now, per chat. */
  readonly working = new Map<string, Set<string>>()
  /** chat -> bot -> the last reply already folded in, so a re-poll cannot duplicate it. */
  private readonly consumed = new Map<string, Map<string, string>>()
  private readonly listeners = new Set<() => void>()

  constructor(private messenger?: GroupMessenger) {}

  attach(messenger: GroupMessenger): void {
    this.messenger = messenger
  }

  subscribe(fn: () => void): () => void {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }

  private changed(): void {
    for (const l of this.listeners) l()
  }

  chat(id: string): GroupChat | undefined {
    return this.chats.find((c) => c.id === id)
  }

  displayName(botID: string): string {
    return this.messenger?.displayName(botID) ?? botID
  }

  private require(id: string): GroupChat {
    const c = this.chat(id)
    if (!c) throw GroupChatError.unknownChat(id)
    return c
  }

  private line(chat: GroupChat, speaker: GroupSpeaker, text: string, undelivered = false): void {
    chat.messages.push({ id: newId(), speaker, text, at: new Date().toISOString(), undelivered })
  }

  create(title: string, members: readonly string[]): GroupChat {
    const chat = new GroupChat(title, members)
    this.chats.push(chat)
    this.lastError = undefined
    this.changed()
    return chat
  }

  delete(id: string): void {
    this.chats = this.chats.filter((c) => c.id !== id)
    this.consumed.delete(id)
    this.working.delete(id)
    this.changed()
  }

  add(botID: string, chatID: string): void {
    this.mutate(chatID, (c) => c.add(botID), (c) => `${this.displayName(botID)} joined, ${c.membershipLabel}.`)
  }

  remove(botID: string, chatID: string): void {
    this.mutate(chatID, (c) => c.remove(botID), (c) => `${this.displayName(botID)} left, ${c.membershipLabel}.`)
  }

  private mutate(chatID: string, op: (c: GroupChat) => void, announce: (c: GroupChat) => string): void {
    const chat = this.require(chatID)
    try {
      op(chat)
      this.line(chat, { kind: "system" }, announce(chat))
      this.lastError = undefined
    } catch (e) {
      const text = e instanceof Error ? e.message : String(e)
      this.lastError = text
      this.line(chat, { kind: "system" }, text, true)
      this.changed()
      throw e
    }
    this.changed()
  }

  /** Sends one message to every member; returns each member's delivery. */
  async send(text: string, chatID: string): Promise<GroupDelivery[]> {
    const chat = this.chat(chatID)
    if (!chat) return []
    this.line(chat, { kind: "human" }, text)
    this.changed()
    const out: GroupDelivery[] = []
    for (const botID of chat.memberIDs) {
      const framed = frameGroupMessage(text, botID, chat, (id) => this.displayName(id))
      let d: GroupDelivery
      if (!this.messenger) d = { botID, accepted: false, reason: "not connected to a Space" }
      else {
        try {
          d = await this.messenger.deliver(framed, botID)
        } catch (e) {
          d = { botID, accepted: false, reason: e instanceof Error ? e.message : String(e) }
        }
      }
      out.push(d)
      if (!d.accepted) this.line(chat, { kind: "bot", botID }, `Did not receive that message: ${d.reason}`, true)
    }
    this.refreshWorking(chatID)
    this.changed()
    return out
  }

  /** Folds what members said since the last poll into the transcript, attributed. */
  async collectReplies(chatID: string): Promise<GroupMessage[]> {
    const chat = this.chat(chatID)
    if (!chat || !this.messenger) return []
    const added: GroupMessage[] = []
    for (const botID of chat.memberIDs) {
      const reply = (await this.messenger.latestReply(botID))?.trim()
      if (!reply) continue
      const seen = this.consumed.get(chatID) ?? new Map<string, string>()
      if (seen.get(botID) === reply) continue
      seen.set(botID, reply)
      this.consumed.set(chatID, seen)
      this.line(chat, { kind: "bot", botID }, reply)
      added.push(chat.messages[chat.messages.length - 1] as GroupMessage)
    }
    this.refreshWorking(chatID)
    if (added.length) this.changed()
    return added
  }

  /** Attaches a reaction to the last line. */
  react(emoji: string, chatID: string): void {
    const last = this.chat(chatID)?.messages.at(-1)
    if (!last) return
    last.reaction = emoji
    this.changed()
  }

  refreshWorking(chatID: string): void {
    const chat = this.chat(chatID)
    if (!chat || !this.messenger) return
    const m = this.messenger
    this.working.set(chatID, new Set(chat.memberIDs.filter((b) => m.isWorking(b))))
  }

  workingBots(chatID: string): string[] {
    return [...(this.working.get(chatID) ?? [])].sort()
  }
}

/**
 * The presence records of the native binding (`../native`), restated
 * structurally so the browser-safe presence model type-checks without the
 * native module (whose runtime loader imports `node:*`). The native records
 * are assignable to these; `presenceTypes.check.ts` keeps them in step.
 */

/** A participant (`PresenceParticipant`). */
export type PresenceParticipant = {
  participantId: string
  principalId: string
  displayName: string
  color: string
  kind: string
}

/** A cursor (`PresenceCursor`). */
export type PresenceCursor = {
  displayId: string
  windowId?: string
  x: number
  y: number
  visible: boolean
  pressed: boolean
  shape: string
  shapeSource: string
  atMs: number
  receivedMs: number
}

/** One presence event (`PresenceEvent`). */
export type PresenceEvent = {
  kind: string
  participant?: PresenceParticipant
  participantId?: string
  cursor?: PresenceCursor
  shape?: string
  shapeSource?: string
  reason?: string
  participantIds?: Array<string>
}

/** A roster member at join (`PresenceMember`). */
export type PresenceMember = {
  participant: PresenceParticipant
  cursor?: PresenceCursor
}

/** The part of a presence session `waitForPresence` reads. */
export interface PresenceEventSource {
  nextEvent(timeoutMs: bigint | undefined): Promise<PresenceEvent | undefined>
}

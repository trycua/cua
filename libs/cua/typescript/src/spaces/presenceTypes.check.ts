// Compile-time guard: the native binding's presence records stay assignable
// to the structural types the browser-safe model uses (./presenceTypes).
import type * as Native from "../native/index.js"
import type * as Model from "./presenceTypes.js"

type Holds<T extends true> = T
type Assignable<A, B> = [A] extends [B] ? true : false

export type PresenceTypesInStep = [
  Holds<Assignable<Native.PresenceParticipant, Model.PresenceParticipant>>,
  Holds<Assignable<Native.PresenceCursor, Model.PresenceCursor>>,
  Holds<Assignable<Native.PresenceEvent, Model.PresenceEvent>>,
  Holds<Assignable<Native.PresenceMember, Model.PresenceMember>>,
  Holds<Assignable<Native.SpacePresenceLike, Model.PresenceEventSource>>,
]

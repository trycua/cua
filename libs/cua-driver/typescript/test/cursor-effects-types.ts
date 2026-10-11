// Legacy callers remain source-compatible without casting to the new enum.
import { CursorEffectSetting, CursorMotionEffects, defaultCursorMotionParams, planCursorMove, type CursorMoveRequest } from "../src/index.js"
const effects: CursorMotionEffects = { trail: true, glow: false, magnet: undefined, ripple: CursorEffectSetting.Default }
const constructed = CursorMotionEffects.create({ trail: true, squish: false })
const params = defaultCursorMotionParams()
params.effects = effects
params.effects = constructed
export function legacyCall(request: CursorMoveRequest) { return planCursorMove(params, request) }
// @ts-expect-error Arbitrary strings are not SDK enum values.
const invalid: CursorMotionEffects = { trail: "true" }
void invalid

// @ts-expect-error Use undefined for omission or the Default enum for reset.
const invalidNull: CursorMotionEffects = { trail: null }
void invalidNull

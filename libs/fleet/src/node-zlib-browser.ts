const unsupportedMessage = "gzip commands are unsupported in the browser"

export const constants = {
  Z_BEST_COMPRESSION: 9,
  Z_BEST_SPEED: 1,
  Z_DEFAULT_COMPRESSION: -1,
} as const

function unsupported(_input: Uint8Array, _options?: unknown): never {
  throw new Error(unsupportedMessage)
}

export const gzipSync = unsupported
export const gunzipSync = unsupported

// uniffi enum errors (e.g. the fleet SDK's SdkError) stringify as just the
// variant name — "SdkError.Status" — while the useful details live on the
// error's `inner` field: Status carries the backend response body verbatim,
// and most other variants carry a `reason`. The shapes are matched
// structurally so this module stays independent of the generated bindings.
interface UniffiErrorLike {
  tag?: unknown
  inner?: unknown
}

// Backend errors arrive as JSON like {"error": "..."} (the auth middleware)
// or {"message": "..."} (the K8s API); anything else is piped through as-is.
function backendErrorDetail(body: string): string | undefined {
  const trimmed = body.trim()
  if (!trimmed) return undefined
  try {
    const parsed = JSON.parse(trimmed) as { error?: unknown; message?: unknown }
    if (typeof parsed?.error === "string" && parsed.error) return parsed.error
    if (typeof parsed?.message === "string" && parsed.message) {
      return parsed.message
    }
  } catch {
    // Not JSON — surface the raw body below.
  }
  return trimmed
}

function uniffiErrorDetail(error: UniffiErrorLike): string | undefined {
  if (typeof error.tag !== "string") return undefined
  const inner = error.inner
  if (!inner || typeof inner !== "object") return undefined
  const fields = inner as { status?: unknown; body?: unknown; reason?: unknown }
  // Status, PoolAccessDenied, … — every variant carrying an HTTP response.
  if (typeof fields.status === "number") {
    const detail = backendErrorDetail(
      typeof fields.body === "string" ? fields.body : "",
    )
    return detail ?? `Request failed with HTTP status ${fields.status}.`
  }
  if (typeof fields.reason === "string" && fields.reason) return fields.reason
  return undefined
}

export function errorMessage(error: unknown): string {
  if (error && typeof error === "object") {
    const detail = uniffiErrorDetail(error as UniffiErrorLike)
    if (detail) return detail
  }
  if (error instanceof Error && error.message) return error.message
  if (typeof error === "string" && error) return error
  if (error == null) return "Unknown error"
  try {
    return String(error)
  } catch {
    return "Unknown error"
  }
}

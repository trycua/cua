export function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message) return error.message
  if (typeof error === "string" && error) return error
  if (error == null) return "Unknown error"
  try {
    return String(error)
  } catch {
    return "Unknown error"
  }
}

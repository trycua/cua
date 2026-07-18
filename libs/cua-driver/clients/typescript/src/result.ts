export interface ImageContent {
  mimeType: string
  dataBase64: string
}

export interface ToolResult {
  text: string
  images: ImageContent[]
  structured: Record<string, unknown> | null
  isError: boolean
  errorCode: string | null
  verified: boolean | null
  degraded: boolean
  raw: Record<string, unknown>
}

function record(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}

export function normalizeToolResult(value: unknown): ToolResult {
  const raw = record(value)
  if (!raw) throw new TypeError("MCP tool result must be an object")
  const content = Array.isArray(raw.content) ? raw.content : []
  const text: string[] = []
  const images: ImageContent[] = []
  for (const itemValue of content) {
    const item = record(itemValue)
    if (!item) continue
    if (item.type === "text" && typeof item.text === "string") text.push(item.text)
    if (
      item.type === "image" &&
      typeof item.mimeType === "string" &&
      typeof item.data === "string"
    ) {
      images.push({ mimeType: item.mimeType, dataBase64: item.data })
    }
  }
  const structured = record(raw.structuredContent)
  const refusal = record(structured?.refusal)
  const directCode = structured?.code
  const refusalCode = refusal?.code
  const errorCode =
    typeof directCode === "string"
      ? directCode
      : typeof refusalCode === "string"
        ? refusalCode
        : null
  return {
    text: text.join("\n"),
    images,
    structured,
    isError: raw.isError === true,
    errorCode,
    verified: typeof structured?.verified === "boolean" ? structured.verified : null,
    degraded: structured?.degraded === true,
    raw,
  }
}

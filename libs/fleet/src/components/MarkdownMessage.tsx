import DOMPurify, { type Config } from "dompurify"
import { marked } from "marked"
import { useMemo } from "react"

const SANITIZE_OPTIONS = {
  ALLOW_DATA_ATTR: false,
  FORBID_ATTR: ["id", "name", "style"],
  FORBID_TAGS: ["audio", "embed", "form", "iframe", "img", "input", "object", "picture", "source", "style", "svg", "video"],
  SANITIZE_NAMED_PROPS: true,
  USE_PROFILES: { html: true },
} satisfies Config

export function MarkdownMessage({ content }: { content: string }) {
  const html = useMemo(() => {
    const parsed = marked.parse(content, { async: false, breaks: true, gfm: true })
    return DOMPurify.sanitize(parsed, SANITIZE_OPTIONS)
  }, [content])

  // DOMPurify is the security boundary for this intentional HTML sink.
  // biome-ignore lint/security/noDangerouslySetInnerHtml: marked output is sanitized immediately before rendering.
  return <div className="agent-chat-markdown" dangerouslySetInnerHTML={{ __html: html }} />
}

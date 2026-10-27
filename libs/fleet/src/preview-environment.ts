// Review previews are served from `cyclops-cs-pr-<n>.<suffix>`. The suffix is
// supplied at build time through VITE_CUA_REVIEW_PREVIEW_HOST_SUFFIX.
const DEFAULT_REVIEW_PREVIEW_HOST_SUFFIX = "example.ts.net"

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
}

export function isReviewPreviewHostname(
  hostname: string,
  suffix: string = import.meta.env?.VITE_CUA_REVIEW_PREVIEW_HOST_SUFFIX ||
    DEFAULT_REVIEW_PREVIEW_HOST_SUFFIX,
): boolean {
  return new RegExp(`^cyclops-cs-pr-\\d+\\.${escapeRegExp(suffix)}$`).test(hostname)
}

export function isReviewPreviewEnvironment(): boolean {
  return (
    import.meta.env.VITE_CUA_REVIEW_VISUAL_PREVIEW === "true" &&
    isReviewPreviewHostname(window.location.hostname)
  )
}

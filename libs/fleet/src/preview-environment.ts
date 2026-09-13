const REVIEW_PREVIEW_HOSTNAME = /^cyclops-cs-pr-\d+\.tail204509\.ts\.net$/

export function isReviewPreviewHostname(hostname: string): boolean {
  return REVIEW_PREVIEW_HOSTNAME.test(hostname)
}

export function isReviewPreviewEnvironment(): boolean {
  return (
    import.meta.env.VITE_CUA_REVIEW_VISUAL_PREVIEW === "true" &&
    isReviewPreviewHostname(window.location.hostname)
  )
}

export function formatTtl(seconds: number | undefined): string {
  if (seconds === undefined) return "None"
  if (seconds === 0) return "0s"

  const units = [
    { label: "d", seconds: 86_400 },
    { label: "h", seconds: 3_600 },
    { label: "m", seconds: 60 },
    { label: "s", seconds: 1 },
  ]
  const parts: string[] = []
  let remaining = seconds

  for (const unit of units) {
    const value = Math.floor(remaining / unit.seconds)
    if (value > 0) {
      parts.push(`${value}${unit.label}`)
      remaining %= unit.seconds
    }
  }

  return parts.join(" ")
}

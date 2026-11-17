import { requiresCuaApp } from "@trycua/cua/teleport"

/**
 * A person-facing message for a server error, with the raw text kept as the
 * detail. The spacesd answers a wrong or missing token with gRPC
 * `unauthenticated`; say that in words.
 */
export function friendlyError(raw: string): {
  message: string
  detail: string
  /** A button to show with the message (the Install Cua affordance). */
  link?: { label: string; url: string }
} {
  const text = raw.replace(/^Error:\s*/, "")
  // Session teleport through the daemon needs the Keyvault, which needs the
  // Cua app; say what to do instead of showing the refusal code.
  const install = requiresCuaApp(text)
  if (install)
    return { message: `${install.title}. ${install.message}`, detail: text, link: { label: install.actionLabel, url: install.url } }
  if (/unauthenticated|invalid bearer|missing or invalid .*token/i.test(text))
    return { message: "The Space rejected the token. Check the token and try again.", detail: text }
  if (/connection refused|tcp connect error|failed to connect|dns error|timed out/i.test(text))
    return { message: "Could not reach the Space. Check the address and that it is running.", detail: text }
  return { message: text, detail: "" }
}

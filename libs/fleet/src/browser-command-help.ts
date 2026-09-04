export interface BrowserCommandHelp {
  summary: string
  usage: string
  arguments?: string[]
  output: string
  presentation: string[]
  safety: string
  examples?: string[]
}

export function isHelpRequest(args: string[]): boolean {
  return args.length === 1 && (args[0] === "-h" || args[0] === "--help")
}

function appendSection(lines: string[], heading: string, values: string[]): void {
  lines.push(heading, ...values.map(value => `  ${value}`), "")
}

export function renderCommandHelp(name: string, help: BrowserCommandHelp): string {
  const lines = [help.summary, ""]
  appendSection(lines, "Usage:", [help.usage || name])
  if (help.arguments?.length) appendSection(lines, "Arguments:", help.arguments)
  appendSection(lines, "Output:", [help.output])
  appendSection(lines, "Present to the user:", help.presentation)
  appendSection(lines, "Safety:", [help.safety])
  if (help.examples?.length) appendSection(lines, "Examples:", help.examples)
  return lines.join("\n")
}

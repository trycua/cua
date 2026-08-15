import { defineCommand, type Command } from "just-bash/browser"

export type BrowserSdkMethod = (...args: never[]) => Promise<unknown>

export interface BrowserSdk {
  listNamespaces: BrowserSdkMethod
  listPools: BrowserSdkMethod
  getPool: BrowserSdkMethod
  createPool: BrowserSdkMethod
  updatePoolServices: BrowserSdkMethod
  deletePool: BrowserSdkMethod
  listClaims: BrowserSdkMethod
  createClaim: BrowserSdkMethod
  getClaim: BrowserSdkMethod
  deleteClaim: BrowserSdkMethod
  listUserKeys: BrowserSdkMethod
  createUserKey: BrowserSdkMethod
  deleteUserKey: BrowserSdkMethod
}

export const BROWSER_SDK_COMMAND_NAMES = [
  "listNamespaces",
  "listPools",
  "getPool",
  "createPool",
  "updatePoolServices",
  "deletePool",
  "listClaims",
  "createClaim",
  "getClaim",
  "deleteClaim",
  "listUserKeys",
  "createUserKey",
  "deleteUserKey",
] as const satisfies readonly (keyof BrowserSdk)[]

function decodeStdin(stdin: unknown): string {
  const bytes = Uint8Array.from(stdin as string, character => character.charCodeAt(0))
  return new TextDecoder().decode(bytes)
}

function parseArguments(args: string[], stdin: string): unknown[] {
  if (args.length > 1) throw new Error("expected one JSON array argument")
  const input = args[0] ?? stdin.trim()
  if (!input) return []

  let parsed: unknown
  try {
    parsed = JSON.parse(input)
  } catch {
    throw new Error("arguments must be a JSON array")
  }
  if (!Array.isArray(parsed)) throw new Error("arguments must be a JSON array")
  return parsed
}

function errorMessage(error: unknown): string {
  return error instanceof Error && error.message ? error.message : "SDK command failed"
}

export function createBrowserSdkCommands(sdk: BrowserSdk): Command[] {
  return BROWSER_SDK_COMMAND_NAMES.map(name =>
    defineCommand(name, async (args, context) => {
      try {
        const methodArguments = parseArguments(args, decodeStdin(context.stdin))
        const method = sdk[name] as (...args: unknown[]) => Promise<unknown>
        const result = await method(...methodArguments)
        return {
          stdout: result === undefined ? "" : `${JSON.stringify(result)}\n`,
          stderr: "",
          exitCode: 0,
        }
      } catch (error) {
        return { stdout: "", stderr: `${errorMessage(error)}\n`, exitCode: 1 }
      }
    }),
  )
}

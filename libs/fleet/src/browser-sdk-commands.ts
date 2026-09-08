import { defineCommand, type Command } from "just-bash/browser"
import {
  isHelpRequest,
  renderCommandHelp,
  type BrowserCommandHelp,
} from "./browser-command-help.js"
import type { SensitiveOutputBuffer, UserApiKeyCredentials } from "./sensitive-output.js"

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

type BrowserSdkCommandName = (typeof BROWSER_SDK_COMMAND_NAMES)[number]

const SDK_COMMAND_HELP: Record<BrowserSdkCommandName, BrowserCommandHelp> = {
  listNamespaces: {
    summary: "List namespaces visible to the current user.",
    usage: "listNamespaces",
    output: "A JSON array of namespace objects.",
    presentation: [
      "Use a short bullet list unless the user asks for raw JSON.",
      "Do not include namespaces in pool listings; list them only when namespaces are the requested subject.",
    ],
    safety: "Read-only.",
    examples: ["listNamespaces | jq -r '.[].name'"],
  },
  listPools: {
    summary: "List all pools visible to the current user.",
    usage: "listPools",
    output: "A JSON array with name, namespace, replicas, availableCount, and status for each pool.",
    presentation: [
      "Use a concise Markdown table with exactly these columns: Pool | Replicas | Available | Status.",
      "Omit Namespace; it is implementation detail and redundant in this listing.",
      "Render each pool name as a Markdown link to /pools/<URL-encoded namespace>/<URL-encoded pool name>.",
      "After the table, summarize the total pool count and total available sandboxes in one sentence.",
    ],
    safety: "Read-only.",
    examples: ["listPools", "listPools | jq '[length, (map(.availableCount) | add // 0)]'"],
  },
  getPool: {
    summary: "Get the full configuration and live status of one pool.",
    usage: "getPool '[\"namespace\",\"name\"]'",
    arguments: ["namespace: Pool namespace.", "name: Pool name."],
    output: "A JSON pool object including capacity, image, resources, services, probes, and autoscaling.",
    presentation: [
      "Use the linked pool name as the heading: [name](/pools/<URL-encoded namespace>/<URL-encoded name>), followed by compact status and configuration sections.",
      "Do not repeat the namespace unless it disambiguates the result or the user explicitly asks for it.",
      "Omit empty optional sections rather than displaying null or empty values.",
    ],
    safety: "Read-only.",
    examples: ["getPool '[\"team\",\"browser-pool\"]' | jq"],
  },
  createPool: {
    summary: "Create a pool and its sandbox template.",
    usage: "createPool '[\"name\",poolTemplateConfig]'",
    arguments: [
      "name: New pool name; the namespace is created with the same name.",
      "poolTemplateConfig: Object containing cpu, ram, ociImage, replicas, and optional firmware, services, probes, or autoscaling.",
    ],
    output: "The created pool object.",
    presentation: [
      "Before execution, state the exact pool name and requested capacity.",
      "After success, link the pool name to /pools/<URL-encoded namespace>/<URL-encoded pool name> and summarize its key configuration.",
      "Never display raw internal objects unless requested.",
    ],
    safety: "Mutating. Confirm all required values are supplied and use exactly the user's requested configuration.",
    examples: ["createPool '[\"demo\",{\"cpu\":2,\"ram\":\"4Gi\",\"ociImage\":\"ghcr.io/trycua/cua:latest\",\"replicas\":1}]'"],
  },
  updatePoolServices: {
    summary: "Replace the service definitions exposed by a pool.",
    usage: "updatePoolServices '[\"namespace\",\"name\",services]'",
    arguments: ["namespace: Pool namespace.", "name: Pool name.", "services: Complete replacement array of service definitions."],
    output: "No stdout on success.",
    presentation: [
      "Before execution, state that the complete service list will be replaced.",
      "After success, link the pool name to /pools/<URL-encoded namespace>/<URL-encoded name> and list the resulting services concisely.",
    ],
    safety: "Mutating. This replaces, rather than merges, the pool's service list.",
    examples: ["updatePoolServices '[\"team\",\"browser-pool\",[{\"name\":\"desktop\",\"targetPort\":6901,\"protocol\":\"TCP\"}]]'"],
  },
  deletePool: {
    summary: "Delete a pool and its associated namespace resources.",
    usage: "deletePool '[\"namespace\",\"name\"]'",
    arguments: ["namespace: Pool namespace.", "name: Pool name."],
    output: "No stdout on success.",
    presentation: [
      "Before execution, explicitly name the pool being deleted and describe the destructive effect.",
      "After success, confirm deletion in one sentence without inventing deleted resource counts.",
    ],
    safety: "Destructive. Run only when the user clearly requested deletion of this exact pool.",
    examples: ["deletePool '[\"team\",\"browser-pool\"]'"],
  },
  listClaims: {
    summary: "List sandbox claims in one namespace.",
    usage: "listClaims '[\"namespace\"]'",
    arguments: ["namespace: Namespace whose claims should be listed."],
    output: "A JSON array with claim name, pool, phase, sandbox details, and creation time.",
    presentation: [
      "Use a concise Markdown table with Claim, Pool, Phase, Sandbox, and Created columns.",
      "Omit Namespace because the command already scopes the result to one namespace.",
      "Link claim names to /pools/<URL-encoded namespace>/<URL-encoded pool name>/claims/<URL-encoded claim name> when the pool name is available.",
    ],
    safety: "Read-only.",
    examples: ["listClaims '[\"team\"]' | jq"],
  },
  createClaim: {
    summary: "Allocate a sandbox claim from a pool.",
    usage: "createClaim '[\"namespace\",\"poolName\"]'",
    arguments: ["namespace: Pool namespace.", "poolName: Pool that should supply the sandbox."],
    output: "The created claim object.",
    presentation: [
      "Before execution, state which linked pool will allocate the sandbox.",
      "After success, report the claim name and phase, and link the claim to its detail page.",
    ],
    safety: "Mutating. Creates a claim that consumes pool capacity.",
    examples: ["createClaim '[\"team\",\"browser-pool\"]'"],
  },
  getClaim: {
    summary: "Get the live state of one sandbox claim.",
    usage: "getClaim '[\"namespace\",\"name\"]'",
    arguments: ["namespace: Claim namespace.", "name: Claim name."],
    output: "A JSON claim object with pool, phase, sandbox endpoint fields, and creation time.",
    presentation: [
      "Show the claim name, linked pool, phase, sandbox name, service endpoint, and creation time.",
      "Omit fields that are absent while the claim is pending.",
    ],
    safety: "Read-only.",
    examples: ["getClaim '[\"team\",\"claim-one\"]' | jq"],
  },
  deleteClaim: {
    summary: "Delete a claim and return its sandbox capacity to the pool.",
    usage: "deleteClaim '[\"namespace\",\"name\"]'",
    arguments: ["namespace: Claim namespace.", "name: Claim name."],
    output: "No stdout on success.",
    presentation: [
      "Before execution, explicitly name the claim being released.",
      "After success, confirm that the claim was deleted and its sandbox capacity was returned to the pool.",
    ],
    safety: "Destructive. Run only when the user clearly requested release of this exact claim.",
    examples: ["deleteClaim '[\"team\",\"claim-one\"]'"],
  },
  listUserKeys: {
    summary: "List API keys owned by the current user without revealing secrets.",
    usage: "listUserKeys",
    output: "A JSON array containing key IDs, client IDs, names, and scopes.",
    presentation: [
      "Use a concise Markdown table with Name, Client ID, and Scopes.",
      "Do not display internal key IDs unless the user needs one for deletion.",
    ],
    safety: "Read-only. Existing client secrets are never returned.",
    examples: ["listUserKeys | jq"],
  },
  createUserKey: {
    summary: "Create a user API key whose secret is shown once.",
    usage: "createUserKey '[\"name\",[\"scope\"]]'",
    arguments: ["name: Human-readable key name.", "scopes: Optional array of scopes."],
    output: "A safe confirmation. The credential values are displayed directly to the user in the chat UI and are unavailable to the model.",
    presentation: [
      "Warn before execution that a new credential will be created.",
      "After success, state that the credentials are shown directly in the chat and must be copied before the page is reloaded.",
      "Do not ask the user to paste the client secret back into chat.",
    ],
    safety: "Sensitive mutation. Create only with the user's requested name and scopes; credential values never enter model context or server-side chat history.",
    examples: ["createUserKey '[\"automation\",[\"fleets:read\"]]'"],
  },
  deleteUserKey: {
    summary: "Revoke one user API key by ID.",
    usage: "deleteUserKey '[\"id\"]'",
    arguments: ["id: Internal key ID, obtainable from listUserKeys."],
    output: "No stdout on success.",
    presentation: [
      "Before execution, identify the human-readable key name and client ID when available, not only its internal ID.",
      "After success, confirm revocation in one sentence.",
    ],
    safety: "Destructive credential operation. Verify the exact key before revoking it.",
    examples: ["deleteUserKey '[\"key-id\"]'"],
  },
}

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

function isUserApiKeyCredentials(value: unknown): value is UserApiKeyCredentials {
  if (!value || typeof value !== "object") return false
  const credentials = value as Partial<UserApiKeyCredentials>
  return (
    typeof credentials.clientId === "string" &&
    typeof credentials.clientSecret === "string" &&
    typeof credentials.tokenUrl === "string" &&
    typeof credentials.name === "string" &&
    Array.isArray(credentials.scope) &&
    credentials.scope.every(scope => typeof scope === "string")
  )
}

export function createBrowserSdkCommands(
  sdk: BrowserSdk,
  sensitiveOutputs?: SensitiveOutputBuffer,
): Command[] {
  return BROWSER_SDK_COMMAND_NAMES.map(name =>
    defineCommand(name, async (args, context) => {
      if (isHelpRequest(args)) {
        return { stdout: renderCommandHelp(name, SDK_COMMAND_HELP[name]), stderr: "", exitCode: 0 }
      }
      try {
        const methodArguments = parseArguments(args, decodeStdin(context.stdin))
        const method = sdk[name] as (...args: unknown[]) => Promise<unknown>
        const result = await method(...methodArguments)
        if (name === "createUserKey") {
          if (!isUserApiKeyCredentials(result)) {
            throw new Error("createUserKey returned invalid credentials")
          }
          sensitiveOutputs?.push({ kind: "user_api_key", value: result })
          return {
            stdout: '{"status":"created","credentials":"shown directly to the user"}\n',
            stderr: "",
            exitCode: 0,
          }
        }
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

import {
  CreateUserApiKeyRequestBuilder,
  type CreateUserApiKeyRequest,
  type NewUserApiKey,
  type UserApiKey,
} from "../../sdk-bindings/ts-uniffi-browser/ts/index.web"
import { withClient } from "../auth/cyclops-client"
import { isLocalVisualPreview } from "../local-visual-preview"

export type { NewUserApiKey, UserApiKey } from "../../sdk-bindings/ts-uniffi-browser/ts/index.web"

let localVisualPreviewKeys: UserApiKey[] = [
  {
    id: "preview-key-1",
    clientId: "cua_preview_automation",
    name: "Preview automation",
    scope: ["preview"],
  },
]

export function buildUserApiKeyRequest(
  name: string,
  scope?: string[],
): CreateUserApiKeyRequest {
  return new CreateUserApiKeyRequestBuilder()
    .name(name)
    .scope(scope ?? [])
    .build()
}

export async function listUserKeys(): Promise<UserApiKey[]> {
  if (isLocalVisualPreview()) return [...localVisualPreviewKeys]
  return withClient(client => client.listUserApiKeys())
}

export async function createUserKey(
  name: string,
  scope?: string[],
): Promise<NewUserApiKey> {
  if (isLocalVisualPreview()) {
    const key: UserApiKey = {
      id: `preview-key-${localVisualPreviewKeys.length + 1}`,
      clientId: `cua_preview_${name.toLowerCase().replace(/[^a-z0-9]+/g, "_")}`,
      name,
      scope: scope ?? [],
    }
    localVisualPreviewKeys = [...localVisualPreviewKeys, key]
    return {
      clientId: key.clientId,
      clientSecret: "cua_preview_secret_shown_once",
      tokenUrl: "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token",
      name: key.name,
      scope: key.scope,
    }
  }
  return withClient(client => client.createUserApiKey(buildUserApiKeyRequest(name, scope)))
}

export async function deleteUserKey(id: string): Promise<void> {
  if (isLocalVisualPreview()) {
    localVisualPreviewKeys = localVisualPreviewKeys.filter(key => key.id !== id)
    return
  }
  await withClient(client => client.deleteUserApiKey(id))
}

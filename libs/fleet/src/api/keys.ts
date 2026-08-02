// Thin facade over the generated cyclops-cs-backend client.
//
// Existing components import { ApiKey, NewApiKey, keysApi } from "./keys"
// — keep that surface stable while delegating the actual HTTP calls to
// the swagger-generated client (src/api/generated/cyclops-cs-backend.ts).
// To regenerate the client after backend changes:  pnpm gen:api

import { apiClient } from "./client"
import type {
  HandlersCreateKeyResponse,
  KeycloakKeyClient,
} from "./generated/cyclops-cs-backend"

export type ApiKey = Required<
  Pick<KeycloakKeyClient, "id" | "client_id" | "name" | "namespace" | "owner_sub">
>
export type NewApiKey = Required<
  Pick<
    HandlersCreateKeyResponse,
    "client_id" | "client_secret" | "token_url" | "name" | "namespace"
  >
>

export const keysApi = {
  list: async (): Promise<{ keys: ApiKey[] }> => {
    const res = await apiClient.keys.keysList()
    return { keys: (res.data.keys ?? []) as ApiKey[] }
  },
  create: async (name: string, namespace: string): Promise<NewApiKey> => {
    const res = await apiClient.keys.keysCreate({ name, namespace })
    return res.data as NewApiKey
  },
  remove: async (id: string): Promise<void> => {
    await apiClient.keys.keysDelete(id)
  },
}

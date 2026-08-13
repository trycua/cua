import {
  CreateUserApiKeyRequestBuilder,
  type CreateUserApiKeyRequest,
  type NewUserApiKey,
  type UserApiKey,
} from "./generated"
import { withClient } from "./client"

export type { NewUserApiKey, UserApiKey } from "./generated"

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
  return withClient(client => client.listUserApiKeys())
}

export async function createUserKey(
  name: string,
  scope?: string[],
): Promise<NewUserApiKey> {
  return withClient(client => client.createUserApiKey(buildUserApiKeyRequest(name, scope)))
}

export async function deleteUserKey(id: string): Promise<void> {
  await withClient(client => client.deleteUserApiKey(id))
}

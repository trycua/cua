import type { NewUserApiKey, UserApiKey } from "./generated"
import { withClient } from "./client"

export type { NewUserApiKey, UserApiKey } from "./generated"

export async function listUserKeys(): Promise<UserApiKey[]> {
  return withClient(client => client.listUserApiKeys())
}

export async function createUserKey(
  name: string,
  scope?: string[],
): Promise<NewUserApiKey> {
  return withClient(client =>
    client.createUserApiKey({ name, scope: scope ?? [] }),
  )
}

export async function deleteUserKey(id: string): Promise<void> {
  await withClient(client => client.deleteUserApiKey(id))
}

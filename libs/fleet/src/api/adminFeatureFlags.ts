import { getToken } from "../auth/keycloak"

export type FeatureFlagValueType = "boolean" | "number" | "string" | "json"
export type FeatureFlagOwnership = "terraform" | "ad_hoc" | "external"
export type JSONValue =
  | null
  | boolean
  | number
  | string
  | JSONValue[]
  | { [key: string]: JSONValue }

export interface AdminFeatureFlag {
  key: string
  path: string
  value_type: FeatureFlagValueType
  value: JSONValue
  raw_value: string
  ownership: FeatureFlagOwnership
  managed_by?: string
  deletable: boolean
  version: number
  modified_at: string
  description?: string
  tags: Record<string, string>
}

export interface CreateAdminFeatureFlag {
  key: string
  value_type: FeatureFlagValueType
  value: JSONValue
  description?: string
}

export interface UpdateAdminFeatureFlag {
  value_type: FeatureFlagValueType
  value: JSONValue
  expected_version: number
}

export class AdminFeatureFlagError extends Error {
  status: number
  code: string
  current?: AdminFeatureFlag

  constructor(
    status: number,
    code: string,
    message: string,
    current?: AdminFeatureFlag,
  ) {
    super(message)
    this.name = "AdminFeatureFlagError"
    this.status = status
    this.code = code
    this.current = current
  }
}

interface AdminFeatureFlagErrorResponse {
  code?: string
  message?: string
  current?: AdminFeatureFlag
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const token = await getToken()
  const response = await fetch(path, {
    ...init,
    headers: {
      Authorization: token ? `Bearer ${token}` : "",
      "Content-Type": "application/json",
      ...init?.headers,
    },
  })
  if (!response.ok) {
    const body = (await response.json().catch(() => ({}))) as AdminFeatureFlagErrorResponse
    throw new AdminFeatureFlagError(
      response.status,
      body.code ?? "request_failed",
      body.message ?? `feature flag request failed: ${response.status}`,
      body.current,
    )
  }
  if (response.status === 204) return undefined as T
  return response.json() as Promise<T>
}

export const adminFeatureFlagsApi = {
  list: () => request<AdminFeatureFlag[]>("/api/admin/feature-flags"),
  create: (input: CreateAdminFeatureFlag) =>
    request<AdminFeatureFlag>("/api/admin/feature-flags", {
      method: "POST",
      body: JSON.stringify(input),
    }),
  update: (key: string, input: UpdateAdminFeatureFlag) =>
    request<AdminFeatureFlag>(`/api/admin/feature-flags/${encodeURIComponent(key)}`, {
      method: "PUT",
      body: JSON.stringify(input),
    }),
  remove: (key: string, expectedVersion: number) =>
    request<void>(`/api/admin/feature-flags/${encodeURIComponent(key)}`, {
      method: "DELETE",
      body: JSON.stringify({ expected_version: expectedVersion }),
    }),
}

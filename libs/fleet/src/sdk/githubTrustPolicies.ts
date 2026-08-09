import { getToken } from "../auth/keycloak"

export interface Namespace {
  name: string
  status: string
  createdAt: string
  labels: Record<string, string> | null
}

export interface GitHubTrustPolicy {
  id: string
  name: string
  repository: string
  allowed_namespaces: string[]
  enabled: boolean
  created_at: string
  updated_at: string
}

export interface GitHubTrustPolicyListResponse {
  policies: GitHubTrustPolicy[]
  oidc: {
    issuer: string
    audience: string
  }
}

export interface GitHubTrustPolicyInput {
  name: string
  repository: string
  allowed_namespaces: string[]
  enabled: boolean
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const token = await getToken()
  const response = await fetch(path, {
    ...init,
    headers: {
      "Content-Type": "application/json",
      Authorization: token ? `Bearer ${token}` : "",
      ...init?.headers,
    },
  })
  if (!response.ok) {
    const body = await response.text()
    throw new Error(body || `request failed: ${response.status}`)
  }
  if (response.status === 204) return undefined as T
  return response.json() as Promise<T>
}

export const githubTrustPoliciesApi = {
  list: () => request<GitHubTrustPolicyListResponse>("/api/github-trust-policies"),
  create: (input: GitHubTrustPolicyInput) =>
    request<GitHubTrustPolicy>("/api/github-trust-policies", {
      method: "POST",
      body: JSON.stringify(input),
    }),
  update: (id: string, input: Partial<GitHubTrustPolicyInput>) =>
    request<GitHubTrustPolicy>(`/api/github-trust-policies/${encodeURIComponent(id)}`, {
      method: "PATCH",
      body: JSON.stringify(input),
    }),
  remove: (id: string) =>
    request<void>(`/api/github-trust-policies/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),
}

export const namespacesApi = {
  list: () => request<Namespace[]>("/api/namespaces"),
}

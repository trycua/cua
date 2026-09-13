import { getToken } from "../auth/keycloak"
import { isLocalVisualPreview } from "../local-visual-preview"

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

let localVisualPreviewPolicies: GitHubTrustPolicy[] = [
  {
    id: "preview-policy-1",
    name: "Cloud CI",
    repository: "trycua/cloud",
    allowed_namespaces: ["preview"],
    enabled: true,
    created_at: "2026-08-16T20:20:00.000Z",
    updated_at: "2026-08-16T20:20:00.000Z",
  },
]

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
  list: async () => {
    if (isLocalVisualPreview()) {
      return {
        policies: [...localVisualPreviewPolicies],
        oidc: {
          issuer: "https://token.actions.githubusercontent.com",
          audience: "fleets",
        },
      }
    }
    return request<GitHubTrustPolicyListResponse>("/api/github-trust-policies")
  },
  create: async (input: GitHubTrustPolicyInput) => {
    if (isLocalVisualPreview()) {
      const now = new Date().toISOString()
      const policy: GitHubTrustPolicy = {
        id: `preview-policy-${localVisualPreviewPolicies.length + 1}`,
        ...input,
        created_at: now,
        updated_at: now,
      }
      localVisualPreviewPolicies = [...localVisualPreviewPolicies, policy]
      return policy
    }
    return request<GitHubTrustPolicy>("/api/github-trust-policies", {
      method: "POST",
      body: JSON.stringify(input),
    })
  },
  update: async (id: string, input: Partial<GitHubTrustPolicyInput>) => {
    if (isLocalVisualPreview()) {
      const current = localVisualPreviewPolicies.find(policy => policy.id === id)
      if (!current) throw new Error("Preview trust policy not found")
      const policy = { ...current, ...input, updated_at: new Date().toISOString() }
      localVisualPreviewPolicies = localVisualPreviewPolicies.map(item =>
        item.id === id ? policy : item,
      )
      return policy
    }
    return request<GitHubTrustPolicy>(`/api/github-trust-policies/${encodeURIComponent(id)}`, {
      method: "PATCH",
      body: JSON.stringify(input),
    })
  },
  remove: async (id: string) => {
    if (isLocalVisualPreview()) {
      localVisualPreviewPolicies = localVisualPreviewPolicies.filter(
        policy => policy.id !== id,
      )
      return
    }
    return request<void>(`/api/github-trust-policies/${encodeURIComponent(id)}`, {
      method: "DELETE",
    })
  },
}

export const namespacesApi = {
  list: () => isLocalVisualPreview()
    ? Promise.resolve([
        {
          name: "preview",
          status: "Active",
          createdAt: "2026-08-16T20:20:00.000Z",
          labels: null,
        },
      ])
    : request<Namespace[]>("/api/namespaces"),
}

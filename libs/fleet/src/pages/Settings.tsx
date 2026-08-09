// Settings page — shows account identity plus GitHub Actions OIDC trust
// policies for backend automation.

import { useEffect, useMemo, useState } from "react"
import Alert from "@cloudscape-design/components/alert"
import Box from "@cloudscape-design/components/box"
import Button from "@cloudscape-design/components/button"
import Container from "@cloudscape-design/components/container"
import CopyToClipboard from "@cloudscape-design/components/copy-to-clipboard"
import Form from "@cloudscape-design/components/form"
import FormField from "@cloudscape-design/components/form-field"
import Header from "@cloudscape-design/components/header"
import Input from "@cloudscape-design/components/input"
import Modal from "@cloudscape-design/components/modal"
import Multiselect, {
  type MultiselectProps,
} from "@cloudscape-design/components/multiselect"
import SpaceBetween from "@cloudscape-design/components/space-between"
import Table from "@cloudscape-design/components/table"
import Toggle from "@cloudscape-design/components/toggle"
import { userInfo } from "../auth/keycloak"
import { useFeatureFlags } from "../components/FeatureFlagContext"
import {
  type GitHubTrustPolicy,
  githubTrustPoliciesApi,
  namespacesApi,
} from "../sdk/githubTrustPolicies"
import { BillingSettings } from "./Billing"

export function Settings() {
  const { sub, name } = userInfo()
  const { billing } = useFeatureFlags()
  const [policies, setPolicies] = useState<GitHubTrustPolicy[]>([])
  const [namespaceOptions, setNamespaceOptions] = useState<MultiselectProps.Option[]>([])
  const [selectedNamespaces, setSelectedNamespaces] = useState<MultiselectProps.Option[]>([])
  const [policyName, setPolicyName] = useState("")
  const [repository, setRepository] = useState("")
  const [enabled, setEnabled] = useState(true)
  const [editingId, setEditingId] = useState<string | null>(null)
  const [oidc, setOidc] = useState({
    issuer: "https://token.actions.githubusercontent.com",
    audience: "cyclops-cs",
  })
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [confirmDelete, setConfirmDelete] = useState<GitHubTrustPolicy | null>(null)

  const allNamespaceOptions = useMemo(() => {
    const byValue = new Map<string, MultiselectProps.Option>()
    for (const option of namespaceOptions) {
      if (option.value) byValue.set(option.value, option)
    }
    for (const option of selectedNamespaces) {
      if (option.value) byValue.set(option.value, option)
    }
    return [...byValue.values()].sort((a, b) =>
      (a.label ?? a.value ?? "").localeCompare(b.label ?? b.value ?? ""),
    )
  }, [namespaceOptions, selectedNamespaces])

  const refresh = async () => {
    setLoading(true)
    try {
      const [policyResp, namespaces] = await Promise.all([
        githubTrustPoliciesApi.list(),
        namespacesApi.list().catch(() => []),
      ])
      setPolicies(policyResp.policies)
      setOidc(policyResp.oidc)
      setNamespaceOptions(
        namespaces.map(ns => ({
          label: ns.name,
          value: ns.name,
        })),
      )
      setError(null)
    } catch (e) {
      setError(String(e))
    } finally {
      setLoading(false)
    }
  }

  // biome-ignore lint/correctness/useExhaustiveDependencies: one-time page load
  useEffect(() => {
    refresh()
  }, [])

  const resetForm = () => {
    setEditingId(null)
    setPolicyName("")
    setRepository("")
    setEnabled(true)
    setSelectedNamespaces([])
  }

  const collectNamespaces = () =>
    selectedNamespaces
      .flatMap(option => (option.value ? [option.value] : []))
      .sort()

  const submit = async () => {
    setSaving(true)
    try {
      const payload = {
        name: policyName,
        repository,
        allowed_namespaces: collectNamespaces(),
        enabled,
      }
      if (editingId) {
        await githubTrustPoliciesApi.update(editingId, payload)
      } else {
        await githubTrustPoliciesApi.create(payload)
      }
      resetForm()
      await refresh()
    } catch (e) {
      setError(String(e))
    } finally {
      setSaving(false)
    }
  }

  const startEdit = (policy: GitHubTrustPolicy) => {
    setEditingId(policy.id)
    setPolicyName(policy.name)
    setRepository(policy.repository)
    setEnabled(policy.enabled)
    setSelectedNamespaces(
      policy.allowed_namespaces.map(ns => ({ label: ns, value: ns })),
    )
  }

  const togglePolicy = async (policy: GitHubTrustPolicy) => {
    setBusyId(policy.id)
    try {
      await githubTrustPoliciesApi.update(policy.id, {
        enabled: !policy.enabled,
      })
      await refresh()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusyId(null)
    }
  }

  const removePolicy = async () => {
    if (!confirmDelete) return
    setBusyId(confirmDelete.id)
    try {
      await githubTrustPoliciesApi.remove(confirmDelete.id)
      setConfirmDelete(null)
      if (editingId === confirmDelete.id) resetForm()
      await refresh()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusyId(null)
    }
  }

  const workflowSnippet = `permissions:
  id-token: write
  contents: read

steps:
  - name: Request GitHub OIDC token for cyclops-cs
    id: oidc
    run: |
      echo "token=$(curl -sSL \\
        -H \\"Authorization: bearer $ACTIONS_ID_TOKEN_REQUEST_TOKEN\\" \\
        \\"$ACTIONS_ID_TOKEN_REQUEST_URL&audience=${oidc.audience}\\" | jq -r '.value')" >> "$GITHUB_OUTPUT"

  - name: Call cyclops-cs
    env:
      CYCLOPS_CS_TOKEN: \${{ steps.oidc.outputs.token }}
    run: |
      curl -sSL https://cyclops.example.test/api/namespaces \\
        -H "Authorization: Bearer $CYCLOPS_CS_TOKEN"`

  return (
    <SpaceBetween size="l">
      {error && (
        <Alert type="error" dismissible onDismiss={() => setError(null)}>
          {error}
        </Alert>
      )}

      <Container header={<Header variant="h2">Account</Header>}>
        <SpaceBetween size="m">
          <FormField label="Username">
            {name ? (
              <CopyToClipboard
                variant="inline"
                textToCopy={name}
                textToDisplay={<code>{name}</code>}
                copyButtonAriaLabel="Copy username"
                copySuccessText="Username copied"
                copyErrorText="Failed to copy username"
              />
            ) : (
              <Box color="text-status-inactive">Unknown</Box>
            )}
          </FormField>
          <FormField label="Subject (sub)">
            {sub ? (
              <CopyToClipboard
                variant="inline"
                textToCopy={sub}
                textToDisplay={<code>{sub}</code>}
                copyButtonAriaLabel="Copy subject"
                copySuccessText="Subject copied"
                copyErrorText="Failed to copy subject"
              />
            ) : (
              <Box color="text-status-inactive">Unknown</Box>
            )}
          </FormField>
        </SpaceBetween>
      </Container>

      <Container
        header={
          <Header
            variant="h2"
            description="Trust GitHub repositories to call cyclops-cs directly with GitHub Actions OIDC tokens."
          >
            GitHub Actions OIDC
          </Header>
        }
      >
        <SpaceBetween size="l">
          <Form
            header={
              <Header
                variant="h3"
                description="Create or update a repository trust policy for namespaces you own."
              >
                {editingId ? "Edit trust policy" : "Create trust policy"}
              </Header>
            }
            actions={
              <SpaceBetween direction="horizontal" size="xs">
                {editingId && <Button onClick={resetForm}>Cancel</Button>}
                <Button
                  variant="primary"
                  loading={saving}
                  disabled={!policyName || !repository}
                  onClick={submit}
                >
                  {editingId ? "Save policy" : "Create policy"}
                </Button>
              </SpaceBetween>
            }
          >
            <SpaceBetween size="m">
              <FormField label="Display name">
                <Input
                  value={policyName}
                  onChange={e => setPolicyName(e.detail.value)}
                  placeholder="CI access"
                />
              </FormField>
              <FormField
                label="Repository"
                description="Exact owner/repo match from the GitHub OIDC repository claim."
              >
                <Input
                  value={repository}
                  onChange={e => setRepository(e.detail.value)}
                  placeholder="trycua/cloud"
                />
              </FormField>
              <FormField
                label="Existing namespaces"
                description="Select namespaces you already own."
              >
                <Multiselect
                  selectedOptions={selectedNamespaces}
                  onChange={e =>
                    setSelectedNamespaces([...e.detail.selectedOptions])
                  }
                  options={allNamespaceOptions}
                  placeholder="Choose existing namespaces"
                  loadingText="Loading namespaces..."
                  statusType={loading ? "loading" : "finished"}
                  empty="No namespaces found"
                />
              </FormField>
              <Toggle
                checked={enabled}
                onChange={e => setEnabled(e.detail.checked)}
              >
                Policy enabled
              </Toggle>
            </SpaceBetween>
          </Form>

          <Table
            loading={loading}
            items={policies}
            columnDefinitions={[
              { id: "name", header: "Name", cell: policy => policy.name },
              {
                id: "repository",
                header: "Repository",
                cell: policy => <code>{policy.repository}</code>,
              },
              {
                id: "namespaces",
                header: "Namespaces",
                cell: policy => policy.allowed_namespaces.join(", "),
              },
              {
                id: "enabled",
                header: "Enabled",
                cell: policy => (policy.enabled ? "Yes" : "No"),
              },
              {
                id: "updated",
                header: "Updated",
                cell: policy => new Date(policy.updated_at).toLocaleString(),
              },
              {
                id: "actions",
                header: "",
                cell: policy => (
                  <SpaceBetween direction="horizontal" size="xs">
                    <Button onClick={() => startEdit(policy)}>Edit</Button>
                    <Button
                      loading={busyId === policy.id}
                      onClick={() => togglePolicy(policy)}
                    >
                      {policy.enabled ? "Disable" : "Enable"}
                    </Button>
                    <Button onClick={() => setConfirmDelete(policy)}>
                      Delete
                    </Button>
                  </SpaceBetween>
                ),
              },
            ]}
            empty={
              <Box textAlign="center">
                No GitHub trust policies yet.
              </Box>
            }
            header={<Header variant="h3">Configured policies</Header>}
          />

          <Container
            header={
              <Header
                variant="h3"
                description="Use these exact values in your GitHub workflow when requesting an OIDC token."
              >
                Workflow guidance
              </Header>
            }
          >
            <SpaceBetween size="m">
              <FormField label="Issuer">
                <CopyToClipboard
                  variant="inline"
                  textToCopy={oidc.issuer}
                  textToDisplay={<code>{oidc.issuer}</code>}
                  copyButtonAriaLabel="Copy GitHub OIDC issuer"
                  copySuccessText="Issuer copied"
                  copyErrorText="Failed to copy issuer"
                />
              </FormField>
              <FormField label="Audience">
                <CopyToClipboard
                  variant="inline"
                  textToCopy={oidc.audience}
                  textToDisplay={<code>{oidc.audience}</code>}
                  copyButtonAriaLabel="Copy GitHub OIDC audience"
                  copySuccessText="Audience copied"
                  copyErrorText="Failed to copy audience"
                />
              </FormField>
              <FormField label="Example workflow">
                <Box variant="code" padding="m">
                  <pre style={{ margin: 0, whiteSpace: "pre-wrap" }}>
                    {workflowSnippet}
                  </pre>
                </Box>
              </FormField>
              <Alert type="info">
                Matching is exact on the GitHub OIDC <code>repository</code>{" "}
                claim. The backend grants access only to the namespaces
                listed in the matching trust policy.
              </Alert>
            </SpaceBetween>
          </Container>
        </SpaceBetween>
      </Container>

      {billing && <BillingSettings />}

      {confirmDelete && (
        <Modal
          visible
          header="Delete GitHub trust policy?"
          onDismiss={() => setConfirmDelete(null)}
          footer={
            <Box float="right">
              <SpaceBetween direction="horizontal" size="xs">
                <Button onClick={() => setConfirmDelete(null)}>Cancel</Button>
                <Button
                  variant="primary"
                  loading={busyId === confirmDelete.id}
                  onClick={removePolicy}
                >
                  Delete
                </Button>
              </SpaceBetween>
            </Box>
          }
        >
          Delete <b>{confirmDelete.name}</b> for{" "}
          <code>{confirmDelete.repository}</code>? GitHub workflows using
          this policy will lose access immediately.
        </Modal>
      )}
    </SpaceBetween>
  )
}

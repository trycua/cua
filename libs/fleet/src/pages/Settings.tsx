// Settings page — shows account identity plus GitHub Actions OIDC trust
// policies for backend automation.

import { useEffect, useMemo, useState } from "react"
import Alert from "@cloudscape-design/components/alert"
import Box from "@cloudscape-design/components/box"
import Container from "@cloudscape-design/components/container"
import CopyToClipboard from "@cloudscape-design/components/copy-to-clipboard"
import ExpandableSection from "@cloudscape-design/components/expandable-section"
import Form from "@cloudscape-design/components/form"
import FormField from "@cloudscape-design/components/form-field"
import Header from "@cloudscape-design/components/header"
import Input from "@cloudscape-design/components/input"
import Modal from "@cloudscape-design/components/modal"
import Multiselect, {
  type MultiselectProps,
} from "@cloudscape-design/components/multiselect"
import Select from "@cloudscape-design/components/select"
import SpaceBetween from "@cloudscape-design/components/space-between"
import Table from "@cloudscape-design/components/table"
import Toggle from "@cloudscape-design/components/toggle"
import { userInfo } from "../auth/keycloak"
import { errorMessage } from "../error-message"
import { CuaButton } from "../components/CuaButton"
import { useFeatureFlags } from "../components/FeatureFlagContext"
import { useFlash } from "../components/FlashContext"
import { PageEmpty, PageError } from "../components/PageState"
import { PageShell } from "../components/PageShell"
import {
  type GitHubTrustPolicy,
  githubTrustPoliciesApi,
  namespacesApi,
} from "../api/githubTrustPolicies"
import { BillingSettings } from "./Billing"
import { useI18n } from "../i18n/I18nProvider"
import { localeLabels, supportedLocales, type Locale } from "../i18n/translations"

export function Settings() {
  const flash = useFlash()
  const { formatDateTime, locale, setLocale, t } = useI18n()
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
    audience: "fleets",
  })
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
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
      setLoadError(null)
    } catch (e) {
      setLoadError(errorMessage(e))
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
      flash.push({
        type: "success",
        header: editingId ? "Trust policy saved" : "Trust policy created",
        content: "The change is stored remotely and applies immediately.",
      })
      resetForm()
      await refresh()
    } catch (e) {
      flash.push({
        type: "error",
        header: editingId
          ? "Failed to save trust policy"
          : "Failed to create trust policy",
        content: errorMessage(e),
      })
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
      flash.push({
        type: "success",
        header: policy.enabled ? "Trust policy disabled" : "Trust policy enabled",
        content: "The change is stored remotely and applies immediately.",
      })
      await refresh()
    } catch (e) {
      flash.push({
        type: "error",
        header: "Failed to update trust policy",
        content: errorMessage(e),
      })
    } finally {
      setBusyId(null)
    }
  }

  const removePolicy = async () => {
    if (!confirmDelete) return
    setBusyId(confirmDelete.id)
    try {
      await githubTrustPoliciesApi.remove(confirmDelete.id)
      flash.push({
        type: "success",
        header: "Trust policy deleted",
        content: "GitHub workflows using it can no longer authenticate.",
      })
      setConfirmDelete(null)
      if (editingId === confirmDelete.id) resetForm()
      await refresh()
    } catch (e) {
      flash.push({
        type: "error",
        header: "Failed to delete trust policy",
        content: errorMessage(e),
      })
    } finally {
      setBusyId(null)
    }
  }

  const workflowSnippet = `permissions:
  id-token: write
  contents: read

steps:
  - uses: actions/setup-python@v5
    with:
      python-version: "3.12"

  - name: Install Cua CLI
    run: pip install cua-cli==0.1.14

  - name: Get GitHub WIF token for Fleets
    id: fleets-token
    run: |
      token="$(cua wif-token github)"
      echo "::add-mask::$token"
      printf 'token=%s\\n' "$token" >> "$GITHUB_OUTPUT"

  - name: Create and use an authorized Fleets sandbox
    env:
      FLEETS_TOKEN: \${{ steps.fleets-token.outputs.token }}
    run: |
      set -euo pipefail
      # This exact value is authorized by the repository trust policy.
      # Cleanup releases this claim; its pool, template, and namespace persist.
      sandbox="replace-with-an-authorized-namespace"
      cleanup() {
        status=$?
        if ! cua sb delete --force "$sandbox"; then
          if [ "$status" -eq 0 ]; then status=1; fi
        fi
        exit "$status"
      }
      trap cleanup EXIT

      cua sb launch \\
        public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:latest \\
        --name "$sandbox"
      cua sb exec "$sandbox" sh -lc 'uname -a; id; pwd'`
  return (
    <PageShell
      eyebrow={t("account.heading")}
      title={t("page.settings")}
    >
      <SpaceBetween size="l">
        <Container
          header={
            <Header variant="h2" description={t("i18n.description")}>
              {t("i18n.heading")}
            </Header>
          }
        >
          <FormField label={t("i18n.displayLanguage")}>
            <Select
              selectedOption={{ label: localeLabels[locale], value: locale }}
              onChange={({ detail }) => setLocale(detail.selectedOption.value as Locale)}
              options={supportedLocales.map(value => ({
                label: localeLabels[value],
                value,
              }))}
              placeholder={t("i18n.selectPlaceholder")}
            />
          </FormField>
        </Container>
        <ExpandableSection
          variant="container"
          defaultExpanded={false}
          headerText={t("account.heading")}
          headerDescription={t("account.description")}
        >
          <SpaceBetween size="m">
            <FormField label={t("account.username")}>
              {name ? (
                <CopyToClipboard
                  variant="inline"
                  textToCopy={name}
                  textToDisplay={<code>{name}</code>}
                  copyButtonAriaLabel={t("account.copyUsername")}
                  copySuccessText={t("account.usernameCopied")}
                  copyErrorText={t("account.usernameCopyFailed")}
                />
              ) : (
                <Box color="text-status-inactive">{t("account.unknown")}</Box>
              )}
            </FormField>
            <FormField label={t("account.subject")}>
              {sub ? (
                <CopyToClipboard
                  variant="inline"
                  textToCopy={sub}
                  textToDisplay={<code>{sub}</code>}
                  copyButtonAriaLabel={t("account.copySubject")}
                  copySuccessText={t("account.subjectCopied")}
                  copyErrorText={t("account.subjectCopyFailed")}
                />
              ) : (
                <Box color="text-status-inactive">{t("account.unknown")}</Box>
              )}
            </FormField>
          </SpaceBetween>
        </ExpandableSection>

      {billing && <BillingSettings />}

      <ExpandableSection
        variant="container"
        defaultExpanded={false}
        headerText="GitHub Actions OIDC"
        headerDescription={loadError
          ? "GitHub Actions settings are unavailable. Expand to retry."
          : "Trust GitHub repositories to call Fleets with short-lived tokens. Policies are stored remotely and apply immediately."}
      >
        {loadError ? (
          <PageError
            title="GitHub Actions settings are unavailable"
            action={<CuaButton onClick={refresh}>Retry</CuaButton>}
          >
            {loadError}
          </PageError>
        ) : (
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
                {editingId && <CuaButton onClick={resetForm}>Cancel</CuaButton>}
                <CuaButton
                  tone="primary"
                  loading={saving}
                  disabled={!policyName || !repository}
                  onClick={submit}
                >
                  {editingId ? "Save policy" : "Create policy"}
                </CuaButton>
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
            variant="borderless"
            loading={loading}
            loadingText="Loading GitHub trust policies"
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
                cell: policy => formatDateTime(policy.updated_at),
              },
              {
                id: "actions",
                header: "",
                cell: policy => (
                  <SpaceBetween direction="horizontal" size="xs">
                    <CuaButton
                      disabled={busyId !== null}
                      onClick={() => startEdit(policy)}
                    >
                      Edit
                    </CuaButton>
                    <CuaButton
                      loading={busyId === policy.id}
                      disabled={busyId !== null}
                      onClick={() => togglePolicy(policy)}
                    >
                      {policy.enabled ? "Disable" : "Enable"}
                    </CuaButton>
                    <CuaButton
                      tone="danger"
                      disabled={busyId !== null}
                      onClick={() => setConfirmDelete(policy)}
                    >
                      Delete
                    </CuaButton>
                  </SpaceBetween>
                ),
              },
            ]}
            empty={
              <PageEmpty title="No GitHub trust policies">
                Create a policy when a repository needs short-lived access to Fleets.
              </PageEmpty>
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
              <FormField label="Workflow job snippet">
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
        )}
      </ExpandableSection>

      {confirmDelete && (
        <Modal
          visible
          header="Delete GitHub trust policy?"
          onDismiss={() => setConfirmDelete(null)}
          footer={
            <Box float="right">
              <SpaceBetween direction="horizontal" size="xs">
                <CuaButton onClick={() => setConfirmDelete(null)}>Cancel</CuaButton>
                <CuaButton
                  tone="danger"
                  loading={busyId === confirmDelete.id}
                  onClick={removePolicy}
                >
                  Delete
                </CuaButton>
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
    </PageShell>
  )
}

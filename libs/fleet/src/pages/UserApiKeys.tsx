// Per-user API key management.
//
// Lists the calling user's personal API keys, lets them create new ones
// (showing the credentials exactly once), and revoke existing keys.
// Each key is a Keycloak service-account client; the client_secret is
// returned once on creation and cannot be retrieved later.

import { useEffect, useState } from "react"
import designTokens from "@cua/design/tokens.json"
import Alert from "@cloudscape-design/components/alert"
import Box from "@cloudscape-design/components/box"
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
import {
  createUserKey,
  deleteUserKey,
  listUserKeys,
  type NewUserApiKey,
  type UserApiKey,
} from "../fleet/userKeys"
import { listNamespaces } from "../fleet/pools"
import { errorMessage } from "../error-message"
import { CuaButton } from "../components/CuaButton"
import { useFlash } from "../components/FlashContext"
import { PageEmpty, PageError } from "../components/PageState"
import { PageShell } from "../components/PageShell"

export function UserApiKeys() {
  const flash = useFlash()
  const [keys, setKeys] = useState<UserApiKey[]>([])
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [name, setName] = useState("")
  const [selectedScope, setSelectedScope] = useState<
    MultiselectProps.Option[]
  >([])
  const [nsOptions, setNsOptions] = useState<MultiselectProps.Option[]>([])
  const [creating, setCreating] = useState(false)
  const [created, setCreated] = useState<NewUserApiKey | null>(null)
  const [confirmRevoke, setConfirmRevoke] = useState<string | null>(null)
  const [revokingId, setRevokingId] = useState<string | null>(null)
  const [compactTable, setCompactTable] = useState(
    () => window.matchMedia(
      `(max-width: ${designTokens.layout.breakpoint.mobile - 1}px)`,
    ).matches,
  )

  useEffect(() => {
    const query = window.matchMedia(
      `(max-width: ${designTokens.layout.breakpoint.mobile - 1}px)`,
    )
    const update = () => setCompactTable(query.matches)
    query.addEventListener("change", update)
    return () => query.removeEventListener("change", update)
  }, [])

  const refresh = async () => {
    setLoading(true)
    try {
      const [userKeys, namespaces] = await Promise.all([
        listUserKeys(),
        listNamespaces().catch(() => []),
      ])
      setKeys(userKeys)
      setNsOptions(
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

  const create = async () => {
    setCreating(true)
    try {
      const scope = selectedScope.map(o => o.value!).filter(Boolean)
      const k = await createUserKey(
        name,
        scope.length > 0 ? scope : undefined,
      )
      setCreated(k)
      setName("")
      setSelectedScope([])
      await refresh()
    } catch (e) {
      flash.push({
        type: "error",
        header: "Failed to create API key",
        content: errorMessage(e),
      })
    } finally {
      setCreating(false)
    }
  }

  const remove = async (id: string) => {
    setRevokingId(id)
    try {
      await deleteUserKey(id)
      setConfirmRevoke(null)
      flash.push({
        type: "success",
        header: "API key revoked",
        content: "Systems using this key can no longer authenticate.",
      })
      await refresh()
    } catch (e) {
      flash.push({
        type: "error",
        header: "Failed to revoke API key",
        content: errorMessage(e),
      })
    } finally {
      setRevokingId(null)
    }
  }

  return (
    <PageShell
      eyebrow="Account"
      title="API keys"
      counter={loading || loadError ? undefined : `(${keys.length})`}
      secondaryActions={
        <CuaButton
          tone="icon"
          ariaLabel="Refresh API keys"
          iconName="refresh"
          disabled={loading}
          onClick={refresh}
        />
      }
    >
      <SpaceBetween size="l">
        <Container
          header={
            <Header variant="h2">
              Create API key
            </Header>
          }
        >
          <Form
            actions={
              <CuaButton
                tone="primary"
                loading={creating}
                disabled={!name}
                onClick={create}
              >
                Create key
              </CuaButton>
            }
          >
            <SpaceBetween size="m">
            <FormField
              controlId="api-key-name"
              label="Name"
            >
              <Input value={name} onChange={e => setName(e.detail.value)} />
            </FormField>
            <FormField
              label="Allowed Namespaces (optional)"
            >
              <Multiselect
                selectedOptions={selectedScope}
                onChange={e =>
                  setSelectedScope([...e.detail.selectedOptions])
                }
                options={nsOptions}
                placeholder="All namespaces (no restriction)"
                filteringType="auto"
                filteringPlaceholder="Filter namespaces"
                filteringAriaLabel="Filter namespaces"
                loadingText="Loading namespaces..."
                noMatch="No matching namespaces"
                statusType={loading ? "loading" : "finished"}
                empty="No namespaces found"
              />
            </FormField>
            </SpaceBetween>
          </Form>
        </Container>

      {loadError ? (
        <Container>
          <PageError
            title="API keys are unavailable"
            action={<CuaButton onClick={refresh}>Retry</CuaButton>}
          >
            {loadError}
          </PageError>
        </Container>
      ) : (
        <section className="cua-page-table">
          <Table
            variant="borderless"
            loading={loading}
            loadingText="Loading API keys"
            items={keys}
            visibleColumns={compactTable
              ? ["name", "scope", "actions"]
              : ["name", "client_id", "scope", "actions"]}
            columnDefinitions={[
          {
            id: "name",
            header: "Name",
            cell: r => (
              <SpaceBetween size="xxs">
                <span>{r.name}</span>
                {compactTable ? <code>{r.clientId}</code> : null}
              </SpaceBetween>
            ),
          },
          { id: "client_id", header: "Client ID", cell: r => <code>{r.clientId}</code> },
          {
            id: "scope",
            header: "Namespaces",
            cell: r =>
              r.scope && r.scope.length > 0
                ? r.scope.join(", ")
                : "All namespaces",
          },
          {
            id: "actions",
            header: "",
            cell: r => (
              <CuaButton
                tone="danger"
                loading={revokingId === r.id}
                disabled={revokingId !== null}
                onClick={() => setConfirmRevoke(r.id)}
              >
                Revoke
              </CuaButton>
            ),
          },
            ]}
            empty={
          <PageEmpty
            title="No API keys"
            action={
              <CuaButton
                tone="primary"
                onClick={() => document.getElementById("api-key-name")?.focus()}
              >
                Create API key
              </CuaButton>
            }
          >
            Create a key when a tool or workflow needs to access Cua.
          </PageEmpty>
            }
            header={
          <Header variant="h2">
            API keys
          </Header>
            }
          />
        </section>
      )}

      {/* Key-created modal -- shows credentials once */}
      {created && (
        <Modal
          visible
          header="API key created"
          onDismiss={() => setCreated(null)}
          footer={
            <Box float="right">
              <CuaButton tone="primary" onClick={() => setCreated(null)}>
                I have copied the credentials
              </CuaButton>
            </Box>
          }
        >
          <SpaceBetween size="m">
            <Alert type="warning">
              These credentials will only be shown <b>once</b>. Copy them now
              -- they cannot be retrieved later.
            </Alert>
            <Container>
              <SpaceBetween size="m">
                <FormField label="Client ID">
                  <CopyToClipboard
                    variant="inline"
                    textToCopy={created.clientId}
                    textToDisplay={<code>{created.clientId}</code>}
                    copyButtonAriaLabel="Copy client ID"
                    copySuccessText="Client ID copied"
                    copyErrorText="Failed to copy"
                  />
                </FormField>
                <FormField label="Client Secret">
                  <CopyToClipboard
                    variant="inline"
                    textToCopy={created.clientSecret}
                    textToDisplay={<code>{created.clientSecret}</code>}
                    copyButtonAriaLabel="Copy client secret"
                    copySuccessText="Client secret copied"
                    copyErrorText="Failed to copy"
                  />
                </FormField>
              </SpaceBetween>
            </Container>
          </SpaceBetween>
        </Modal>
      )}

      {/* Revoke confirmation modal */}
      {confirmRevoke && (
        <Modal
          visible
          header="Revoke API key?"
          onDismiss={() => setConfirmRevoke(null)}
          footer={
            <Box float="right">
              <SpaceBetween direction="horizontal" size="xs">
                <CuaButton onClick={() => setConfirmRevoke(null)}>Cancel</CuaButton>
                <CuaButton
                  tone="danger"
                  loading={revokingId === confirmRevoke}
                  onClick={() => remove(confirmRevoke)}
                >
                  Revoke
                </CuaButton>
              </SpaceBetween>
            </Box>
          }
        >
          This action cannot be undone. Any systems using this key will lose
          access immediately.
        </Modal>
      )}
      </SpaceBetween>
    </PageShell>
  )
}

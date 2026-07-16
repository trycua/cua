// Per-user API key management.
//
// Lists the calling user's personal API keys, lets them create new ones
// (showing the credentials exactly once), and revoke existing keys.
// Each key is a Keycloak service-account client; the client_secret is
// returned once on creation and cannot be retrieved later.

import { useEffect, useState } from "react"
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
import {
  type UserApiKey,
  type NewUserApiKey,
  userKeysApi,
  namespacesApi,
} from "../api/cyclops"

export function UserApiKeys() {
  const [keys, setKeys] = useState<UserApiKey[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [name, setName] = useState("")
  const [selectedScope, setSelectedScope] = useState<
    MultiselectProps.Option[]
  >([])
  const [nsOptions, setNsOptions] = useState<MultiselectProps.Option[]>([])
  const [creating, setCreating] = useState(false)
  const [created, setCreated] = useState<NewUserApiKey | null>(null)
  const [confirmRevoke, setConfirmRevoke] = useState<string | null>(null)

  const refresh = async () => {
    setLoading(true)
    try {
      const [userKeys, namespaces] = await Promise.all([
        userKeysApi.list(),
        namespacesApi.list().catch(() => []),
      ])
      setKeys(userKeys)
      setNsOptions(
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

  useEffect(() => {
    refresh()
  }, [])

  const create = async () => {
    setCreating(true)
    try {
      const scope = selectedScope.map(o => o.value!).filter(Boolean)
      const k = await userKeysApi.create(
        name,
        scope.length > 0 ? scope : undefined,
      )
      setCreated(k)
      setName("")
      setSelectedScope([])
      await refresh()
    } catch (e) {
      setError(String(e))
    } finally {
      setCreating(false)
    }
  }

  const remove = async (id: string) => {
    try {
      await userKeysApi.remove(id)
      setConfirmRevoke(null)
      await refresh()
    } catch (e) {
      setError(String(e))
    }
  }

  return (
    <SpaceBetween size="l">
      {error && (
        <Alert type="error" dismissible onDismiss={() => setError(null)}>
          {error}
        </Alert>
      )}

      <Container header={<Header variant="h2">Create a new API key</Header>}>
        <Form
          actions={
            <Button
              variant="primary"
              loading={creating}
              disabled={!name}
              onClick={create}
            >
              Create key
            </Button>
          }
        >
          <SpaceBetween size="m">
            <FormField label="Name" description="A label to identify this key">
              <Input value={name} onChange={e => setName(e.detail.value)} />
            </FormField>
            <FormField
              label="Scope (optional)"
              description="Restrict this key to specific namespaces. Leave empty for full access."
            >
              <Multiselect
                selectedOptions={selectedScope}
                onChange={e =>
                  setSelectedScope([...e.detail.selectedOptions])
                }
                options={nsOptions}
                placeholder="All namespaces (no restriction)"
                loadingText="Loading namespaces..."
                statusType={loading ? "loading" : "finished"}
                empty="No namespaces found"
              />
            </FormField>
          </SpaceBetween>
        </Form>
      </Container>

      <Table
        loading={loading}
        items={keys}
        columnDefinitions={[
          { id: "name", header: "Name", cell: r => r.name },
          { id: "client_id", header: "Client ID", cell: r => <code>{r.client_id}</code> },
          {
            id: "scope",
            header: "Scope",
            cell: r =>
              r.scope && r.scope.length > 0
                ? r.scope.join(", ")
                : "All namespaces",
          },
          {
            id: "actions",
            header: "",
            cell: r => (
              <Button onClick={() => setConfirmRevoke(r.id)}>Revoke</Button>
            ),
          },
        ]}
        empty={<Box textAlign="center">No API keys yet.</Box>}
        header={<Header variant="h2">Your API keys</Header>}
      />

      {/* Key-created modal -- shows credentials once */}
      {created && (
        <Modal
          visible
          header="API key created"
          onDismiss={() => setCreated(null)}
          footer={
            <Box float="right">
              <Button variant="primary" onClick={() => setCreated(null)}>
                I have copied the credentials
              </Button>
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
                    textToCopy={created.client_id}
                    textToDisplay={<code>{created.client_id}</code>}
                    copyButtonAriaLabel="Copy client ID"
                    copySuccessText="Client ID copied"
                    copyErrorText="Failed to copy"
                  />
                </FormField>
                <FormField label="Client Secret">
                  <CopyToClipboard
                    variant="inline"
                    textToCopy={created.client_secret}
                    textToDisplay={<code>{created.client_secret}</code>}
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
                <Button onClick={() => setConfirmRevoke(null)}>Cancel</Button>
                <Button
                  variant="primary"
                  onClick={() => remove(confirmRevoke)}
                >
                  Revoke
                </Button>
              </SpaceBetween>
            </Box>
          }
        >
          This action cannot be undone. Any systems using this key will lose
          access immediately.
        </Modal>
      )}
    </SpaceBetween>
  )
}

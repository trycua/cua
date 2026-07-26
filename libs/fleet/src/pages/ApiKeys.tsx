// API keys management.
//
// Lists the calling user's keys, lets them mint new ones (showing the
// generated client_secret exactly once), and revoke existing keys. Each
// key is a Keycloak service-account client with a `namespace` claim
// mapper, which gates which orchestrator the resulting JWT can reach via
// /api/gateway/:name.

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
import Select, { type SelectProps } from "@cloudscape-design/components/select"
import SpaceBetween from "@cloudscape-design/components/space-between"
import Table from "@cloudscape-design/components/table"
import { ApiKey, NewApiKey, keysApi } from "../api/keys"
import { api } from "../api/cyclops"

export function ApiKeys() {
  const [keys, setKeys] = useState<ApiKey[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [name, setName] = useState("")
  const [selectedNs, setSelectedNs] = useState<SelectProps.Option | null>(null)
  const [nsOptions, setNsOptions] = useState<SelectProps.Option[]>([])
  const [creating, setCreating] = useState(false)
  const [created, setCreated] = useState<NewApiKey | null>(null)

  const refresh = async () => {
    setLoading(true)
    try {
      const [r, pools] = await Promise.all([
        keysApi.list(),
        api.listPools(),
      ])
      setKeys(r.keys)
      const poolOpts = pools.map(p => ({
        label: `${p.name} (pool-${p.name})`,
        value: `pool-${p.name}`,
      }))
      setNsOptions(poolOpts)
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
      const k = await keysApi.create(name, selectedNs!.value!)
      setCreated(k)
      setName("")
      setSelectedNs(null)
      await refresh()
    } catch (e) {
      setError(String(e))
    } finally {
      setCreating(false)
    }
  }

  const remove = async (id: string) => {
    try {
      await keysApi.remove(id)
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

      <Container header={<Header variant="h2">Issue a new API key</Header>}>
        <Form
          actions={
            <Button
              variant="primary"
              loading={creating}
              disabled={!name || !selectedNs}
              onClick={create}
            >
              Create key
            </Button>
          }
        >
          <SpaceBetween size="m">
            <FormField label="Name" description="Free-text label for this key">
              <Input value={name} onChange={e => setName(e.detail.value)} />
            </FormField>
            <FormField
              label="Namespace"
              description="Pool namespace this key may proxy to"
            >
              <Select
                selectedOption={selectedNs}
                onChange={e => setSelectedNs(e.detail.selectedOption)}
                options={nsOptions}
                placeholder="Select a pool"
                loadingText="Loading pools..."
                statusType={loading ? "loading" : "finished"}
                empty="No pools found"
              />
            </FormField>
          </SpaceBetween>
        </Form>
      </Container>

      <Table
        loading={loading}
        items={keys}
        columnDefinitions={[
          { id: "client_id", header: "Client ID", cell: r => r.client_id },
          { id: "name", header: "Name", cell: r => r.name },
          { id: "namespace", header: "Namespace", cell: r => r.namespace },
          {
            id: "actions",
            header: "",
            cell: r => (
              <Button onClick={() => remove(r.id)}>Revoke</Button>
            ),
          },
        ]}
        empty={<Box textAlign="center">No keys yet.</Box>}
        header={<Header variant="h2">Existing keys</Header>}
      />

      {created && (
        <Modal
          visible
          header="API key created"
          onDismiss={() => setCreated(null)}
          footer={
            <Box float="right">
              <Button variant="primary" onClick={() => setCreated(null)}>
                I have copied the secret
              </Button>
            </Box>
          }
        >
          <SpaceBetween size="m">
            <Alert type="warning">
              The client secret is shown <b>once</b>. Copy it now — it
              can&rsquo;t be retrieved later. To rotate, revoke this key and
              create a new one.
            </Alert>
            <Container>
              <SpaceBetween size="m">
                <FormField label="Client ID">
                  <CopyToClipboard
                    variant="inline"
                    textToCopy={created.client_id}
                    textToDisplay={<code>{created.client_id}</code>}
                    copyButtonAriaLabel="Copy Client ID"
                    copySuccessText="Client ID copied"
                    copyErrorText="Failed to copy"
                  />
                </FormField>
                <FormField label="Client secret">
                  <CopyToClipboard
                    variant="inline"
                    textToCopy={created.client_secret}
                    textToDisplay={<code>{created.client_secret}</code>}
                    copyButtonAriaLabel="Copy Client secret"
                    copySuccessText="Client secret copied"
                    copyErrorText="Failed to copy"
                  />
                </FormField>
              </SpaceBetween>
            </Container>
          </SpaceBetween>
        </Modal>
      )}
    </SpaceBetween>
  )
}

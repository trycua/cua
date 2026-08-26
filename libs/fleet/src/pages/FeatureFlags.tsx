import { useCallback, useEffect, useMemo, useState } from "react"
import { useCollection } from "@cloudscape-design/collection-hooks"
import Alert from "@cloudscape-design/components/alert"
import Badge from "@cloudscape-design/components/badge"
import Box from "@cloudscape-design/components/box"
import Button from "@cloudscape-design/components/button"
import ButtonDropdown from "@cloudscape-design/components/button-dropdown"
import Checkbox from "@cloudscape-design/components/checkbox"
import CopyToClipboard from "@cloudscape-design/components/copy-to-clipboard"
import Form from "@cloudscape-design/components/form"
import FormField from "@cloudscape-design/components/form-field"
import Header from "@cloudscape-design/components/header"
import Input from "@cloudscape-design/components/input"
import Link from "@cloudscape-design/components/link"
import Modal from "@cloudscape-design/components/modal"
import Select, { type SelectProps } from "@cloudscape-design/components/select"
import SpaceBetween from "@cloudscape-design/components/space-between"
import StatusIndicator from "@cloudscape-design/components/status-indicator"
import Table from "@cloudscape-design/components/table"
import Textarea from "@cloudscape-design/components/textarea"
import TextFilter from "@cloudscape-design/components/text-filter"
import { useNavigate } from "react-router-dom"
import { userInfo } from "../auth/keycloak"
import { useFeatureFlags } from "../components/FeatureFlagContext"
import { useFlash } from "../components/FlashContext"
import {
  AdminFeatureFlagError,
  adminFeatureFlagsApi,
  type AdminFeatureFlag,
  type FeatureFlagOwnership,
  type FeatureFlagValueType,
  type JSONValue,
} from "../api/adminFeatureFlags"

const TERRAFORM_FEATURE_FLAGS_URL =
  "https://github.com/trycua/cloud/tree/main/terraform/aws/feature-flags"

const VALUE_TYPE_OPTIONS: SelectProps.Option[] = [
  { label: "Boolean", value: "boolean" },
  { label: "Number", value: "number" },
  { label: "String", value: "string" },
  { label: "JSON", value: "json" },
]

const BOOLEAN_OPTIONS: SelectProps.Option[] = [
  { label: "True", value: "true" },
  { label: "False", value: "false" },
]

interface EditorState {
  mode: "create" | "edit"
  original?: AdminFeatureFlag
  key: string
  valueType: FeatureFlagValueType
  valueText: string
  description: string
  error?: string
  errorField?: "key" | "value"
  conflict?: AdminFeatureFlag
}

function typeLabel(valueType: FeatureFlagValueType): string {
  return valueType === "json" ? "JSON" : valueType[0].toUpperCase() + valueType.slice(1)
}

function ownershipLabel(ownership: FeatureFlagOwnership): string {
  if (ownership === "ad_hoc") return "Ad hoc"
  return ownership[0].toUpperCase() + ownership.slice(1)
}

function formatValue(value: JSONValue, valueType: FeatureFlagValueType): string {
  if (valueType === "string") return String(value)
  if (valueType === "json") return JSON.stringify(value, null, 2)
  return JSON.stringify(value)
}

function compactValue(value: JSONValue, valueType: FeatureFlagValueType): string {
  const formatted = valueType === "json" ? JSON.stringify(value) : formatValue(value, valueType)
  return formatted.length > 72 ? `${formatted.slice(0, 69)}...` : formatted
}

function parseValue(valueType: FeatureFlagValueType, valueText: string): JSONValue {
  if (valueType === "boolean") return valueText === "true"
  if (valueType === "number") {
    const parsed = Number(valueText)
    if (!Number.isFinite(parsed) || valueText.trim() === "") throw new Error("Enter a valid number")
    return parsed
  }
  if (valueType === "string") return valueText
  try {
    const parsed = JSON.parse(valueText) as JSONValue
    if (parsed === null || typeof parsed !== "object") {
      throw new Error("JSON values must be an object or array")
    }
    return parsed
  } catch (error) {
    if (error instanceof Error && error.message === "JSON values must be an object or array") throw error
    throw new Error("Enter valid JSON")
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

export function FeatureFlags() {
  const navigate = useNavigate()
  const flash = useFlash()
  const { refresh: refreshAccess } = useFeatureFlags()
  const currentUser = userInfo()
  const [flags, setFlags] = useState<AdminFeatureFlag[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [filteringText, setFilteringText] = useState("")
  const [editor, setEditor] = useState<EditorState | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const [deleteFlag, setDeleteFlag] = useState<AdminFeatureFlag | null>(null)
  const [deleteConfirmation, setDeleteConfirmation] = useState("")
  const [deleteError, setDeleteError] = useState<string | null>(null)
  const [deleteConflict, setDeleteConflict] = useState<AdminFeatureFlag | null>(null)
  const [selfRemovalPending, setSelfRemovalPending] = useState<JSONValue | null>(null)
  const [selfRemovalConfirmed, setSelfRemovalConfirmed] = useState(false)
  const [selfRemovalError, setSelfRemovalError] = useState<string | null>(null)
  const [valueDetails, setValueDetails] = useState<AdminFeatureFlag | null>(null)

  const clearAdminState = useCallback(() => {
    setFlags([])
    setError(null)
    setFilteringText("")
    setEditor(null)
    setDeleteFlag(null)
    setDeleteConfirmation("")
    setDeleteError(null)
    setDeleteConflict(null)
    setSelfRemovalPending(null)
    setSelfRemovalConfirmed(false)
    setSelfRemovalError(null)
    setValueDetails(null)
  }, [])

  const handleAuthorizationError = useCallback(async (requestError: unknown): Promise<boolean> => {
    if (!(requestError instanceof AdminFeatureFlagError) || requestError.code !== "not_admin") return false
    clearAdminState()
    try {
      await refreshAccess()
    } finally {
      navigate("/pools", { replace: true })
    }
    return true
  }, [clearAdminState, navigate, refreshAccess])

  const load = useCallback(async () => {
    setLoading(true)
    try {
      setFlags(await adminFeatureFlagsApi.list())
      setError(null)
    } catch (loadError) {
      if (!await handleAuthorizationError(loadError)) setError(errorMessage(loadError))
    } finally {
      setLoading(false)
    }
  }, [handleAuthorizationError])

  useEffect(() => {
    void load()
  }, [load])

  const filteredFlags = useMemo(() => {
    const filter = filteringText.trim().toLowerCase()
    if (!filter) return flags
    return flags.filter(flag =>
      [flag.key, compactValue(flag.value, flag.value_type), flag.value_type, flag.ownership, flag.managed_by]
        .filter(Boolean)
        .some(value => String(value).toLowerCase().includes(filter)),
    )
  }, [filteringText, flags])

  const { items: sortedFlags, collectionProps } = useCollection(filteredFlags, {
    sorting: {},
  })

  const openCreate = () => {
    setEditor({ mode: "create", key: "", valueType: "boolean", valueText: "false", description: "" })
  }

  const openEdit = (flag: AdminFeatureFlag) => {
    setEditor({
      mode: "edit",
      original: flag,
      key: flag.key,
      valueType: flag.value_type,
      valueText: formatValue(flag.value, flag.value_type),
      description: flag.description ?? "",
    })
  }

  const changeValueType = (valueType: FeatureFlagValueType) => {
    setEditor(current => {
      if (!current) return current
      const valueText = valueType === "boolean" ? "false" : valueType === "json" ? "{}" : ""
      return { ...current, valueType, valueText, error: undefined, errorField: undefined, conflict: undefined }
    })
  }

  const saveValue = async (value: JSONValue) => {
    if (!editor) return
    setSubmitting(true)
    try {
      if (editor.mode === "create") {
        await adminFeatureFlagsApi.create({
          key: editor.key.trim(),
          value_type: editor.valueType,
          value,
          ...(editor.description.trim() ? { description: editor.description.trim() } : {}),
        })
        flash.push({ type: "success", content: `Created ${editor.key.trim()}` })
      } else if (editor.original) {
        await adminFeatureFlagsApi.update(editor.original.key, {
          value_type: editor.valueType,
          value,
          expected_version: editor.original.version,
        })
        flash.push({ type: "success", content: `Updated ${editor.original.key}` })
      }
      const updatedAdminSubs = editor.mode === "edit" && editor.original?.key === "admin-subs"
      if (updatedAdminSubs) {
        const access = await refreshAccess()
        if (!access.admin) {
          clearAdminState()
          navigate("/pools", { replace: true })
          return
        }
      }
      setEditor(null)
      setSelfRemovalPending(null)
      setSelfRemovalConfirmed(false)
      setSelfRemovalError(null)
      await load()
    } catch (saveError) {
      if (await handleAuthorizationError(saveError)) return
      if (saveError instanceof AdminFeatureFlagError && saveError.code === "version_conflict") {
        if (selfRemovalPending !== null) {
          setSelfRemovalPending(null)
          setSelfRemovalConfirmed(false)
          setSelfRemovalError(null)
        }
        setEditor(current => current ? { ...current, conflict: saveError.current, error: undefined, errorField: undefined } : current)
      } else if (selfRemovalPending !== null) {
        setSelfRemovalError(errorMessage(saveError))
      } else {
        setEditor(current => current ? { ...current, error: errorMessage(saveError), errorField: undefined } : current)
      }
    } finally {
      setSubmitting(false)
    }
  }

  const submitEditor = async () => {
    if (!editor) return
    if (editor.mode === "create" && editor.key.trim().length > 63) {
      setEditor({
        ...editor,
        error: "Feature flag keys can be at most 63 characters",
        errorField: "key",
      })
      return
    }
    if (editor.mode === "create" && !/^[a-z0-9](?:[-a-z0-9]*[a-z0-9])?$/.test(editor.key.trim())) {
      setEditor({
        ...editor,
        error: "Use a lowercase DNS label with letters, numbers, or hyphens",
        errorField: "key",
      })
      return
    }
    let value: JSONValue
    try {
      value = parseValue(editor.valueType, editor.valueText)
    } catch (parseError) {
      setEditor({ ...editor, error: errorMessage(parseError), errorField: "value" })
      return
    }
    const removesCurrentUser =
      editor.mode === "edit" &&
      editor.original?.key === "admin-subs" &&
      editor.valueType === "json" &&
      Array.isArray(value) &&
      Boolean(currentUser.sub) &&
      !value.includes(currentUser.sub as JSONValue)
    if (removesCurrentUser && selfRemovalPending === null) {
      setSelfRemovalPending(value)
      setSelfRemovalConfirmed(false)
      setSelfRemovalError(null)
      return
    }
    await saveValue(value)
  }

  const reloadConflict = () => {
    if (!editor?.conflict) return
    const current = editor.conflict
    setEditor({
      mode: "edit",
      original: current,
      key: current.key,
      valueType: current.value_type,
      valueText: formatValue(current.value, current.value_type),
      description: current.description ?? "",
    })
  }

  const remove = async () => {
    if (!deleteFlag) return
    setSubmitting(true)
    setDeleteError(null)
    try {
      await adminFeatureFlagsApi.remove(deleteFlag.key, deleteFlag.version)
      flash.push({ type: "success", content: `Deleted ${deleteFlag.key}` })
      setDeleteFlag(null)
      setDeleteConfirmation("")
      setDeleteConflict(null)
      await load()
    } catch (removeError) {
      if (removeError instanceof AdminFeatureFlagError && removeError.code === "version_conflict" && removeError.current) {
        setDeleteConflict(removeError.current)
      } else {
        setDeleteError(errorMessage(removeError))
      }
    } finally {
      setSubmitting(false)
    }
  }

  const reloadDeleteConflict = () => {
    if (!deleteConflict) return
    setDeleteFlag(deleteConflict)
    setDeleteConflict(null)
    setDeleteError(null)
    setDeleteConfirmation("")
  }

  const editorValueControl = editor?.valueType === "boolean" ? (
    <Select
      ariaLabel="Value"
      selectedOption={BOOLEAN_OPTIONS.find(option => option.value === editor.valueText) ?? BOOLEAN_OPTIONS[1]}
      options={BOOLEAN_OPTIONS}
      onChange={({ detail }) => setEditor(current => current ? { ...current, valueText: detail.selectedOption.value ?? "false", error: undefined, errorField: undefined } : current)}
    />
  ) : editor?.valueType === "json" || editor?.valueType === "string" ? (
    <Textarea
      ariaLabel="Value"
      rows={10}
      value={editor.valueText}
      onChange={({ detail }) => setEditor(current => current ? { ...current, valueText: detail.value, error: undefined, errorField: undefined } : current)}
    />
  ) : (
    <Input
      ariaLabel="Value"
      type={editor?.valueType === "number" ? "number" : "text"}
      value={editor?.valueText ?? ""}
      onChange={({ detail }) => setEditor(current => current ? { ...current, valueText: detail.value, error: undefined, errorField: undefined } : current)}
    />
  )

  return (
    <>
      <Table
        {...collectionProps}
        trackBy="key"
        loading={loading}
        loadingText="Loading feature flags"
        items={sortedFlags}
        empty={
          <Box textAlign="center" padding="l">
            <SpaceBetween size="xs">
              <b>{filteringText ? "No matching feature flags" : "No feature flags"}</b>
              {filteringText ? (
                <Button onClick={() => setFilteringText("")}>Clear filter</Button>
              ) : (
                <Button onClick={openCreate}>Create feature flag</Button>
              )}
            </SpaceBetween>
          </Box>
        }
        header={
          <Header
            variant="h1"
            counter={loading ? undefined : `(${flags.length})`}
            info={<Badge color="blue">Admin only · Feature flagged</Badge>}
            actions={
              <SpaceBetween direction="horizontal" size="xs">
                <Button
                  iconName="refresh"
                  ariaLabel="Refresh feature flags"
                  disabled={loading}
                  onClick={() => void load()}
                />
                <Button variant="primary" onClick={openCreate}>Create feature flag</Button>
              </SpaceBetween>
            }
          >
            Feature flags
          </Header>
        }
        filter={
          <TextFilter
            filteringText={filteringText}
            filteringPlaceholder="Find feature flags"
            filteringClearAriaLabel="Clear filter"
            countText={`${filteredFlags.length} matches`}
            onChange={({ detail }) => setFilteringText(detail.filteringText)}
          />
        }
        columnDefinitions={[
          {
            id: "key",
            header: "Key",
            cell: flag => <code>{flag.key}</code>,
            isRowHeader: true,
            sortingField: "key",
          },
          {
            id: "value",
            header: "Value",
            cell: flag => (
              <SpaceBetween direction="horizontal" size="xs">
                <code>{compactValue(flag.value, flag.value_type)}</code>
                <Button
                  variant="inline-icon"
                  iconName="view-full"
                  ariaLabel={`View full value for ${flag.key}`}
                  onClick={() => setValueDetails(flag)}
                />
              </SpaceBetween>
            ),
          },
          { id: "type", header: "Type", cell: flag => typeLabel(flag.value_type), sortingField: "value_type" },
          {
            id: "ownership",
            header: "Ownership",
            sortingField: "ownership",
            cell: flag => flag.ownership === "terraform" ? (
              <Link external externalIconAriaLabel="Opens in a new tab" href={TERRAFORM_FEATURE_FLAGS_URL}>
                <Badge color="blue">Terraform</Badge>
              </Link>
            ) : <Badge color={flag.ownership === "ad_hoc" ? "green" : "grey"}>{ownershipLabel(flag.ownership)}</Badge>,
          },
          { id: "version", header: "Version", cell: flag => flag.version, sortingField: "version" },
          {
            id: "modified",
            header: "Modified",
            cell: flag => new Intl.DateTimeFormat("en-US", { dateStyle: "medium", timeStyle: "short" }).format(new Date(flag.modified_at)),
            sortingField: "modified_at",
          },
          {
            id: "actions",
            header: "Actions",
            cell: flag => (
              <ButtonDropdown
                ariaLabel={`Actions for ${flag.key}`}
                variant="icon"
                expandToViewport
                items={[
                  { id: "edit", text: "Edit" },
                  ...(flag.key !== "admin-subs" && flag.ownership === "ad_hoc" && flag.deletable
                    ? [{ id: "delete", text: "Delete" }]
                    : []),
                ]}
                onItemClick={({ detail }) => {
                  if (detail.id === "edit") openEdit(flag)
                  if (detail.id === "delete") {
                    setDeleteFlag(flag)
                    setDeleteConfirmation("")
                    setDeleteError(null)
                    setDeleteConflict(null)
                  }
                }}
              />
            ),
          },
        ]}
      />

      {error && (
        <Box margin={{ top: "m" }}>
          <Alert type="error" dismissible onDismiss={() => setError(null)}>{error}</Alert>
        </Box>
      )}

      <Modal
        visible={editor !== null}
        header={editor?.mode === "create" ? "Create feature flag" : `Edit ${editor?.key ?? "feature flag"}`}
        onDismiss={() => !submitting && setEditor(null)}
        footer={
          <Box float="right">
            <SpaceBetween direction="horizontal" size="xs">
              <Button disabled={submitting} onClick={() => setEditor(null)}>Cancel</Button>
              <Button variant="primary" loading={submitting} onClick={() => void submitEditor()}>
                {editor?.mode === "create" ? "Create" : "Save changes"}
              </Button>
            </SpaceBetween>
          </Box>
        }
      >
        {editor && (
          <Form errorText={editor.errorField ? undefined : editor.error}>
            <SpaceBetween size="m">
              {editor.conflict && (
                <Alert
                  type="warning"
                  action={<Button onClick={reloadConflict}>Reload current value</Button>}
                >
                  This flag changed after you loaded it.
                </Alert>
              )}
              <FormField label="Key" errorText={editor.errorField === "key" ? editor.error : undefined}>
                <Input
                  value={editor.key}
                  disabled={editor.mode === "edit"}
                  onChange={({ detail }) => setEditor(current => current ? { ...current, key: detail.value, error: undefined, errorField: undefined } : current)}
                />
              </FormField>
              <FormField label="Value type">
                <Select
                  selectedOption={VALUE_TYPE_OPTIONS.find(option => option.value === editor.valueType) ?? null}
                  options={VALUE_TYPE_OPTIONS}
                  onChange={({ detail }) => changeValueType(detail.selectedOption.value as FeatureFlagValueType)}
                />
              </FormField>
              <FormField label="Value" errorText={editor.errorField === "value" ? editor.error : undefined}>{editorValueControl}</FormField>
              {editor.mode === "create" && (
                <FormField label="Description (optional)">
                  <Textarea
                    value={editor.description}
                    onChange={({ detail }) => setEditor(current => current ? { ...current, description: detail.value } : current)}
                  />
                </FormField>
              )}
              {editor.mode === "edit" && editor.original?.description && (
                <FormField label="Description">
                  <Box>{editor.original.description}</Box>
                </FormField>
              )}
              {editor.mode === "edit" && editor.original && (
                <Box color="text-body-secondary">Current version: {editor.original.version}</Box>
              )}
              {editor.mode === "edit" && editor.original?.ownership !== "ad_hoc" && (
                <StatusIndicator type="info">The key is protected; only its typed value can be changed.</StatusIndicator>
              )}
              {editor.mode === "edit" && editor.original?.ownership === "external" && (
                <StatusIndicator type="info">External flags cannot be deleted here.</StatusIndicator>
              )}
            </SpaceBetween>
          </Form>
        )}
      </Modal>

      <Modal
        visible={deleteFlag !== null}
        header={`Delete ${deleteFlag?.key ?? "feature flag"}?`}
        onDismiss={() => {
          if (!submitting) {
            setDeleteFlag(null)
            setDeleteError(null)
            setDeleteConflict(null)
          }
        }}
        footer={
          <Box float="right">
            <SpaceBetween direction="horizontal" size="xs">
              <Button disabled={submitting} onClick={() => setDeleteFlag(null)}>Cancel</Button>
              <Button
                variant="primary"
                loading={submitting}
                disabled={deleteConfirmation !== deleteFlag?.key}
                onClick={() => void remove()}
              >
                Delete feature flag
              </Button>
            </SpaceBetween>
          </Box>
        }
      >
        <SpaceBetween size="m">
          <Alert type="warning">This action cannot be undone.</Alert>
          <StatusIndicator type="info">
            Deleting this flag causes consumers to receive defaults after the feature flag cache refreshes.
          </StatusIndicator>
          {deleteConflict && (
            <Alert
              type="warning"
              action={<Button onClick={reloadDeleteConflict}>Reload current version</Button>}
            >
              This flag changed after you loaded it.
            </Alert>
          )}
          {deleteError && <Alert type="error">{deleteError}</Alert>}
          {deleteFlag && <Box color="text-body-secondary">Current version: {deleteFlag.version}</Box>}
          <FormField label={`Type ${deleteFlag?.key ?? "the key"} to confirm`}>
            <Input value={deleteConfirmation} onChange={({ detail }) => setDeleteConfirmation(detail.value)} />
          </FormField>
        </SpaceBetween>
      </Modal>

      <Modal
        visible={selfRemovalPending !== null}
        header="Confirm loss of administrator access"
        onDismiss={() => {
          if (!submitting) {
            setSelfRemovalPending(null)
            setSelfRemovalError(null)
          }
        }}
        footer={
          <Box float="right">
            <SpaceBetween direction="horizontal" size="xs">
              <Button disabled={submitting} onClick={() => setSelfRemovalPending(null)}>Cancel</Button>
              <Button
                variant="primary"
                loading={submitting}
                disabled={!selfRemovalConfirmed}
                onClick={() => selfRemovalPending !== null && void saveValue(selfRemovalPending)}
              >
                Confirm and save
              </Button>
            </SpaceBetween>
          </Box>
        }
      >
        <SpaceBetween size="m">
          <Alert type="warning">You are removing your own administrator access.</Alert>
          {selfRemovalError && <Alert type="error">{selfRemovalError}</Alert>}
          <Checkbox checked={selfRemovalConfirmed} onChange={({ detail }) => setSelfRemovalConfirmed(detail.checked)}>
            I understand I will lose access
          </Checkbox>
        </SpaceBetween>
      </Modal>

      <Modal
        visible={valueDetails !== null}
        header={`Value for ${valueDetails?.key ?? "feature flag"}`}
        onDismiss={() => setValueDetails(null)}
        footer={
          <Box float="right">
            <Button onClick={() => setValueDetails(null)}>Close</Button>
          </Box>
        }
      >
        {valueDetails && (
          <SpaceBetween size="m">
            <Box variant="code" padding="m">
              <pre style={{ margin: 0, whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>
                {formatValue(valueDetails.value, valueDetails.value_type)}
              </pre>
            </Box>
            <CopyToClipboard
              variant="button"
              copyButtonText="Copy full value"
              copyButtonAriaLabel="Copy full value"
              textToCopy={formatValue(valueDetails.value, valueDetails.value_type)}
              copySuccessText="Feature flag value copied"
              copyErrorText="Failed to copy feature flag value"
            />
          </SpaceBetween>
        )}
      </Modal>
    </>
  )
}

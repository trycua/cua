import { useEffect, useState } from "react"
import { useNavigate } from "react-router-dom"
import { useCollection } from "@cloudscape-design/collection-hooks"
import Box from "@cloudscape-design/components/box"
import Link from "@cloudscape-design/components/link"
import Modal from "@cloudscape-design/components/modal"
import designTokens from "@cua/design/tokens.json"
import Pagination from "@cloudscape-design/components/pagination"
import PropertyFilter from "@cloudscape-design/components/property-filter"
import SpaceBetween from "@cloudscape-design/components/space-between"
import Table from "@cloudscape-design/components/table"
import { deletePool, getPool, listPools } from "../fleet/pools"
import { localVisualPreviewPath } from "../local-visual-preview"
import { tombstonePool } from "../fleet/status"
import { PoolStatusPill } from "../components/PoolStatus"
import { useFlash } from "../components/FlashContext"
import { CuaButton } from "../components/CuaButton"
import { PageEmpty } from "../components/PageState"
import { PageShell } from "../components/PageShell"
import type { PoolSummary } from "../fleet/models"
import { formatTtl } from "../fleet/ttl"

interface PoolRow {
  name: string
  namespace: string
  replicas: number
  ttlSecondsAfterCreated?: number
  status: PoolSummary["status"]
  statusText: string
}

export function PoolsList() {
  const navigate = useNavigate()
  const flash = useFlash()
  const [rows, setRows] = useState<PoolRow[]>([])
  const [loading, setLoading] = useState(true)
  const [selected, setSelected] = useState<PoolRow[]>([])
  const [busy, setBusy] = useState(false)
  const [duplicating, setDuplicating] = useState(false)
  const [confirmDeleteOpen, setConfirmDeleteOpen] = useState(false)
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

  const load = async () => {
    setLoading(true)
    try {
      const poolList = await listPools()
      setRows(
        poolList.map(p => ({
          ...p,
          statusText: p.status.label,
        })),
      )
    } catch (e) {
      flash.push({
        type: "error",
        header: "Failed to list pools",
        content: String((e as Error).message),
      })
    } finally {
      setLoading(false)
    }
  }
  useEffect(() => {
    load()
  }, [])

  const removeSelected = async () => {
    if (selected.length === 0) return
    setBusy(true)
    try {
      await Promise.all(selected.map(p => deletePool(p.namespace, p.name)))
      for (const p of selected) tombstonePool(p.name)
      flash.push({
        type: "success",
        header: `Deleted ${selected.length} pool${selected.length > 1 ? "s" : ""}`,
      })
      setSelected([])
      setConfirmDeleteOpen(false)
      await load()
    } catch (e) {
      flash.push({
        type: "error",
        header: "Delete failed",
        content: String((e as Error).message),
      })
    } finally {
      setBusy(false)
    }
  }

  // A list row only carries summary fields,
  // so fetch the full pool spec (cpu/ram/image/services) before navigating —
  // otherwise the New pool form falls back to defaults. Mirrors the Duplicate
  // action on the pool detail page, which passes the full getPool result.
  const duplicate = async () => {
    const src = selected[0]
    if (!src) return
    setDuplicating(true)
    try {
      const full = await getPool(src.namespace, src.name)
      navigate("/pools/new", { state: { source: full } })
    } catch (e) {
      flash.push({
        type: "error",
        header: "Failed to load pool for duplication",
        content: String((e as Error).message),
      })
    } finally {
      setDuplicating(false)
    }
  }

  const {
    items,
    collectionProps,
    propertyFilterProps,
    filteredItemsCount,
    paginationProps,
  } = useCollection(rows, {
    propertyFiltering: {
      filteringProperties: [
        { key: "name",       propertyLabel: "Name",       groupValuesLabel: "Name values",       operators: [":", "!:", "=", "!=", "^"] },
        { key: "replicas",   propertyLabel: "Replicas",   groupValuesLabel: "Replica counts",    operators: ["=", "!=", "<", "<=", ">", ">="] },
        { key: "statusText", propertyLabel: "Status",     groupValuesLabel: "Status values",     operators: ["=", "!="] },
      ],
      empty: (
        <PageEmpty
          title="No pools"
          action={
            <CuaButton tone="primary" onClick={() => navigate("/pools/new")}>
              New pool
            </CuaButton>
          }
        >
          Create one to get started.
        </PageEmpty>
      ),
      noMatch: (
        <PageEmpty title="No matches" />
      ),
    },
    pagination: { pageSize: 25 },
    sorting: {},
  })

  return (
    <PageShell
      eyebrow="Fleet"
      title="Pools"
      counter={loading ? undefined : `(${rows.length})`}
      secondaryActions={
        <SpaceBetween direction="horizontal" size="xs">
          <CuaButton
            tone="icon"
            ariaLabel="Refresh pools"
            iconName="refresh"
            onClick={load}
            disabled={loading}
          />
        </SpaceBetween>
      }
      primaryAction={
        <CuaButton
          tone="primary"
          onClick={() => navigate(localVisualPreviewPath("/pools/new"))}
        >
          New pool
        </CuaButton>
      }
    >
      {selected.length > 0 ? (
        <div className="cua-selection-actions" aria-live="polite">
          <span>
            {selected.length} pool{selected.length === 1 ? "" : "s"} selected
          </span>
          <SpaceBetween direction="horizontal" size="xs">
            <CuaButton
              disabled={selected.length !== 1 || duplicating}
              loading={duplicating}
              onClick={duplicate}
            >
              Duplicate
            </CuaButton>
            <CuaButton
              disabled={busy}
              loading={busy}
              onClick={() => setConfirmDeleteOpen(true)}
            >
              Delete
            </CuaButton>
          </SpaceBetween>
        </div>
      ) : null}
      <section
        className="cua-page-table"
        aria-label="Pools table"
      >
        <Table
        {...collectionProps}
        ariaLabels={{
          tableLabel: "Pools",
          selectionGroupLabel: "Pool selection",
          allItemsSelectionLabel: ({ selectedItems }) =>
            selectedItems.length === items.length && items.length > 0
              ? "Deselect all pools"
              : "Select all pools",
          itemSelectionLabel: ({ selectedItems }, item) =>
            selectedItems.includes(item)
              ? `Deselect pool ${item.name}`
              : `Select pool ${item.name}`,
        }}
        variant="borderless"
        items={items}
        loading={loading}
        loadingText="Loading pools"
        selectionType="multi"
        trackBy={(p) => `${p.namespace}/${p.name}`}
        selectedItems={selected}
        onSelectionChange={({ detail }) => setSelected(detail.selectedItems)}
        filter={
          <PropertyFilter
            {...propertyFilterProps}
            countText={`${filteredItemsCount} match${filteredItemsCount === 1 ? "" : "es"}`}
            i18nStrings={{
              filteringAriaLabel: "Filter pools",
              filteringPlaceholder: "Filter pools by property",
              operationAndText: "and",
              operationOrText: "or",
              clearFiltersText: "Clear filters",
              applyActionText: "Apply",
              cancelActionText: "Cancel",
            }}
          />
        }
        pagination={
          <Pagination
            {...paginationProps}
            ariaLabels={{
              paginationLabel: "Pools pagination",
              previousPageLabel: "Previous page",
              nextPageLabel: "Next page",
              pageLabel: pageNumber => `Page ${pageNumber}`,
            }}
          />
        }
        stickyColumns={{ first: 1, last: 1 }}
        columnDisplay={[
          { id: "name", visible: true },
          { id: "replicas", visible: !compactTable },
          { id: "ttl", visible: !compactTable },
          { id: "status", visible: true },
        ]}
        columnDefinitions={[
          {
            id: "name",
            header: "Name",
            cell: p => (
              <Link
                href={`#${localVisualPreviewPath(`/pools/${p.namespace}/${p.name}`)}`}
                onFollow={e => {
                  e.preventDefault()
                  navigate(
                    localVisualPreviewPath(`/pools/${p.namespace}/${p.name}`),
                  )
                }}
              >
                <span className="cua-pool-name" title={p.name}>{p.name}</span>
              </Link>
            ),
            sortingField: "name",
            isRowHeader: true,
            minWidth: compactTable ? 112 : 220,
            width: compactTable ? 120 : undefined,
            maxWidth: compactTable ? 120 : undefined,
          },
          {
            id: "replicas",
            header: "Replicas",
            cell: p => p.replicas,
            sortingField: "replicas",
            minWidth: 96,
          },
          {
            id: "ttl",
            header: "TTL",
            cell: p => formatTtl(p.ttlSecondsAfterCreated),
            sortingField: "ttlSecondsAfterCreated",
            minWidth: 96,
          },
          {
            id: "status",
            header: "Status",
            cell: p => (
              <PoolStatusPill
                status={p.status}
                compact={compactTable}
              />
            ),
            sortingField: compactTable ? undefined : "statusText",
            minWidth: compactTable ? 60 : 140,
            width: compactTable ? 60 : undefined,
            maxWidth: compactTable ? 60 : undefined,
          },
        ]}
        />
      </section>
      <Modal
        visible={confirmDeleteOpen}
        onDismiss={() => setConfirmDeleteOpen(false)}
        header={`Delete ${selected.length} pool${selected.length === 1 ? "" : "s"}?`}
        footer={
          <Box float="right">
            <SpaceBetween direction="horizontal" size="xs">
              <CuaButton onClick={() => setConfirmDeleteOpen(false)}>
                Cancel
              </CuaButton>
              <CuaButton tone="danger" onClick={removeSelected} loading={busy}>
                Delete
              </CuaButton>
            </SpaceBetween>
          </Box>
        }
      >
        This action cannot be undone.
      </Modal>
    </PageShell>
  )
}

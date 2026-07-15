import { useEffect, useState } from "react"
import { useNavigate } from "react-router-dom"
import { useCollection } from "@cloudscape-design/collection-hooks"
import Box from "@cloudscape-design/components/box"
import Button from "@cloudscape-design/components/button"
import Header from "@cloudscape-design/components/header"
import Link from "@cloudscape-design/components/link"
import Pagination from "@cloudscape-design/components/pagination"
import PropertyFilter from "@cloudscape-design/components/property-filter"
import SpaceBetween from "@cloudscape-design/components/space-between"
import Table from "@cloudscape-design/components/table"
import { api } from "../api/cyclops"
import {
  derivePoolStatus,
  reconcileTombstones,
  tombstonePool,
} from "../api/pools"
import { PoolStatusPill } from "../components/PoolStatus"
import { useFlash } from "../components/FlashContext"

interface PoolRow {
  name: string
  namespace: string
  replicas: number
  phase: string
  availableCount: number
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

  const load = async () => {
    setLoading(true)
    try {
      const poolList = await api.listPools()
      reconcileTombstones(poolList.map(p => p.name))
      setRows(
        poolList.map(p => ({
          ...p,
          statusText: derivePoolStatus(p).kind,
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
      await Promise.all(selected.map(p => api.deletePool(p.namespace, p.name)))
      for (const p of selected) tombstonePool(p.name)
      flash.push({
        type: "success",
        header: `Deleted ${selected.length} pool${selected.length > 1 ? "s" : ""}`,
      })
      setSelected([])
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

  // A list row only carries summary fields (name/replicas/phase/available),
  // so fetch the full pool spec (cpu/ram/image/services) before navigating —
  // otherwise the New pool form falls back to defaults. Mirrors the Duplicate
  // action on the pool detail page, which passes the full getPool result.
  const duplicate = async () => {
    const src = selected[0]
    if (!src) return
    setDuplicating(true)
    try {
      const full = await api.getPool(src.namespace, src.name)
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
    actions,
  } = useCollection(rows, {
    propertyFiltering: {
      filteringProperties: [
        { key: "name",       propertyLabel: "Name",       groupValuesLabel: "Name values",       operators: [":", "!:", "=", "!=", "^"] },
        { key: "replicas",   propertyLabel: "Replicas",   groupValuesLabel: "Replica counts",    operators: ["=", "!=", "<", "<=", ">", ">="] },
        { key: "statusText", propertyLabel: "Status",     groupValuesLabel: "Status values",     operators: ["=", "!="] },
      ],
      empty: (
        <Box textAlign="center" padding="m">
          <SpaceBetween size="xs">
            <b>No pools</b>
            <Box variant="p" color="text-body-secondary">
              Create one to get started.
            </Box>
            <Button onClick={() => navigate("/pools/new")}>New pool</Button>
          </SpaceBetween>
        </Box>
      ),
      noMatch: (
        <Box textAlign="center" color="text-body-secondary" padding="m">
          <SpaceBetween size="xs">
            <b>No matches</b>
            <Button onClick={() => actions.setPropertyFiltering({ tokens: [], operation: "and" })}>
              Clear filter
            </Button>
          </SpaceBetween>
        </Box>
      ),
    },
    pagination: { pageSize: 25 },
    sorting: {},
  })

  return (
      <Table
        {...collectionProps}
        header={
          <Header
            variant="h1"
            counter={loading ? undefined : `(${rows.length})`}
            actions={
              <SpaceBetween direction="horizontal" size="xs">
                <Button iconName="refresh" onClick={load} disabled={loading} />
                <Button
                  disabled={selected.length !== 1 || duplicating}
                  loading={duplicating}
                  onClick={duplicate}
                >
                  Duplicate
                </Button>
                <Button
                  disabled={selected.length === 0 || busy}
                  loading={busy}
                  onClick={removeSelected}
                >
                  Delete
                </Button>
                <Button
                  variant="primary"
                  onClick={() => navigate("/pools/new")}
                >
                  New pool
                </Button>
              </SpaceBetween>
            }
          >
            Pools
          </Header>
        }
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
        pagination={<Pagination {...paginationProps} />}
        columnDefinitions={[
          {
            id: "name",
            header: "Name",
            cell: p => (
              <Link
                href={`#/pools/${p.namespace}/${p.name}`}
                onFollow={e => {
                  e.preventDefault()
                  navigate(`/pools/${p.namespace}/${p.name}`)
                }}
              >
                {p.name}
              </Link>
            ),
            sortingField: "name",
            isRowHeader: true,
          },
          {
            id: "replicas",
            header: "Replicas",
            cell: p => p.replicas,
            sortingField: "replicas",
          },
          {
            id: "available",
            header: "Available",
            cell: p => p.availableCount,
            sortingField: "availableCount",
          },
          {
            id: "status",
            header: "Status",
            cell: p => <PoolStatusPill status={derivePoolStatus(p)} />,
            sortingField: "statusText",
          },
        ]}
      />
  )
}

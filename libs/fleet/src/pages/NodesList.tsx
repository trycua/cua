import { useEffect, useState } from "react"
import Box from "@cloudscape-design/components/box"
import Button from "@cloudscape-design/components/button"
import Header from "@cloudscape-design/components/header"
import Table from "@cloudscape-design/components/table"
import {
  api,
  isPoolWorkerNode,
  NodeSummary,
  parseCpuCores,
  parseMemoryBytes,
  POOL_WORKER_NODE_PREFIX,
} from "../api/cyclops"
import { useFlash } from "../components/FlashContext"

function readyCondition(n: NodeSummary): string {
  const c = (n.status?.conditions ?? []).find(c => c.type === "Ready")
  if (!c) return "Unknown"
  return c.status === "True" ? "Ready" : "NotReady"
}

function fmtMemGi(s?: string): string {
  if (!s) return "—"
  const b = parseMemoryBytes(s)
  return `${(b / 1024 ** 3).toFixed(1)}Gi`
}

interface NodeRow {
  node: NodeSummary
  totalVms: number
}

export function NodesList() {
  const flash = useFlash()
  const [rows, setRows] = useState<NodeRow[]>([])
  const [loading, setLoading] = useState(true)

  const load = async () => {
    setLoading(true)
    try {
      const [nodes, pods, pools] = await Promise.all([
        api.listNodes(),
        api.listVmPods(),
        api.listPools().catch(() => []),
      ])

      const knownPoolNs = new Set(
        pools.map(p => `pool-${p.name}`),
      )

      const byNode = new Map<string, number>()
      for (const p of pods.items) {
        const node = p.spec?.nodeName
        if (!node) continue
        const ns = p.metadata.namespace
        if (!knownPoolNs.has(ns)) continue
        byNode.set(node, (byNode.get(node) ?? 0) + 1)
      }

      setRows(
        nodes.items.filter(isPoolWorkerNode).map(n => ({
          node: n,
          totalVms: byNode.get(n.metadata.name) ?? 0,
        })),
      )
    } catch (e) {
      flash.push({
        type: "error",
        header: "Failed to list nodes",
        content: String((e as Error).message),
      })
    } finally {
      setLoading(false)
    }
  }
  useEffect(() => {
    load()
  }, [])

  const totalVms = rows.reduce((s, r) => s + r.totalVms, 0)

  return (
    <Table
      header={
        <Header
          variant="h1"
          counter={loading ? undefined : `(${rows.length})`}
          description={
            loading
              ? undefined
              : `${totalVms} pool VM${totalVms === 1 ? "" : "s"} scheduled across ${rows.length} node${rows.length === 1 ? "" : "s"}.`
          }
          actions={
            <Button iconName="refresh" onClick={load} disabled={loading} />
          }
        >
          Nodes
        </Header>
      }
      items={rows}
      loading={loading}
      loadingText="Loading nodes"
      trackBy={r => r.node.metadata.name}
      empty={
        <Box textAlign="center" padding="m" color="text-body-secondary">
          No nodes
        </Box>
      }
      columnDefinitions={[
        {
          id: "name",
          header: "Name",
          cell: r => (
            <code title={r.node.metadata.name}>
              {r.node.metadata.name.startsWith(POOL_WORKER_NODE_PREFIX)
                ? r.node.metadata.name.slice(POOL_WORKER_NODE_PREFIX.length)
                : r.node.metadata.name}
            </code>
          ),
          sortingField: "node.metadata.name",
          isRowHeader: true,
        },
        {
          id: "status",
          header: "Status",
          cell: r => readyCondition(r.node),
        },
        {
          id: "cpu",
          header: "CPU (alloc.)",
          cell: r => {
            const c = r.node.status?.allocatable?.cpu
            return c ? parseCpuCores(c).toFixed(1) : "—"
          },
        },
        {
          id: "mem",
          header: "Memory (alloc.)",
          cell: r => fmtMemGi(r.node.status?.allocatable?.memory),
        },
        {
          id: "vms",
          header: "Pool VMs",
          cell: r => r.totalVms,
        },
      ]}
    />
  )
}

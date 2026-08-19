import Alert from "@cloudscape-design/components/alert"
import BarChart from "@cloudscape-design/components/bar-chart"
import Box from "@cloudscape-design/components/box"
import Button from "@cloudscape-design/components/button"
import Container from "@cloudscape-design/components/container"
import FormField from "@cloudscape-design/components/form-field"
import Header from "@cloudscape-design/components/header"
import Input from "@cloudscape-design/components/input"
import LineChart from "@cloudscape-design/components/line-chart"
import SegmentedControl from "@cloudscape-design/components/segmented-control"
import Select from "@cloudscape-design/components/select"
import SpaceBetween from "@cloudscape-design/components/space-between"
import Spinner from "@cloudscape-design/components/spinner"
import StatusIndicator from "@cloudscape-design/components/status-indicator"
import { useEffect, useMemo, useState } from "react"
import { useSearchParams } from "react-router-dom"
import { useFeatureFlags } from "../components/FeatureFlagContext"
import { PageShell } from "../components/PageShell"
import {
  usageApi,
  type UsageOverviewResponse,
  type UsagePoolDetailResponse,
  type UsageTimeframe,
} from "../sdk/usage"
type Metric = "cpu" | "memory"
const opts = [
  { id: "24h", text: "24 hours" },
  { id: "7d", text: "7 days" },
  { id: "30d", text: "30 days" },
]
const i18n = {
  filterLabel: "Filter displayed data",
  filterPlaceholder: "Filter data",
  filterSelectedAriaLabel: "selected",
  detailPopoverDismissAriaLabel: "Dismiss",
  legendAriaLabel: "Legend",
  chartAriaRoleDescription: "chart",
}
const util = (c: number, p: number) => (p > 0 ? Math.round((c / p) * 100) : 0)
const stale = (d: string, t: UsageTimeframe) =>
  Date.now() - new Date(d).getTime() > (t === "24h" ? 3 : 30) * 3600000
export function Usage() {
  const { admin } = useFeatureFlags()
  const [sp, setSp] = useSearchParams()
  const raw = sp.get("timeframe")
  const timeframe: UsageTimeframe = raw === "7d" || raw === "30d" ? raw : "24h"
  const subject = sp.get("subject") ?? ""
  const [input, setInput] = useState(subject)
  const [metric, setMetric] = useState<Metric>("cpu")
  const [overview, setOverview] = useState<UsageOverviewResponse | null>(null)
  const [detail, setDetail] = useState<UsagePoolDetailResponse | null>(null)
  const [pool, setPool] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [detailLoading, setDetailLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let active = true
    setLoading(true)
    setError(null)
    usageApi
      .overview(timeframe, subject || undefined)
      .then((v) => {
        if (active) {
          setOverview(v)
          setPool((p) =>
            v.pools.some((x) => x.id === p) ? p : (v.pools[0]?.id ?? null),
          )
        }
      })
      .catch((e) => {
        if (active) {
          setOverview(null)
          setPool(null)
          setError(e instanceof Error ? e.message : "Usage data is unavailable")
        }
      })
      .finally(() => {
        if (active) setLoading(false)
      })
    return () => {
      active = false
    }
  }, [timeframe, subject])
  useEffect(() => {
    if (!pool) {
      setDetail(null)
      return
    }
    let active = true
    setDetailLoading(true)
    usageApi
      .pool(timeframe, pool, subject || undefined)
      .then((v) => {
        if (active) setDetail(v)
      })
      .catch((e) => {
        if (active)
          setError(e instanceof Error ? e.message : "Pool usage is unavailable")
      })
      .finally(() => {
        if (active) setDetailLoading(false)
      })
    return () => {
      active = false
    }
  }, [timeframe, subject, pool])
  const series = useMemo(
    () => [
      {
        title: "Consumed",
        type: "bar" as const,
        color: "#0972d3",
        data: (overview?.pools ?? []).map((p) => ({
          x: p.name,
          y: p[metric].consumed,
        })),
      },
      {
        title: "Provisioned capacity",
        type: "bar" as const,
        color: "#9dc7ea",
        data: (overview?.pools ?? []).map((p) => ({
          x: p.name,
          y: p[metric].provisioned,
        })),
      },
    ],
    [overview, metric],
  )
  const selected = overview?.pools.find((p) => p.id === pool)
  const unit = metric === "cpu" ? "CPU core-hours" : "Memory GiB-hours"
  const changeTime = (v: UsageTimeframe) => {
    const n = new URLSearchParams(sp)
    n.set("timeframe", v)
    setSp(n)
  }
  const view = () => {
    const n = new URLSearchParams(sp),
      v = input.trim()
    v ? n.set("subject", v) : n.delete("subject")
    setSp(n)
  }
  return (
    <PageShell
      eyebrow="Resources"
      title="Usage preview"
      description="Preview resource consumption and provisioned capacity. No pricing or payment data is shown."
      actions={
        <SegmentedControl
          selectedId={timeframe}
          onChange={(e) => changeTime(e.detail.selectedId as UsageTimeframe)}
          options={opts}
          label="Usage timeframe"
        />
      }
    >
      <SpaceBetween size="l">
        {admin && (
          <Container header={<Header variant="h2">Administrator view</Header>}>
            <SpaceBetween size="s">
              <FormField
                label="View usage for subject"
                description="The signed-in administrator remains the real actor; this selection only scopes usage reads."
              >
                <SpaceBetween direction="horizontal" size="xs">
                  <Input
                    value={input}
                    onChange={(e) => setInput(e.detail.value)}
                  />
                  <Button onClick={view}>View usage</Button>
                </SpaceBetween>
              </FormField>
              {subject && (
                <Alert type="info">Viewing usage for {subject}</Alert>
              )}
            </SpaceBetween>
          </Container>
        )}
        {loading && (
          <Box textAlign="center">
            <Spinner /> Loading usage data
          </Box>
        )}
        {error && (
          <Alert type="error" header="Usage data is unavailable">
            {error}
          </Alert>
        )}
        {!loading && overview && (
          <SpaceBetween size="l">
            <Container
              header={
                <Header
                  variant="h2"
                  description="Consumed values use measured usage; provisioned values use Kubernetes requests observed by OpenCost, grouped by pool for the selected timeframe."
                >
                  Overview
                </Header>
              }
            >
              <SpaceBetween size="m">
                <SegmentedControl
                  selectedId={metric}
                  onChange={(e) => setMetric(e.detail.selectedId as Metric)}
                  options={[
                    { id: "cpu", text: "CPU" },
                    { id: "memory", text: "Memory" },
                  ]}
                  label="Usage metric"
                />
                <Box variant="h3">{unit}</Box>
                {overview.partial && (
                  <StatusIndicator type="warning">
                    Data is partial
                  </StatusIndicator>
                )}
                {stale(overview.data_as_of, timeframe) && (
                  <StatusIndicator type="warning">
                    Usage data may be stale
                  </StatusIndicator>
                )}
                <Box color="text-body-secondary">
                  Data as of {new Date(overview.data_as_of).toLocaleString()}
                </Box>
                {overview.pools.length === 0 ? (
                  <Box textAlign="center" color="text-body-secondary">
                    No usage data is available for this timeframe.
                  </Box>
                ) : (
                  <>
                    <BarChart
                      series={series}
                      xScaleType="categorical"
                      xDomain={overview.pools.map((p) => p.name)}
                      yTitle={unit}
                      ariaLabel={`Usage overview in ${unit}`}
                      ariaDescription="Each pool has separately labeled consumed and provisioned capacity bars."
                      i18nStrings={i18n}
                      hideFilter
                    />
                    {overview.pools.map((p) => {
                      const v = p[metric]
                      return (
                        <Box key={p.id} color="text-body-secondary">
                          {p.name}: {v.consumed} consumed, {v.provisioned}{" "}
                          provisioned, {util(v.consumed, v.provisioned)}%
                          utilization
                        </Box>
                      )
                    })}
                  </>
                )}
              </SpaceBetween>
            </Container>
            {overview.pools.length > 0 && (
              <Container
                header={
                  <Header variant="h2">
                    {selected?.name ?? "Pool"} drilldown
                  </Header>
                }
              >
                <SpaceBetween size="l">
                  <FormField label="Pool drilldown">
                    <Select
                      selectedOption={
                        selected
                          ? { label: selected.name, value: selected.id }
                          : null
                      }
                      onChange={(e) =>
                        setPool(e.detail.selectedOption.value ?? null)
                      }
                      options={overview.pools.map((p) => ({
                        label: p.name,
                        value: p.id,
                      }))}
                    />
                  </FormField>
                  {detailLoading && (
                    <Box>
                      <Spinner /> Loading pool usage
                    </Box>
                  )}
                  {detail && !detailLoading && (
                    <>
                      <UsageLine
                        title="CPU core-hours per bucket"
                        consumed={detail.buckets.map((b) => ({
                          x: new Date(b.start),
                          y: b.cpu_consumed,
                        }))}
                        provisioned={detail.buckets.map((b) => ({
                          x: new Date(b.start),
                          y: b.cpu_provisioned,
                        }))}
                      />
                      <UsageLine
                        title="Memory GiB-hours per bucket"
                        consumed={detail.buckets.map((b) => ({
                          x: new Date(b.start),
                          y: b.memory_consumed,
                        }))}
                        provisioned={detail.buckets.map((b) => ({
                          x: new Date(b.start),
                          y: b.memory_provisioned,
                        }))}
                      />
                    </>
                  )}
                </SpaceBetween>
              </Container>
            )}
          </SpaceBetween>
        )}
      </SpaceBetween>
    </PageShell>
  )
}
function UsageLine({
  title,
  consumed,
  provisioned,
}: {
  title: string
  consumed: { x: Date; y: number }[]
  provisioned: { x: Date; y: number }[]
}) {
  return (
    <SpaceBetween size="s">
      <Box variant="h3">{title}</Box>
      <LineChart
        series={[
          { title: "Consumed", type: "line", color: "#0972d3", data: consumed },
          {
            title: "Provisioned capacity",
            type: "line",
            color: "#9dc7ea",
            data: provisioned,
          },
        ]}
        xScaleType="time"
        xTitle="Bucket start"
        yTitle={title}
        ariaLabel={title}
        ariaDescription="Consumed values use measured usage; provisioned values use Kubernetes requests observed by OpenCost."
        i18nStrings={i18n}
        hideFilter
      />
    </SpaceBetween>
  )
}

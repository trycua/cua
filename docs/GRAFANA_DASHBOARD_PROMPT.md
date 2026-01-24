# Grafana Dashboard Generation Prompt

Use this prompt with your Grafana agent to automatically generate dashboards based on the Four Golden Signals telemetry data collected from the CUA services.

## Data Sources

The metrics are exported to `otel.cua.ai` via OTLP and should be available in your Prometheus/Mimir data source. Traces go to Tempo, and you can correlate with Loki for logs.

---

## Prompt for Grafana Agent

```
Create comprehensive Grafana dashboards for the CUA (Computer Use Agent) platform monitoring the Four Golden Signals: Latency, Traffic, Errors, and Saturation.

## Services to Monitor

There are three main services exporting OTEL metrics:

1. **cua-docs** (Next.js documentation site with CopilotKit AI assistant)
2. **cua-mcp-server** (Python MCP server for computer control)
3. **cua-docs-indexer** (Modal-based documentation and code indexing service)

## Metric Naming Conventions

All metrics follow the pattern: `{service_prefix}.{signal_category}.{metric_name}`

### CUA Docs (cua-docs) - Prefix: `cua.docs`

**Latency Metrics (Histograms):**
- `cua.docs.request.duration` - HTTP request duration (ms), labels: method, route, status_code
- `cua.docs.copilot.response_time` - CopilotKit response time (ms), labels: category, question_type, response_type, has_code_snippet
- `cua.docs.tool.execution_time` - MCP tool execution time (ms), labels: tool_name, success

**Traffic Metrics (Counters):**
- `cua.docs.requests.total` - Total HTTP requests, labels: method, route, status_code
- `cua.docs.copilot.messages.total` - CopilotKit messages processed, labels: category, question_type, response_type
- `cua.docs.tool.calls.total` - MCP tool calls, labels: tool_name, success

**Error Metrics (Counters):**
- `cua.docs.errors.total` - Total errors, labels: method, route, error_type
- `cua.docs.copilot.errors.total` - CopilotKit errors, labels: category, error_type
- `cua.docs.tool.errors.total` - Tool errors, labels: tool_name, error_type

**Saturation Metrics (Gauges):**
- `cua.docs.requests.concurrent` - Current concurrent requests
- `cua.docs.copilot.queue_depth` - CopilotKit message queue depth
- `cua.docs.memory.usage_bytes` - Memory usage in bytes

### CUA MCP Server (cua-mcp-server) - Prefix: `cua.mcp_server`

**Latency Metrics (Histograms):**
- `cua.mcp_server.tool.duration` - Tool execution duration (ms), labels: tool_name, success, session_id
- `cua.mcp_server.session.operation_duration` - Session operation duration (ms)
- `cua.mcp_server.task.duration` - CUA task duration (ms), labels: success, session_id, model_name

**Traffic Metrics (Counters):**
- `cua.mcp_server.tool.calls_total` - Total tool calls, labels: tool_name, success
- `cua.mcp_server.sessions.created_total` - Sessions created
- `cua.mcp_server.tasks.total` - Total CUA tasks, labels: success, model_name
- `cua.mcp_server.messages.processed_total` - Messages processed

**Error Metrics (Counters):**
- `cua.mcp_server.errors.total` - Total errors, labels: error_type, source
- `cua.mcp_server.tool.errors_total` - Tool errors, labels: tool_name, error_type
- `cua.mcp_server.session.errors_total` - Session errors, labels: session_id, error_type
- `cua.mcp_server.task.errors_total` - Task errors, labels: error_type, model_name

**Saturation Metrics (UpDownCounters):**
- `cua.mcp_server.sessions.active` - Active sessions
- `cua.mcp_server.tasks.active` - Active tasks
- `cua.mcp_server.tool.concurrent` - Concurrent tool calls
- `cua.mcp_server.computer_pool.size` - Computer pool size

### CUA Docs Indexer (cua-docs-indexer) - Prefix: `cua.indexer`

**Latency Metrics (Histograms):**
- `cua.indexer.crawl.duration` - Documentation crawl duration (seconds), labels: job_type
- `cua.indexer.index.duration` - Index generation duration (seconds), labels: job_type, db_type, component
- `cua.indexer.query.duration` - MCP query duration (ms), labels: query_type, success, component

**Traffic Metrics (Counters):**
- `cua.indexer.pages.crawled_total` - Pages crawled, labels: job_type
- `cua.indexer.chunks.indexed_total` - Chunks indexed, labels: job_type, db_type
- `cua.indexer.files.indexed_total` - Code files indexed, labels: job_type, component
- `cua.indexer.queries.total` - MCP queries, labels: query_type, success
- `cua.indexer.jobs.total` - Indexing jobs, labels: job_type, component

**Error Metrics (Counters):**
- `cua.indexer.crawl.errors_total` - Crawl errors, labels: job_type
- `cua.indexer.index.errors_total` - Indexing errors, labels: job_type, component
- `cua.indexer.query.errors_total` - Query errors, labels: query_type, error_type

**Saturation Metrics (UpDownCounters):**
- `cua.indexer.jobs.active` - Active indexing jobs
- `cua.indexer.component.queue_depth` - Components waiting to be indexed

## Dashboard Requirements

### Dashboard 1: CUA Platform Overview
Create an executive summary dashboard showing:
- **Row 1: Traffic Overview**
  - Total requests across all services (stat panel)
  - Requests per second by service (time series)
  - CopilotKit messages per minute (time series)

- **Row 2: Latency Overview**
  - P50, P95, P99 latency by service (stat panels)
  - Latency distribution heatmap (heatmap panel)
  - CopilotKit response time trends (time series)

- **Row 3: Error Rate Overview**
  - Error rate percentage by service (gauge panels with thresholds: green <1%, yellow <5%, red >5%)
  - Error timeline (time series with annotations)
  - Top error types table

- **Row 4: Saturation Overview**
  - Active sessions/tasks gauge
  - Concurrent requests over time
  - Memory usage trend

### Dashboard 2: CopilotKit AI Assistant Deep Dive
- **Row 1: Usage Metrics**
  - Messages by category (pie chart)
  - Questions by type (bar chart)
  - Response types distribution (pie chart)

- **Row 2: Performance**
  - Response time by category (time series)
  - Response time histogram
  - P99 latency by question type

- **Row 3: Quality Indicators**
  - Code snippet inclusion rate
  - Error rate by category
  - Conversations with errors

### Dashboard 3: MCP Server Operations
- **Row 1: Task Execution**
  - Tasks per minute (time series)
  - Task duration distribution (histogram)
  - Active tasks gauge

- **Row 2: Tool Usage**
  - Tool calls by name (bar chart)
  - Tool latency comparison (bar chart)
  - Tool error rates (table)

- **Row 3: Session Management**
  - Active sessions over time
  - Session creation rate
  - Session errors

### Dashboard 4: Indexing Pipeline Health
- **Row 1: Job Status**
  - Active jobs indicator
  - Last crawl status and duration
  - Index generation timeline

- **Row 2: Crawling Metrics**
  - Pages crawled per run (stat)
  - Crawl duration trend
  - Crawl error rate

- **Row 3: Code Indexing**
  - Files indexed by component (bar chart)
  - Index duration by component
  - Failed tags count

- **Row 4: Query Performance**
  - Query latency by type (time series)
  - Queries per minute
  - Query error rate

### Dashboard 5: SLI/SLO Tracking
Create SLO-focused dashboard with:
- Availability: % of requests without 5xx errors (target: 99.9%)
- Latency: % of requests under 500ms (target: 95%)
- Error Budget: remaining error budget for the month
- Burn rate alerts

## Alert Rules to Create

1. **High Error Rate**: Error rate > 5% for 5 minutes
2. **High Latency**: P99 latency > 2s for 5 minutes
3. **Saturation Warning**: Concurrent requests > 80% of limit
4. **Indexing Job Failure**: Job fails or takes > 2x average duration
5. **CopilotKit Degradation**: Response time P95 > 10s

## Panel Configuration Notes

- Use `$__rate_interval` for rate calculations
- Set appropriate panel decimals (2 for latency, 0 for counts)
- Include "No Data" states for all panels
- Use consistent color schemes:
  - Green: success/good
  - Yellow: warning
  - Red: error/critical
  - Blue: informational
- Add links between dashboards for drill-down

## Variables to Create

- `$service`: Multi-select for service names
- `$environment`: Environment filter (production, staging)
- `$time_range`: Standard Grafana time range
- `$component`: Component filter for code indexing metrics

## Data Source Configuration

- Prometheus/Mimir: For OTEL metrics
- Tempo: For distributed traces (correlate with TraceID)
- Loki: For logs (correlate with service labels)

Generate the dashboard JSON that can be imported directly into Grafana.
```

---

## Quick Setup Commands

After generating the dashboards, you may need to:

1. **Configure Prometheus Remote Write** to accept OTEL metrics at `otel.cua.ai`
2. **Set up Tempo** for trace correlation
3. **Configure alerting rules** in Grafana Alerting

## Environment Variables

Ensure these are set in your deployment:

```bash
# For cua-docs (Next.js)
OTEL_EXPORTER_OTLP_ENDPOINT=https://otel.cua.ai

# For cua-mcp-server (Python)
OTEL_EXPORTER_OTLP_ENDPOINT=https://otel.cua.ai
CUA_ENVIRONMENT=production

# For Modal indexer
# (Uses hardcoded endpoint in modal_app.py)
```

## Verification

To verify metrics are being exported correctly:

1. Check the OTEL collector logs at `otel.cua.ai`
2. Query Prometheus for `cua_*` metrics
3. Look for service resources with `service.name` labels

---

## Metric Examples for Testing Queries

```promql
# Request rate by service
sum(rate(cua_docs_requests_total[5m])) by (service_name)

# P99 CopilotKit response time
histogram_quantile(0.99, sum(rate(cua_docs_copilot_response_time_bucket[5m])) by (le))

# Error rate
sum(rate(cua_docs_errors_total[5m])) / sum(rate(cua_docs_requests_total[5m])) * 100

# Active sessions
cua_mcp_server_sessions_active

# Indexing job duration
cua_indexer_crawl_duration_seconds
```

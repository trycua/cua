package handlers

import (
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"regexp"
	"strings"

	"cyclops-cs-backend/usage"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"
)

type UsageTimeframe = usage.Timeframe
type UsageInterval = usage.Interval
type UsageQuery = usage.Query
type UsagePoolQuery = usage.PoolQuery
type UsageMetricTotals = usage.MetricTotals
type UsagePoolSummary = usage.PoolSummary
type UsageOverviewResponse = usage.OverviewResponse
type UsagePoolIdentity = usage.PoolIdentity
type UsageBucket = usage.Bucket
type UsagePoolDetailResponse = usage.PoolDetailResponse
type UsageProvider = usage.Provider

const (
	UsageTimeframe24H = usage.Timeframe24H
	UsageTimeframe7D  = usage.Timeframe7D
	UsageTimeframe30D = usage.Timeframe30D
	UsageIntervalHour = usage.IntervalHour
	UsageIntervalDay  = usage.IntervalDay
)

type usageBrowserTimings struct {
	InitialLoadMS    float64 `json:"initial_load_ms"`
	DashboardReadyMS float64 `json:"dashboard_ready_ms"`
}

func (timings usageBrowserTimings) valid() bool {
	const maxDurationMS = 120_000
	return timings.InitialLoadMS >= 0 && timings.InitialLoadMS <= maxDurationMS &&
		timings.DashboardReadyMS >= 0 && timings.DashboardReadyMS <= maxDurationMS &&
		timings.DashboardReadyMS >= timings.InitialLoadMS
}

var usagePoolID = regexp.MustCompile(`^[A-Za-z0-9](?:[A-Za-z0-9._:-]{0,126}[A-Za-z0-9])?$`)

func parseUsageTimeframe(raw string) (UsageTimeframe, UsageInterval, bool) {
	switch UsageTimeframe(raw) {
	case UsageTimeframe24H:
		return UsageTimeframe24H, UsageIntervalHour, true
	case UsageTimeframe7D:
		return UsageTimeframe7D, UsageIntervalHour, true
	case UsageTimeframe30D:
		return UsageTimeframe30D, UsageIntervalDay, true
	}
	return "", "", false
}

func (h Handlers) authorizeUsage(w http.ResponseWriter, r *http.Request) (UsageQuery, UsageInterval, bool) {
	ctx, span := handlerTracer().Start(r.Context(), "usage.authorize")
	defer span.End()

	u := currentUser(r)
	if u == nil || u.ID == "" {
		writeErr(w, 401, "authentication required")
		return UsageQuery{}, "", false
	}
	tf, iv, ok := parseUsageTimeframe(r.URL.Query().Get("timeframe"))
	if !ok {
		writeErr(w, 400, "timeframe must be one of 24h, 7d, or 30d")
		return UsageQuery{}, "", false
	}
	admin, err := h.isAdmin(ctx, u)
	if err != nil {
		admin = false
	}
	sub := strings.TrimSpace(r.URL.Query().Get("subject"))
	if sub == "" {
		sub = u.ID
	} else if sub != u.ID && !admin {
		writeErr(w, 403, "only administrators can select another subject")
		return UsageQuery{}, "", false
	}
	if len(sub) > 255 {
		writeErr(w, 400, "subject is too long")
		return UsageQuery{}, "", false
	}
	span.SetAttributes(
		attribute.String("usage.timeframe", string(tf)),
		attribute.String("usage.interval", string(iv)),
		attribute.Bool("usage.admin", admin),
		attribute.Bool("usage.subject_override", sub != u.ID),
		attribute.Bool("usage.authorized", true),
	)
	return UsageQuery{ActorSubject: u.ID, Subject: sub, Admin: admin, Timeframe: tf}, iv, true
}

func (h Handlers) GetUsageOverview(w http.ResponseWriter, r *http.Request) {
	ctx, span := handlerTracer().Start(r.Context(), "usage.overview")
	defer span.End()
	r = r.WithContext(ctx)
	w.Header().Set("Cache-Control", "private, no-store")
	q, _, ok := h.authorizeUsage(w, r)
	if !ok {
		return
	}
	if h.Usage == nil {
		writeErr(w, 503, "usage data provider is not configured")
		return
	}
	span.SetAttributes(attribute.String("usage.timeframe", string(q.Timeframe)))
	v, err := h.Usage.Overview(r.Context(), q)
	if err != nil {
		markUsageHandlerError(span, "usage overview failed")
		writeErr(w, 502, "usage data is temporarily unavailable")
		return
	}
	writeJSON(w, 200, v)
}

func (h Handlers) GetUsagePoolDetail(w http.ResponseWriter, r *http.Request) {
	ctx, span := handlerTracer().Start(r.Context(), "usage.pool_detail")
	defer span.End()
	r = r.WithContext(ctx)
	w.Header().Set("Cache-Control", "private, no-store")
	q, iv, ok := h.authorizeUsage(w, r)
	if !ok {
		return
	}
	pool := strings.TrimSpace(r.URL.Query().Get("pool"))
	if !usagePoolID.MatchString(pool) {
		writeErr(w, 400, "pool is required and must be a bounded identifier")
		return
	}
	if h.Usage == nil {
		writeErr(w, 503, "usage data provider is not configured")
		return
	}
	span.SetAttributes(
		attribute.String("usage.timeframe", string(q.Timeframe)),
		attribute.String("usage.interval", string(iv)),
	)
	v, err := h.Usage.PoolDetail(r.Context(), UsagePoolQuery{Query: q, PoolID: pool, Interval: iv})
	if errors.Is(err, usage.ErrPoolNotFound) {
		writeErr(w, 404, "usage pool was not found")
		return
	}
	if err != nil {
		markUsageHandlerError(span, "usage pool detail failed")
		writeErr(w, 502, "usage data is temporarily unavailable")
		return
	}
	writeJSON(w, 200, v)
}

func markUsageHandlerError(span trace.Span, description string) {
	span.SetStatus(codes.Error, description)
}

func (h Handlers) RecordUsageBrowserTimings(w http.ResponseWriter, r *http.Request) {
	ctx, span := handlerTracer().Start(r.Context(), "usage.browser_timings")
	defer span.End()
	r = r.WithContext(ctx)
	w.Header().Set("Cache-Control", "private, no-store")
	query, _, ok := h.authorizeUsage(w, r)
	if !ok {
		return
	}

	decoder := json.NewDecoder(io.LimitReader(r.Body, 4<<10))
	decoder.DisallowUnknownFields()
	var timings usageBrowserTimings
	if err := decoder.Decode(&timings); err != nil || !timings.valid() {
		writeErr(w, http.StatusBadRequest, "invalid usage browser timings")
		return
	}
	recordUsageBrowserTimings(ctx, query.Timeframe, timings)
	span.SetAttributes(attribute.String("usage.timeframe", string(query.Timeframe)))
	w.WriteHeader(http.StatusNoContent)
}

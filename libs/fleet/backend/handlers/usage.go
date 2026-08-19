package handlers

import (
	"errors"
	"log/slog"
	"net/http"
	"regexp"
	"strings"

	"cyclops-cs-backend/usage"
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
	enabled, err := h.usageEnabled(r.Context(), u)
	if err != nil || !enabled {
		if err != nil {
			slog.WarnContext(r.Context(), "usage access evaluation failed", "err", err)
		}
		writeErr(w, 403, "usage preview is not enabled")
		return UsageQuery{}, "", false
	}
	admin, err := h.isAdmin(r.Context(), u)
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
	return UsageQuery{ActorSubject: u.ID, Subject: sub, Admin: admin, Timeframe: tf}, iv, true
}

func (h Handlers) GetUsageOverview(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Cache-Control", "private, no-store")
	q, _, ok := h.authorizeUsage(w, r)
	if !ok {
		return
	}
	if h.Usage == nil {
		writeErr(w, 503, "usage data provider is not configured")
		return
	}
	v, err := h.Usage.Overview(r.Context(), q)
	if err != nil {
		writeErr(w, 502, "usage data is temporarily unavailable")
		return
	}
	writeJSON(w, 200, v)
}

func (h Handlers) GetUsagePoolDetail(w http.ResponseWriter, r *http.Request) {
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
	v, err := h.Usage.PoolDetail(r.Context(), UsagePoolQuery{Query: q, PoolID: pool, Interval: iv})
	if errors.Is(err, usage.ErrPoolNotFound) {
		writeErr(w, 404, "usage pool was not found")
		return
	}
	if err != nil {
		writeErr(w, 502, "usage data is temporarily unavailable")
		return
	}
	writeJSON(w, 200, v)
}

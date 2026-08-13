package auth

import (
	"context"
	"net/http/httptest"
	"testing"
	"time"
)

func TestCurrentTimeFactsAreSeparateDeterministicScalars(t *testing.T) {
	now := func() time.Time {
		return time.Date(2026, time.August, 13, 23, 45, 0, 0, time.FixedZone("offset", -7*60*60))
	}
	request := httptest.NewRequest("GET", "/", nil)

	yearFacts, err := CurrentYearFacts(now).LoadFacts(context.Background(), request)
	if err != nil {
		t.Fatalf("load current year: %v", err)
	}
	monthFacts, err := CurrentMonthFacts(now).LoadFacts(context.Background(), request)
	if err != nil {
		t.Fatalf("load current month: %v", err)
	}
	if len(yearFacts) != 1 || yearFacts["current_year"] != 2026 {
		t.Fatalf("year facts = %#v, want current_year:2026", yearFacts)
	}
	if len(monthFacts) != 1 || monthFacts["current_month"] != 8 {
		t.Fatalf("month facts = %#v, want current_month:8", monthFacts)
	}
	if CurrentYearFacts(now).CacheKey() == CurrentMonthFacts(now).CacheKey() {
		t.Fatal("year and month providers must have distinct cache keys")
	}
}

func TestCurrentTimeFactsShareOneLazyRequestSnapshot(t *testing.T) {
	yearCalls := 0
	monthCalls := 0
	yearClock := func() time.Time {
		yearCalls++
		return time.Date(2026, time.December, 31, 23, 59, 59, 0, time.UTC)
	}
	monthClock := func() time.Time {
		monthCalls++
		return time.Date(2027, time.January, 1, 0, 0, 0, 0, time.UTC)
	}
	input := newRequestPolicyInput(httptest.NewRequest("GET", "/", nil), 0)

	document, err := input.forPolicy(context.Background(), policyConfig{factProviders: []factProviderConfig{
		{namespace: TimeFactNamespace, provider: CurrentYearFacts(yearClock)},
		{namespace: TimeFactNamespace, provider: CurrentMonthFacts(monthClock)},
	}})
	if err != nil {
		t.Fatalf("resolve time facts: %v", err)
	}
	timeFacts := document["facts"].(map[string]any)[TimeFactNamespace].(map[string]any)
	if timeFacts["current_year"] != 2026 || timeFacts["current_month"] != 12 {
		t.Fatalf("time facts = %#v, want one 2026-12 snapshot", timeFacts)
	}
	if yearCalls != 1 || monthCalls != 0 {
		t.Fatalf("clock calls = year:%d month:%d, want 1/0", yearCalls, monthCalls)
	}
}
